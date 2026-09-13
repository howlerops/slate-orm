//! Index expressions: the grammar, and building a [`Scalar`].
//!
//! ```text
//! scalar  := sum
//! sum     := product ( ( '+' | '-' ) product )*
//! product := primary ( ( '*' | '/' ) primary )*
//! primary := '(' scalar ')' | call | column | string | number
//! call    := 'lower' '(' scalar ')'
//!          | 'upper' '(' scalar ')'
//!          | 'length' '(' scalar ')'
//!          | 'concat' '(' scalar ( ',' scalar )* ')'
//!          | 'coalesce' '(' scalar ( ',' scalar )* ')'
//!          | 'regexp_replace' '(' scalar ',' string ',' string ')'
//! ```
//!
//! # Typing here is not typing in a predicate
//!
//! A predicate's literal always has a column opposite it, so the column
//! decides what it is. A scalar's literal has nothing opposite it —
//! `length(url) + 1` — so a rule is needed, and the rule is the kernel's own:
//! its arithmetic promotes two integers to `i64` and anything else to `f64`,
//! so an integer literal here is [`Value::I64`] and a decimal literal is
//! [`Value::F64`]. Inventing a different rule would make the same expression
//! mean one thing in an index and another in a query.
//!
//! The type the whole expression *produces* is declared in the file rather
//! than inferred, because [`IndexExpression`](slate_schema::IndexExpression)
//! requires it: the decoder needs the type before it has a row to run the
//! expression against. It is not taken on trust — the record store checks each
//! computed value against the declaration at the write that would otherwise
//! have made the entry undecodable.
//!
//! # What is not here
//!
//! `CASE`, `EXTRACT`, `DATE_TRUNC` and vector distance. Each needs a
//! sub-language of its own — a branch list of predicate/value pairs, a time
//! unit, a metric — and each would be surface syntax invented here for
//! something no other part of this file needs. An index keyed on a vector
//! distance is refused by the schema layer anyway, since a vector's order says
//! nothing about similarity. These are recorded as a gap rather than
//! half-built: an operator who needs one writes a Rust `Scalar` and links
//! against `slate-server` directly, which is what this binary exists to make
//! unnecessary in the common case and does not claim to make unnecessary in
//! every case.

use super::lex::{Kind, Token, tokenize};
use super::{LangError, LangResult, Scope};
use slate_kernel::Scalar;
use slate_tuple::Value;

/// Parse `source` as an index expression over `scope`.
pub(crate) fn parse(source: &str, scope: &dyn Scope) -> LangResult<Scalar> {
    let tokens = tokenize(source)?;
    let mut parser = Parser {
        tokens: &tokens,
        position: 0,
        end: source.len(),
        scope,
    };
    let scalar = parser.sum()?;
    if let Some(token) = parser.peek() {
        return Err(LangError::new(
            token.at,
            format!(
                "{} is left over; the expression already ended",
                token.kind.describe()
            ),
        ));
    }
    Ok(scalar)
}

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
    end: usize,
    scope: &'a dyn Scope,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn at(&self) -> usize {
        self.peek().map_or(self.end, |t| t.at)
    }

    fn peek_punct(&self, punct: &str) -> bool {
        matches!(self.peek().map(|t| &t.kind), Some(Kind::Punct(p)) if *p == punct)
    }

    fn eat_punct(&mut self, punct: &str) -> bool {
        if self.peek_punct(punct) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, punct: &str) -> LangResult<()> {
        if self.eat_punct(punct) {
            return Ok(());
        }
        Err(LangError::new(
            self.at(),
            match self.peek() {
                Some(token) => format!("expected `{punct}`, found {}", token.kind.describe()),
                None => format!("expected `{punct}`, and the expression ended"),
            },
        ))
    }

    fn sum(&mut self) -> LangResult<Scalar> {
        let mut left = self.product()?;
        loop {
            if self.eat_punct("+") {
                left = Scalar::Add(Box::new(left), Box::new(self.product()?));
            } else if self.eat_punct("-") {
                left = Scalar::Sub(Box::new(left), Box::new(self.product()?));
            } else {
                return Ok(left);
            }
        }
    }

    fn product(&mut self) -> LangResult<Scalar> {
        let mut left = self.primary()?;
        loop {
            if self.eat_punct("*") {
                left = Scalar::Mul(Box::new(left), Box::new(self.primary()?));
            } else if self.eat_punct("/") {
                left = Scalar::Div(Box::new(left), Box::new(self.primary()?));
            } else {
                return Ok(left);
            }
        }
    }

    fn primary(&mut self) -> LangResult<Scalar> {
        let at = self.at();
        if self.eat_punct("(") {
            let inner = self.sum()?;
            self.expect_punct(")")?;
            return Ok(inner);
        }
        // Only a literal may be negated. A general unary minus would have to
        // become `0 - x`, and `0` is an `i64`, so negating a float column would
        // silently promote it — a rule nobody would guess from the text.
        if self.eat_punct("-") {
            let number_at = self.at();
            return match self.next_kind() {
                Some(Kind::Number(text)) => {
                    number(&format!("-{text}"), number_at).map(Scalar::Literal)
                }
                _ => Err(LangError::new(
                    number_at,
                    "only a number may be negated here; write `0 - x` if that is what you mean",
                )),
            };
        }

        match self.next_kind() {
            Some(Kind::Number(text)) => number(&text, at).map(Scalar::Literal),
            Some(Kind::Str(text)) => Ok(Scalar::Literal(Value::Str(text))),
            Some(Kind::Placeholder(name)) => Err(LangError::new(
                at,
                format!(
                    "`:{name}` names the caller, and an index key is computed when a row is written rather than when one is read"
                ),
            )),
            Some(Kind::Word(word)) => {
                if self.peek_punct("(") {
                    self.call(&word, at)
                } else {
                    self.scope
                        .ordinal(&word)
                        .map(Scalar::Column)
                        .ok_or_else(|| {
                            LangError::new(
                                at,
                                format!(
                                    "there is no column `{word}` here, and no function of that name; this table has {}",
                                    super::pred::list_columns(&self.scope.column_names())
                                ),
                            )
                        })
                }
            }
            Some(Kind::Punct(p)) => {
                Err(LangError::new(at, format!("expected a value, found `{p}`")))
            }
            None => Err(LangError::new(at, "expected a value")),
        }
    }

    fn next_kind(&mut self) -> Option<Kind> {
        let kind = self.tokens.get(self.position).map(|t| t.kind.clone());
        if kind.is_some() {
            self.position += 1;
        }
        kind
    }

    /// A function call. The name has been read; `(` is next.
    fn call(&mut self, name: &str, at: usize) -> LangResult<Scalar> {
        self.expect_punct("(")?;
        let lowered = name.to_ascii_lowercase();

        let scalar = match lowered.as_str() {
            "lower" => Scalar::Lower(Box::new(self.sum()?)),
            "upper" => Scalar::Upper(Box::new(self.sum()?)),
            "length" => Scalar::Length(Box::new(self.sum()?)),
            "concat" | "coalesce" => {
                let mut parts = vec![self.sum()?];
                while self.eat_punct(",") {
                    parts.push(self.sum()?);
                }
                if lowered == "concat" {
                    Scalar::Concat(parts)
                } else {
                    Scalar::Coalesce(parts)
                }
            }
            "regexp_replace" => {
                let value = Box::new(self.sum()?);
                self.expect_punct(",")?;
                let pattern_at = self.at();
                let pattern = self.string_argument("regexp_replace")?;
                self.expect_punct(",")?;
                let replacement = self.string_argument("regexp_replace")?;
                // The kernel treats an uncompilable pattern as a no-op, which
                // is right for a query and wrong for an index: every entry
                // would key on the untouched value and no read would ever say
                // why. Checked here through `Expr::regex_error`, which is the
                // same compiler the evaluator uses.
                let probe = slate_kernel::Expr::Matches {
                    column: slate_schema::Ordinal(0),
                    pattern: pattern.clone(),
                    negated: false,
                    insensitive: false,
                };
                if let Some(why) = probe.regex_error() {
                    return Err(LangError::new(
                        pattern_at,
                        format!("`{pattern}` is not a valid regular expression: {why}"),
                    ));
                }
                Scalar::RegexpReplace {
                    value,
                    pattern,
                    replacement,
                }
            }
            other => {
                return Err(LangError::new(
                    at,
                    format!(
                        "`{other}` is not a function this server knows here; there are lower, upper, length, concat, coalesce and regexp_replace. CASE, EXTRACT, DATE_TRUNC and vector distance are not expressible in a configuration file — see the crate documentation"
                    ),
                ));
            }
        };

        self.expect_punct(")")?;
        Ok(scalar)
    }

    fn string_argument(&mut self, function: &str) -> LangResult<String> {
        let at = self.at();
        match self.next_kind() {
            Some(Kind::Str(text)) => Ok(text),
            Some(other) => Err(LangError::new(
                at,
                format!(
                    "`{function}` takes a quoted string here, found {}",
                    other.describe()
                ),
            )),
            None => Err(LangError::new(
                at,
                format!("`{function}` takes a quoted string here"),
            )),
        }
    }
}

/// An integer literal is an `i64` and a decimal literal is an `f64`, matching
/// the kernel's own arithmetic promotion.
fn number(text: &str, at: usize) -> LangResult<Value> {
    if text.contains('.') {
        text.parse::<f64>()
            .map(Value::F64)
            .map_err(|_| LangError::new(at, format!("`{text}` is not a number")))
    } else {
        text.parse::<i64>().map(Value::I64).map_err(|_| {
            LangError::new(
                at,
                format!(
                    "`{text}` does not fit in an i64; write it with a decimal point for a float"
                ),
            )
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::lang::TableScope;
    use slate_schema::{Ordinal, Row, TableDef, TableId};
    use slate_tuple::ValueType;

    fn table() -> TableDef {
        TableDef::builder("docs", TableId(1))
            .column("id", ValueType::U64)
            .column("email", ValueType::Str)
            .column("size", ValueType::I64)
            .nullable_column("note", ValueType::Str)
            .primary_key(["id"])
            .build()
            .unwrap()
    }

    fn scalar(source: &str) -> LangResult<Scalar> {
        let table = table();
        parse(source, &TableScope::constant(&table))
    }

    fn row() -> Row {
        Row::new(vec![
            Value::U64(1),
            Value::Str("Ada@Example.com".into()),
            Value::I64(10),
            Value::Null,
        ])
    }

    #[test]
    fn lower_of_a_column_is_the_kernels_lower() {
        let parsed = scalar("lower(email)").unwrap();
        assert_eq!(parsed, Scalar::Lower(Box::new(Scalar::Column(Ordinal(1)))));
        assert_eq!(
            parsed.evaluate(&row()),
            Value::Str("ada@example.com".into())
        );
    }

    #[test]
    fn arithmetic_associates_left_and_multiplies_first() {
        let parsed = scalar("size + 2 * 3").unwrap();
        assert_eq!(parsed.evaluate(&row()), Value::I64(16));
        let parsed = scalar("(size + 2) * 3").unwrap();
        assert_eq!(parsed.evaluate(&row()), Value::I64(36));
        let parsed = scalar("size - 1 - 2").unwrap();
        assert_eq!(parsed.evaluate(&row()), Value::I64(7));
    }

    #[test]
    fn an_integer_literal_is_an_i64_and_a_decimal_is_an_f64() {
        assert_eq!(scalar("1").unwrap(), Scalar::Literal(Value::I64(1)));
        assert_eq!(scalar("1.5").unwrap(), Scalar::Literal(Value::F64(1.5)));
        assert_eq!(scalar("-2").unwrap(), Scalar::Literal(Value::I64(-2)));
    }

    #[test]
    fn coalesce_and_concat_take_any_number_of_arguments() {
        assert_eq!(
            scalar("coalesce(note, 'none')").unwrap().evaluate(&row()),
            Value::Str("none".into())
        );
        assert_eq!(
            scalar("concat(email, '!', email)")
                .unwrap()
                .evaluate(&row()),
            Value::Str("Ada@Example.com!Ada@Example.com".into())
        );
    }

    #[test]
    fn an_unknown_function_lists_the_ones_that_exist_and_the_ones_that_do_not() {
        let error = scalar("date_trunc(size)").unwrap_err();
        assert!(
            error.message.contains("regexp_replace"),
            "{}",
            error.message
        );
        assert!(error.message.contains("DATE_TRUNC"), "{}", error.message);
    }

    #[test]
    fn a_bad_regex_in_regexp_replace_is_refused() {
        let error = scalar("regexp_replace(email, '(', 'x')").unwrap_err();
        assert!(
            error.message.contains("not a valid regular expression"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_placeholder_is_refused_in_an_index_key() {
        let error = scalar("coalesce(email, :principal)").unwrap_err();
        assert!(
            error.message.contains("when a row is written"),
            "{}",
            error.message
        );
    }

    #[test]
    fn an_unknown_name_is_reported_as_neither_column_nor_function() {
        let error = scalar("nope").unwrap_err();
        assert!(
            error.message.contains("no function of that name"),
            "{}",
            error.message
        );
    }
}
