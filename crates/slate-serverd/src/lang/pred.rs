//! Predicates: the grammar, and lowering to [`Expr`].
//!
//! ```text
//! predicate := disjunction
//! disjunction := conjunction ( 'OR' conjunction )*
//! conjunction := negation ( 'AND' negation )*
//! negation    := 'NOT' negation | primary
//! primary     := '(' predicate ')' | 'TRUE' | 'FALSE' | test
//! test        := column 'IS' [ 'NOT' ] 'NULL'
//!              | column [ 'NOT' ] ( 'LIKE' | 'ILIKE' ) string
//!              | column [ 'NOT' ] 'IN' '(' operand ( ',' operand )* ')'
//!              | column ( '~' | '~*' | '!~' | '!~*' ) string
//!              | column ( '=' | '<>' | '!=' | '<' | '<=' | '>' | '>=' ) operand
//! operand     := string | number | 'TRUE' | 'FALSE' | column | placeholder
//! ```
//!
//! Every [`Expr`] variant is reachable, which is the property that makes this
//! a surface syntax for the kernel's predicates rather than a subset of them.
//!
//! # Why the parse result is not an `Expr`
//!
//! A row policy is a function of the caller: `owner = :principal` is a
//! different `Expr` for every principal, and
//! [`Policy`](slate_kernel::Policy) takes a closure precisely so that it can
//! be. The parse result therefore has one node `Expr` does not — a
//! placeholder — and is lowered to an `Expr` once per request, which is the
//! same work a hand-written Rust closure does.
//!
//! Re-parsing the source per request was the obvious alternative and was
//! rejected on measurement grounds: the head node's own work is 7–23 µs per
//! request (`docs/performance.md`), and string scanning per read would be
//! visible against that where a tree walk over a dozen nodes is not.
//!
//! This is *not* a second expression language. Nothing here evaluates a row;
//! [`Pred`] is a parser's syntax tree, lowered to the kernel's `Expr` and
//! discarded.

use super::lex::{Kind, Token, tokenize};
use super::{LangError, LangResult, Scope};
use slate_kernel::{CmpOp, Expr, SecurityContext};
use slate_schema::Ordinal;
use slate_tuple::{Value, ValueType};

/// One side of a comparison, once the column on the other side has fixed its
/// type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Operand {
    /// A literal, already converted to the column's declared type.
    Literal(Value),
    /// Another column of the same row.
    Column(Ordinal),
    /// The asking principal's id.
    Principal,
    /// The asking principal's tenant.
    Tenant,
}

/// A parsed predicate: [`Expr`] plus placeholders.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Pred {
    /// Every row.
    True,
    /// No row.
    False,
    /// All of.
    And(Vec<Pred>),
    /// Any of.
    Or(Vec<Pred>),
    /// Negation, three-valued as in SQL.
    Not(Box<Pred>),
    /// `column <op> operand`.
    Compare {
        column: Ordinal,
        op: CmpOp,
        rhs: Operand,
    },
    /// `column IS [NOT] NULL`.
    IsNull { column: Ordinal, negated: bool },
    /// `column [NOT] (I)LIKE pattern`.
    Like {
        column: Ordinal,
        pattern: String,
        negated: bool,
        insensitive: bool,
    },
    /// `column ~ pattern` and its three variants.
    Matches {
        column: Ordinal,
        pattern: String,
        negated: bool,
        insensitive: bool,
    },
    /// `column [NOT] IN (…)`.
    In {
        column: Ordinal,
        values: Vec<Operand>,
        negated: bool,
    },
}

impl Pred {
    /// Whether this predicate reads the caller's identity.
    ///
    /// A `CHECK` and a partial index are lowered once at startup, so one that
    /// did would be lowered against nobody. The parser refuses a placeholder
    /// in those scopes; this is the second answer to the same question, used
    /// to decide whether a policy can be lowered once and shared.
    pub(crate) fn is_constant(&self) -> bool {
        match self {
            Self::True
            | Self::False
            | Self::IsNull { .. }
            | Self::Like { .. }
            | Self::Matches { .. } => true,
            Self::And(parts) | Self::Or(parts) => parts.iter().all(Self::is_constant),
            Self::Not(inner) => inner.is_constant(),
            Self::Compare { rhs, .. } => {
                matches!(rhs, Operand::Literal(_) | Operand::Column(_))
            }
            Self::In { values, .. } => values
                .iter()
                .all(|v| matches!(v, Operand::Literal(_) | Operand::Column(_))),
        }
    }

    /// Build the kernel expression this predicate means for `context`.
    ///
    /// # A placeholder the caller cannot supply
    ///
    /// `:tenant` against a principal with no tenant lowers to
    /// [`Value::Null`], which makes every comparison it appears in *unknown*
    /// under the kernel's three-valued logic, and an unknown never admits a
    /// row. Negation does not rescue it — `NOT unknown` is unknown — so this
    /// fails closed in every position a placeholder can occupy, while still
    /// letting a *different* policy on the same table admit rows through the
    /// `OR` the security catalog combines them with. Lowering the whole
    /// predicate to [`Expr::False`] instead would be equally safe and would
    /// additionally suppress those other policies, which is not the security
    /// catalog's rule.
    pub(crate) fn lower(&self, context: &SecurityContext) -> Expr {
        match self {
            Self::True => Expr::True,
            Self::False => Expr::False,
            Self::And(parts) => Expr::And(parts.iter().map(|p| p.lower(context)).collect()),
            Self::Or(parts) => Expr::Or(parts.iter().map(|p| p.lower(context)).collect()),
            Self::Not(inner) => Expr::Not(Box::new(inner.lower(context))),
            Self::Compare { column, op, rhs } => match rhs {
                Operand::Column(right) => Expr::CompareColumns {
                    left: *column,
                    op: *op,
                    right: *right,
                },
                other => Expr::Compare {
                    column: *column,
                    op: *op,
                    value: resolve(other, context),
                },
            },
            Self::IsNull { column, negated } => Expr::IsNull {
                column: *column,
                negated: *negated,
            },
            Self::Like {
                column,
                pattern,
                negated,
                insensitive,
            } => Expr::Like {
                column: *column,
                pattern: pattern.clone(),
                negated: *negated,
                insensitive: *insensitive,
            },
            Self::Matches {
                column,
                pattern,
                negated,
                insensitive,
            } => Expr::Matches {
                column: *column,
                pattern: pattern.clone(),
                negated: *negated,
                insensitive: *insensitive,
            },
            Self::In {
                column,
                values,
                negated,
            } => {
                let inner = Expr::In {
                    column: *column,
                    values: values.iter().map(|v| resolve(v, context)).collect(),
                };
                if *negated {
                    Expr::Not(Box::new(inner))
                } else {
                    inner
                }
            }
        }
    }
}

/// A placeholder's value for this caller. A `Column` never reaches here — it
/// becomes [`Expr::CompareColumns`] instead, which has no value at all.
fn resolve(operand: &Operand, context: &SecurityContext) -> Value {
    match operand {
        Operand::Literal(value) => value.clone(),
        Operand::Principal => context.principal().id.clone(),
        Operand::Tenant => context.principal().tenant.clone().unwrap_or(Value::Null),
        Operand::Column(_) => Value::Null,
    }
}

/// Parse `source` as a predicate over `scope`.
pub(crate) fn parse(source: &str, scope: &dyn Scope) -> LangResult<Pred> {
    let tokens = tokenize(source)?;
    let mut parser = Parser {
        tokens: &tokens,
        position: 0,
        end: source.len(),
        scope,
    };
    let predicate = parser.disjunction()?;
    if let Some(token) = parser.peek() {
        return Err(LangError::new(
            token.at,
            format!(
                "{} is left over; the expression already ended",
                token.kind.describe()
            ),
        ));
    }
    Ok(predicate)
}

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
    /// Offset to blame when the expression ends too early.
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

    fn advance(&mut self) -> Option<&Token> {
        let token = self.tokens.get(self.position);
        if token.is_some() {
            self.position += 1;
        }
        token
    }

    /// Whether the next token is the keyword `word`, case-insensitively.
    fn peek_keyword(&self, word: &str) -> bool {
        matches!(self.peek().map(|t| &t.kind), Some(Kind::Word(w)) if w.eq_ignore_ascii_case(word))
    }

    fn eat_keyword(&mut self, word: &str) -> bool {
        if self.peek_keyword(word) {
            self.position += 1;
            true
        } else {
            false
        }
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

    fn disjunction(&mut self) -> LangResult<Pred> {
        let mut parts = vec![self.conjunction()?];
        while self.eat_keyword("or") {
            parts.push(self.conjunction()?);
        }
        Ok(if parts.len() == 1 {
            parts.pop().unwrap_or(Pred::True)
        } else {
            Pred::Or(parts)
        })
    }

    fn conjunction(&mut self) -> LangResult<Pred> {
        let mut parts = vec![self.negation()?];
        while self.eat_keyword("and") {
            parts.push(self.negation()?);
        }
        Ok(if parts.len() == 1 {
            parts.pop().unwrap_or(Pred::True)
        } else {
            Pred::And(parts)
        })
    }

    fn negation(&mut self) -> LangResult<Pred> {
        if self.eat_keyword("not") {
            return Ok(Pred::Not(Box::new(self.negation()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> LangResult<Pred> {
        if self.eat_punct("(") {
            let inner = self.disjunction()?;
            self.expect_punct(")")?;
            return Ok(inner);
        }
        if self.eat_keyword("true") {
            return Ok(Pred::True);
        }
        if self.eat_keyword("false") {
            return Ok(Pred::False);
        }
        self.test()
    }

    /// One comparison, starting at the column it is about.
    fn test(&mut self) -> LangResult<Pred> {
        let column = self.column()?;

        if self.eat_keyword("is") {
            let negated = self.eat_keyword("not");
            if !self.eat_keyword("null") {
                return Err(LangError::new(
                    self.at(),
                    "expected `NULL` after `IS`; the only `IS` test is `IS NULL` and `IS NOT NULL`",
                ));
            }
            return Ok(Pred::IsNull { column, negated });
        }

        let negated = self.eat_keyword("not");
        if self.eat_keyword("like") {
            return Ok(Pred::Like {
                column,
                pattern: self.pattern("LIKE")?,
                negated,
                insensitive: false,
            });
        }
        if self.eat_keyword("ilike") {
            return Ok(Pred::Like {
                column,
                pattern: self.pattern("ILIKE")?,
                negated,
                insensitive: true,
            });
        }
        if self.eat_keyword("in") {
            self.expect_punct("(")?;
            let mut values = Vec::new();
            loop {
                let at = self.at();
                let operand = self.operand(column)?;
                // `Expr::In` holds values, not columns: a set membership test
                // against another column of the same row is `=` repeated, and
                // accepting it here would have to lower to something that
                // matches nothing.
                if matches!(operand, Operand::Column(_)) {
                    return Err(LangError::new(
                        at,
                        "an `IN` list holds values, not columns; write `a = b OR a = c` to compare columns",
                    ));
                }
                values.push(operand);
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct(")")?;
            return Ok(Pred::In {
                column,
                values,
                negated,
            });
        }
        if negated {
            return Err(LangError::new(
                self.at(),
                "expected `LIKE`, `ILIKE` or `IN` after `NOT`; to negate a comparison, put `NOT` in front of it",
            ));
        }

        for (punct, insensitive, negated) in [
            ("~", false, false),
            ("~*", true, false),
            ("!~", false, true),
            ("!~*", true, true),
        ] {
            if self.eat_punct(punct) {
                let pattern_at = self.at();
                let pattern = self.pattern(punct)?;
                // A pattern that does not compile matches nothing rather than
                // failing the query, which is right at query time and wrong in
                // a configuration file: an index or a policy that silently
                // matches nothing is the failure this refusal exists to
                // prevent.
                let probe = Expr::Matches {
                    column,
                    pattern: pattern.clone(),
                    negated,
                    insensitive,
                };
                if let Some(why) = probe.regex_error() {
                    return Err(LangError::new(
                        pattern_at,
                        format!("`{pattern}` is not a valid regular expression: {why}"),
                    ));
                }
                return Ok(Pred::Matches {
                    column,
                    pattern,
                    negated,
                    insensitive,
                });
            }
        }

        let op = self.comparison_operator()?;
        let rhs = self.operand(column)?;
        Ok(Pred::Compare { column, op, rhs })
    }

    fn comparison_operator(&mut self) -> LangResult<CmpOp> {
        let at = self.at();
        for (punct, op) in [
            ("=", CmpOp::Eq),
            ("<>", CmpOp::Ne),
            ("!=", CmpOp::Ne),
            ("<=", CmpOp::Le),
            (">=", CmpOp::Ge),
            ("<", CmpOp::Lt),
            (">", CmpOp::Gt),
        ] {
            if self.eat_punct(punct) {
                return Ok(op);
            }
        }
        Err(LangError::new(
            at,
            match self.peek() {
                Some(token) => format!(
                    "expected a comparison after the column, found {}",
                    token.kind.describe()
                ),
                None => "expected a comparison after the column".to_owned(),
            },
        ))
    }

    /// A column name, resolved against the scope.
    fn column(&mut self) -> LangResult<Ordinal> {
        let at = self.at();
        let Some(token) = self.advance() else {
            return Err(LangError::new(at, "expected a column name"));
        };
        let Kind::Word(name) = &token.kind else {
            return Err(LangError::new(
                at,
                format!("expected a column name, found {}", token.kind.describe()),
            ));
        };
        let name = name.clone();
        self.resolve_column(&name, at)
    }

    fn resolve_column(&self, name: &str, at: usize) -> LangResult<Ordinal> {
        self.scope.ordinal(name).ok_or_else(|| {
            LangError::new(
                at,
                format!(
                    "there is no column `{name}` here; this table has {}",
                    list_columns(&self.scope.column_names())
                ),
            )
        })
    }

    /// The string after a pattern operator.
    fn pattern(&mut self, operator: &str) -> LangResult<String> {
        let at = self.at();
        match self.advance().map(|t| t.kind.clone()) {
            Some(Kind::Str(text)) => Ok(text),
            Some(other) => Err(LangError::new(
                at,
                format!(
                    "{operator} takes a quoted pattern, found {}",
                    other.describe()
                ),
            )),
            None => Err(LangError::new(
                at,
                format!("{operator} takes a quoted pattern"),
            )),
        }
    }

    /// The right-hand side of a comparison against `column`.
    fn operand(&mut self, column: Ordinal) -> LangResult<Operand> {
        let at = self.at();
        let Some(token) = self.advance().cloned() else {
            return Err(LangError::new(at, "expected a value"));
        };
        let Some(declared) = self.scope.value_type(column) else {
            return Err(LangError::new(
                at,
                "the column on the left has no declared type",
            ));
        };

        match token.kind {
            Kind::Placeholder(name) => {
                if !self.scope.allows_placeholders() {
                    return Err(LangError::new(
                        at,
                        format!(
                            "`:{name}` names the caller, and this expression is evaluated with no caller. Only a `[[security.policies]]` predicate may use `:principal` and `:tenant`"
                        ),
                    ));
                }
                match name.as_str() {
                    "principal" => Ok(Operand::Principal),
                    "tenant" => Ok(Operand::Tenant),
                    other => Err(LangError::new(
                        at,
                        format!(
                            "`:{other}` is not a placeholder this server knows; there are `:principal` and `:tenant`"
                        ),
                    )),
                }
            }
            Kind::Str(text) => literal_from_string(&text, declared, at).map(Operand::Literal),
            Kind::Number(text) => literal_from_number(&text, declared, at).map(Operand::Literal),
            Kind::Punct("-") => {
                let next_at = self.at();
                match self.advance().map(|t| t.kind.clone()) {
                    Some(Kind::Number(text)) => {
                        literal_from_number(&format!("-{text}"), declared, at).map(Operand::Literal)
                    }
                    _ => Err(LangError::new(next_at, "expected a number after `-`")),
                }
            }
            Kind::Word(word)
                if word.eq_ignore_ascii_case("true") || word.eq_ignore_ascii_case("false") =>
            {
                if declared == ValueType::Bool {
                    Ok(Operand::Literal(Value::Bool(
                        word.eq_ignore_ascii_case("true"),
                    )))
                } else {
                    Err(LangError::new(
                        at,
                        format!("`{word}` is a bool and the column opposite it is {declared}"),
                    ))
                }
            }
            Kind::Word(word) if word.eq_ignore_ascii_case("null") => Err(LangError::new(
                at,
                "comparing to NULL is always unknown and so admits no row; write `IS NULL` or `IS NOT NULL`",
            )),
            Kind::Word(word) => {
                let other = self.resolve_column(&word, at)?;
                // The kernel refuses a cross-type column comparison at
                // evaluation, because `Value`'s order is type-first and an
                // integer against a float would order by type and answer the
                // same way for every row. Refusing it here means the operator
                // finds out at startup instead of finding out never.
                let right = self.scope.value_type(other);
                if right.is_some_and(|r| r != declared) {
                    return Err(LangError::new(
                        at,
                        format!(
                            "`{word}` is {} and the column opposite it is {declared}; a comparison between two types orders by type and answers the same way for every row",
                            right.map_or_else(|| "untyped".to_owned(), |r| r.to_string())
                        ),
                    ));
                }
                Ok(Operand::Column(other))
            }
            Kind::Punct(p) => Err(LangError::new(at, format!("expected a value, found `{p}`"))),
        }
    }
}

/// Turn a quoted literal into the column's declared type.
pub(crate) fn literal_from_string(text: &str, declared: ValueType, at: usize) -> LangResult<Value> {
    match declared {
        ValueType::Str => Ok(Value::Str(text.to_owned())),
        ValueType::Uuid => uuid::Uuid::parse_str(text)
            .map(Value::Uuid)
            .map_err(|_| LangError::new(at, format!("`{text}` is not a uuid"))),
        // Hex rather than base64 or an escape syntax: a byte column in a
        // configuration file is a key or a hash, hex is what those are printed
        // as everywhere else, and an odd length or a stray character is a
        // refusal rather than a silently different value.
        ValueType::Bytes => decode_hex(text)
            .map(|bytes| Value::Bytes(bytes.into()))
            .ok_or_else(|| {
                LangError::new(
                    at,
                    format!("`{text}` is not hexadecimal; a bytes column takes an even number of hex digits"),
                )
            }),
        ValueType::Bool | ValueType::I64 | ValueType::U64 | ValueType::F64 => Err(LangError::new(
            at,
            format!("a quoted string cannot be compared with a {declared} column"),
        )),
        ValueType::Vector => Err(LangError::new(
            at,
            "a vector cannot be compared here; its order is deterministic but says nothing about similarity, which is why the schema layer refuses one in a key or an index",
        )),
        // `ValueType` is `#[non_exhaustive]`, so a type added upstream lands
        // here rather than being given some plausible conversion. Refusing is
        // the safe direction: an unparsed literal is a startup error, and a
        // guessed one is a predicate that matches the wrong rows.
        other => Err(LangError::new(
            at,
            format!("this server does not know how to write a {other} literal in an expression"),
        )),
    }
}

/// Turn a numeric literal into the column's declared type.
///
/// The column decides, so `10` is a `u64` opposite a `u64` column and an `i64`
/// opposite an `i64` one. Guessing from the text instead would make `id = 7`
/// mean a different thing from `id = -7` in the same table.
pub(crate) fn literal_from_number(text: &str, declared: ValueType, at: usize) -> LangResult<Value> {
    match declared {
        ValueType::I64 => text
            .parse::<i64>()
            .map(Value::I64)
            .map_err(|_| LangError::new(at, format!("`{text}` is not an i64"))),
        ValueType::U64 => text.parse::<u64>().map(Value::U64).map_err(|_| {
            LangError::new(
                at,
                format!(
                    "`{text}` is not a u64; a u64 column cannot hold a negative or fractional value"
                ),
            )
        }),
        ValueType::F64 => text
            .parse::<f64>()
            .map(Value::F64)
            .map_err(|_| LangError::new(at, format!("`{text}` is not a number"))),
        other => Err(LangError::new(
            at,
            format!("`{text}` is a number and the column opposite it is {other}"),
        )),
    }
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len() / 2)
        .map(|i| {
            let pair = text.get(i * 2..i * 2 + 2)?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

/// `a`, `b` and `c`, or "no columns at all" — used in error messages, where an
/// empty list rendered as an empty string reads as a truncated sentence.
pub(crate) fn list_columns(names: &[String]) -> String {
    match names {
        [] => "no columns at all".to_owned(),
        [only] => format!("`{only}`"),
        [rest @ .., last] => format!(
            "{} and `{last}`",
            rest.iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
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
    use slate_kernel::Principal;
    use slate_schema::{TableDef, TableId};

    fn table() -> TableDef {
        TableDef::builder("docs", TableId(1))
            .column("id", ValueType::U64)
            .column("kind", ValueType::Str)
            .column("size", ValueType::I64)
            .column("ratio", ValueType::F64)
            .column("live", ValueType::Bool)
            .column("owner", ValueType::U64)
            .nullable_column("note", ValueType::Str)
            .primary_key(["id"])
            .build()
            .unwrap()
    }

    fn parse_constant(source: &str) -> LangResult<Pred> {
        let table = table();
        parse(source, &TableScope::constant(&table))
    }

    fn parse_policy(source: &str) -> LangResult<Pred> {
        let table = table();
        parse(source, &TableScope::per_caller(&table))
    }

    fn caller() -> SecurityContext {
        SecurityContext::new(Principal::new(Value::U64(7)).with_tenant(Value::U64(3)))
    }

    /// The literal a comparison was built with, whatever it is.
    fn literal(expression: &Expr) -> Value {
        match expression {
            Expr::Compare { value, .. } => value.clone(),
            other => panic!("expected a comparison, got {other:?}"),
        }
    }

    #[test]
    fn a_literal_takes_the_type_of_the_column_opposite_it() {
        // The same text, three columns, three different `Value` *variants* —
        // which is the property that lets the file carry no type tags.
        //
        // Asserted with `matches!` rather than `assert_eq!`, and that is not a
        // stylistic choice: `Value`'s equality goes through its ordering, which
        // ranks `I64` and `U64` together and compares them numerically. So
        // `Value::U64(10) == Value::I64(10)` is true, and an `assert_eq!` here
        // would pass against a parser that ignored the column's type entirely.
        // A mutation test found exactly that.
        let by_u64 = literal(&parse_constant("id = 10").unwrap().lower(&caller()));
        let by_i64 = literal(&parse_constant("size = 10").unwrap().lower(&caller()));
        let by_f64 = literal(&parse_constant("ratio = 10").unwrap().lower(&caller()));
        assert!(matches!(by_u64, Value::U64(10)), "{by_u64:?}");
        assert!(matches!(by_i64, Value::I64(10)), "{by_i64:?}");
        assert!(matches!(by_f64, Value::F64(x) if x == 10.0), "{by_f64:?}");
    }

    #[test]
    fn a_literal_no_i64_can_hold_survives_against_a_u64_column() {
        // Where the variant stops being cosmetic. `u64::MAX` has no `i64`
        // spelling, so a parser that read every integer as an `i64` cannot
        // express this predicate at all — and the row it is about is the one
        // an off-by-one in the codec would land on.
        let expression = parse_constant("id = 18446744073709551615")
            .unwrap()
            .lower(&caller());
        assert!(matches!(literal(&expression), Value::U64(u64::MAX)));

        // And the same text against a signed column is refused rather than
        // wrapped.
        let error = parse_constant("size = 18446744073709551615").unwrap_err();
        assert!(error.message.contains("not an i64"), "{}", error.message);
    }

    #[test]
    fn a_negative_number_is_refused_for_an_unsigned_column() {
        let error = parse_constant("id = -1").unwrap_err();
        assert!(error.message.contains("not a u64"), "{}", error.message);
        assert_eq!(
            parse_constant("size = -1").unwrap().lower(&caller()),
            Expr::Compare {
                column: Ordinal(2),
                op: CmpOp::Eq,
                value: Value::I64(-1)
            }
        );
    }

    #[test]
    fn precedence_is_or_over_and_over_not() {
        let parsed = parse_constant("size > 1 OR size < 0 AND live = true").unwrap();
        match parsed {
            Pred::Or(parts) => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(parts[1], Pred::And(_)));
            }
            other => panic!("expected an OR at the top, got {other:?}"),
        }
    }

    #[test]
    fn parentheses_override_precedence() {
        let parsed = parse_constant("(size > 1 OR size < 0) AND live = true").unwrap();
        assert!(matches!(parsed, Pred::And(_)), "{parsed:?}");
    }

    #[test]
    fn every_expr_variant_is_reachable() {
        let cases = [
            ("true", "True"),
            ("false", "False"),
            ("size > 1 AND size < 9", "And"),
            ("size > 1 OR size < 9", "Or"),
            ("NOT (size > 1)", "Not"),
            ("size >= 1", "Compare"),
            ("id = owner", "CompareColumns"),
            ("note IS NULL", "IsNull"),
            ("kind LIKE 'a%'", "Like"),
            ("kind ~ '^a'", "Matches"),
            ("size IN (1, 2)", "In"),
        ];
        for (source, expected) in cases {
            let parsed =
                parse_constant(source).unwrap_or_else(|e| panic!("{source}: {}", e.render(source)));
            let lowered = parsed.lower(&caller());
            let name = format!("{lowered:?}");
            assert!(
                name.starts_with(expected) || name.contains(expected),
                "{source} lowered to {name}, expected {expected}"
            );
        }
    }

    #[test]
    fn comparing_two_columns_of_different_types_is_refused() {
        let error = parse_constant("size = id").unwrap_err();
        assert!(
            error.message.contains("orders by type"),
            "{}",
            error.message
        );
    }

    #[test]
    fn comparing_two_columns_of_the_same_type_is_a_column_comparison() {
        assert_eq!(
            parse_constant("id = owner").unwrap().lower(&caller()),
            Expr::CompareColumns {
                left: Ordinal(0),
                op: CmpOp::Eq,
                right: Ordinal(5)
            }
        );
    }

    #[test]
    fn a_placeholder_is_refused_where_there_is_no_caller() {
        let error = parse_constant("owner = :principal").unwrap_err();
        assert!(
            error.message.contains("security.policies"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_placeholder_lowers_to_the_callers_own_value() {
        let parsed = parse_policy("owner = :principal").unwrap();
        assert!(!parsed.is_constant());
        assert_eq!(
            parsed.lower(&caller()),
            Expr::Compare {
                column: Ordinal(5),
                op: CmpOp::Eq,
                value: Value::U64(7)
            }
        );
    }

    #[test]
    fn a_tenant_placeholder_with_no_tenant_admits_no_row() {
        let parsed = parse_policy("owner = :tenant").unwrap();
        let anonymous = SecurityContext::new(Principal::new(Value::U64(7)));
        let lowered = parsed.lower(&anonymous);
        assert_eq!(
            lowered,
            Expr::Compare {
                column: Ordinal(5),
                op: CmpOp::Eq,
                value: Value::Null
            }
        );
        // The point is the behaviour, not the shape: no row is admitted, and
        // negating it does not rescue one either.
        let row = slate_schema::Row::new(vec![
            Value::U64(1),
            Value::Str("a".into()),
            Value::I64(1),
            Value::F64(1.0),
            Value::Bool(true),
            Value::U64(7),
            Value::Null,
        ]);
        assert!(!lowered.admits(&row));
        assert!(!Expr::Not(Box::new(lowered)).admits(&row));
    }

    #[test]
    fn an_unknown_placeholder_names_the_two_that_exist() {
        let error = parse_policy("owner = :user").unwrap_err();
        assert!(error.message.contains(":principal"), "{}", error.message);
    }

    #[test]
    fn an_unknown_column_lists_the_columns_there_are() {
        let error = parse_constant("nope = 1").unwrap_err();
        assert!(error.message.contains("`kind`"), "{}", error.message);
        assert!(error.message.contains("`size`"), "{}", error.message);
    }

    #[test]
    fn comparing_to_null_is_refused_and_points_at_is_null() {
        let error = parse_constant("note = null").unwrap_err();
        assert!(error.message.contains("IS NULL"), "{}", error.message);
    }

    #[test]
    fn a_regex_that_does_not_compile_is_refused_at_parse_time() {
        let error = parse_constant("kind ~ '('").unwrap_err();
        assert!(
            error.message.contains("not a valid regular expression"),
            "{}",
            error.message
        );
    }

    #[test]
    fn trailing_tokens_are_refused_rather_than_ignored() {
        let error = parse_constant("size > 1 size > 2").unwrap_err();
        assert!(error.message.contains("left over"), "{}", error.message);
    }

    #[test]
    fn not_in_and_not_like_negate() {
        assert!(matches!(
            parse_constant("kind NOT LIKE 'a%'").unwrap(),
            Pred::Like { negated: true, .. }
        ));
        assert!(matches!(
            parse_constant("size NOT IN (1)").unwrap(),
            Pred::In { negated: true, .. }
        ));
    }

    #[test]
    fn a_constant_predicate_says_it_is_constant() {
        assert!(
            parse_constant("size > 1 AND kind = 'a'")
                .unwrap()
                .is_constant()
        );
        assert!(
            !parse_policy("size > 1 AND owner = :principal")
                .unwrap()
                .is_constant()
        );
    }
}
