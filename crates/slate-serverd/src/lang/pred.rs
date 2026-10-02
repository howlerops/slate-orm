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
//!              | '[' [ element ( ',' element )* ] ']'
//! element     := string | number | 'TRUE' | 'FALSE'
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
                    "expected a comparison after the column, found {}.{}",
                    token.kind.describe(),
                    hint(&token.kind)
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
        // A word immediately followed by `(` is a function call, and the
        // refusal has to say so rather than report the function's name as a
        // missing column.
        //
        // `count(id) > 1` answered ``there is no column `count` here; this
        // table has `id`, `kind` and `size``` — true, and it sends a reader
        // looking for a column to add. That is the shape
        // `ledger/2026-09-29-the-same-wrong-sentence-twice-in-one-file.md`
        // recorded for `matches`, met again: the message names what it found
        // and not what is wrong. Checked here rather than in
        // `resolve_column`, so that a function sharing a column's name —
        // `size(x)` — is refused as a call rather than silently parsed as the
        // column with a stray `(` after it.
        if matches!(self.peek().map(|t| &t.kind), Some(Kind::Punct("("))) {
            return Err(LangError::new(
                at,
                format!(
                    "`{name}(…)` is a function call, and a predicate has no functions. \
                     A predicate compares a column with a literal, a placeholder or \
                     another column; computed values are declared on the query, not here."
                ),
            ));
        }
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

    /// `[ 'a', 'b' ]` — an array literal, opposite an array column.
    ///
    /// The opening bracket is already consumed. The *column* decides the
    /// element type, exactly as it decides a scalar literal's type: `[1, 2]`
    /// is a list of `u64` opposite `array<u64>` and of `i64` opposite
    /// `array<i64>`, and neither needs saying in the text. Guessing from the
    /// digits instead would make `tags = [7]` mean a different thing from
    /// `tags = [-7]` in the same table, which is the argument
    /// `literal_from_number` already makes one level down.
    ///
    /// **No nesting**, because the schema has none: an element type is a
    /// `ValueType` and cannot name an array's own element type, so
    /// `array<array<str>>` is not a column that exists. A `[` inside the list
    /// is therefore refused with that reason rather than parsed into
    /// something the schema could not hold.
    ///
    /// An empty `[]` is accepted and is *not* null: `docs/arrays.md` makes
    /// that distinction load-bearing in the kernel, and a surface syntax that
    /// could not write one would leave a value nothing can express.
    fn array_literal(
        &mut self,
        column: Ordinal,
        declared: ValueType,
        open_at: usize,
    ) -> LangResult<Value> {
        if declared != ValueType::Array {
            return Err(LangError::new(
                open_at,
                format!("`[` starts a list and the column opposite it is {declared}"),
            ));
        }
        let Some(element) = self.scope.element_type(column) else {
            return Err(LangError::new(
                open_at,
                "the column on the left is an array with no declared element type",
            ));
        };

        let mut elements = Vec::new();
        loop {
            let at = self.at();
            let Some(token) = self.advance().cloned() else {
                return Err(LangError::new(open_at, "this `[` is never closed"));
            };
            match token.kind {
                Kind::Punct("]") if elements.is_empty() => return Ok(Value::Array(elements)),
                Kind::Punct("]") => {
                    return Err(LangError::new(at, "a list cannot end with a comma"));
                }
                Kind::Punct("[") => {
                    return Err(LangError::new(
                        at,
                        "a list inside a list has no column type to be: an element type is a single type and cannot itself name an element type, so there is no array-of-arrays column for this to compare with",
                    ));
                }
                // A minus sign is its own token, so a negative element is two.
                // Handled here rather than in `array_element` because that
                // takes one token by design — and `operand` does the same
                // two-token dance one level up, for the same lexer reason.
                // Found by a test asserting `sizes = [1, -2]`, which is an
                // ordinary thing to write and did not parse.
                Kind::Punct("-") => {
                    let next_at = self.at();
                    match self.advance().map(|t| t.kind.clone()) {
                        Some(Kind::Number(text)) => {
                            elements.push(literal_from_number(&format!("-{text}"), element, at)?);
                        }
                        _ => return Err(LangError::new(next_at, "expected a number after `-`")),
                    }
                }
                _ => elements.push(self.array_element(&token.kind, element, at)?),
            }
            let at = self.at();
            match self.advance().map(|t| t.kind.clone()) {
                Some(Kind::Punct("]")) => return Ok(Value::Array(elements)),
                Some(Kind::Punct(",")) => {}
                Some(other) => {
                    return Err(LangError::new(
                        at,
                        format!("expected `,` or `]` in a list, found {}", other.describe()),
                    ));
                }
                None => return Err(LangError::new(open_at, "this `[` is never closed")),
            }
        }
    }

    /// One element of an array literal, at the element type the column gives.
    ///
    /// Deliberately not a call back into [`Parser::operand`]. An element is a
    /// literal and nothing else: a column reference inside a list would have
    /// to mean "this row's other column, as one element", and a `:principal`
    /// would make the *literal* caller-dependent rather than the comparison.
    /// Both are expressible and neither has a meaning anybody asked for, so
    /// they are refused by not being parsed rather than by a special case.
    fn array_element(&self, kind: &Kind, element: ValueType, at: usize) -> LangResult<Value> {
        match kind {
            Kind::Str(text) => literal_from_string(text, element, at),
            Kind::Number(text) => literal_from_number(text, element, at),
            Kind::Word(word)
                if word.eq_ignore_ascii_case("true") || word.eq_ignore_ascii_case("false") =>
            {
                if element == ValueType::Bool {
                    Ok(Value::Bool(word.eq_ignore_ascii_case("true")))
                } else {
                    Err(LangError::new(
                        at,
                        format!("`{word}` is a bool and the list holds {element}"),
                    ))
                }
            }
            // A null element is refused by `Row::validate` on every write, so
            // a predicate that could name one would be a predicate no stored
            // row can satisfy. `docs/arrays.md` records that refusal as the
            // reversible half of an open question; this keeps the two ends
            // saying the same thing.
            Kind::Word(word) if word.eq_ignore_ascii_case("null") => Err(LangError::new(
                at,
                "a list cannot hold NULL; an array column refuses a null element on every write, so no stored row could match",
            )),
            other => Err(LangError::new(
                at,
                format!("expected a literal in a list, found {}", other.describe()),
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
            Kind::Punct("[") => self
                .array_literal(column, declared, at)
                .map(Operand::Literal),
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

/// Word spellings of a regular-expression test that this grammar does not have.
///
/// `ledger/2026-09-19-the-regex-hole-that-was-not-there.md` is entirely about
/// one of these. A session wrote `title matches '^.{1,80}$'`, read
/// ``expected a comparison after the column, found `matches` `` as "there is no
/// regex here", and published a design note whose headline finding was that
/// the rule language could not express a length bound. It could; `~` had
/// parsed since the last ClickBench query. The message was accurate and the
/// reader was wrong, which is the failure a hint exists for.
///
/// Spellings rather than an alias, which that entry rejected and this does not
/// revisit: two ways to write one operator is a grammar with a synonym in it.
/// A message that names the one spelling costs a reader nothing to learn.
const REACHED_FOR: &[&str] = &["matches", "match", "regex", "regexp", "rlike", "similar"];

/// Word spellings of a *range* test this grammar does not have.
///
/// The same shape as `REACHED_FOR`, found by the review
/// `ledger/2026-10-01-the-other-messages-that-name-what-they-found.md` ran
/// over this file's other refusals: `size BETWEEN 1 AND 5` answered
/// ``found `BETWEEN`. The word-spelled tests are `LIKE`, `ILIKE`, `IN` and
/// `IS NULL``` — a correct sentence from which a reader has to work out that
/// a range is two comparisons. `BETWEEN` is in every dialect this grammar's
/// users come from and is the second-commonest thing to reach for after a
/// regular expression.
///
/// `NOT BETWEEN` is not listed: it reaches `NOT`'s own refusal first, which
/// already names what follows a `NOT`.
const RANGE_WORDS: &[&str] = &["between"];

/// What to add after "found `x`", or nothing.
///
/// Only a *word* earns a hint. A stray `(` or a number is a different mistake
/// and a suggestion about regular expressions would be noise in the middle of
/// it.
fn hint(kind: &Kind) -> String {
    let Kind::Word(word) = kind else {
        return String::new();
    };
    let lower = word.to_ascii_lowercase();
    if REACHED_FOR.contains(&lower.as_str()) {
        return " A regular expression is spelt `~` here, as in Postgres \
                 — `~*` ignores case, and `!~` and `!~*` negate."
            .to_owned();
    }
    if RANGE_WORDS.contains(&lower.as_str()) {
        return " A range is two comparisons here, joined with `AND`: \
                 `size >= 1 AND size <= 5`."
            .to_owned();
    }
    " The word-spelled tests are `LIKE`, `ILIKE`, `IN` and `IS NULL`; every \
      other operator is punctuation."
        .to_owned()
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
            .array_column("tags", ValueType::Str)
            .array_column("sizes", ValueType::I64)
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
    fn a_word_where_a_comparison_belongs_names_the_spelling_that_exists() {
        // The exact input from `the-regex-hole-that-was-not-there`, whose
        // whole cost was a message that named what it found and not what it
        // wanted. `~` is the answer and the error now says so.
        let error = parse_constant("kind matches '^a'").unwrap_err();
        assert!(error.message.contains("`matches`"), "{}", error.message);
        assert!(error.message.contains("`~`"), "{}", error.message);
        // Case does not change the guess: a configuration file may shout.
        let shouted = parse_constant("kind MATCHES '^a'").unwrap_err();
        assert!(shouted.message.contains("`~`"), "{}", shouted.message);
    }

    #[test]
    fn a_word_that_is_not_a_regex_guess_names_the_word_operators() {
        // The generic arm, and the assertion that it is a *different* answer:
        // a hint that said "did you mean `~`?" to every unknown word would be
        // wrong most of the time, and a test asserting only that some hint
        // appeared could not tell the two apart.
        //
        // The example was `between` until 2026-10-01, when `between` got a
        // roster of its own and this test went red — which is the right
        // failure and worth leaving a note about: the generic arm's example
        // has to be a word *no* roster claims, and any word that earns a
        // hint later will take this test with it.
        let error = parse_constant("kind resembles 1").unwrap_err();
        assert!(error.message.contains("`LIKE`"), "{}", error.message);
        assert!(!error.message.contains("`~`"), "{}", error.message);
        assert!(
            !error.message.contains("two comparisons"),
            "{}",
            error.message
        );
    }

    #[test]
    fn punctuation_where_a_comparison_belongs_gets_no_hint() {
        // A stray bracket is a different mistake, and advice about operator
        // spellings in the middle of it is noise.
        let error = parse_constant("kind ) 1").unwrap_err();
        assert!(error.message.contains("`)`"), "{}", error.message);
        assert!(!error.message.contains("`LIKE`"), "{}", error.message);
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

    // --- array literals -----------------------------------------------------

    #[test]
    fn an_array_literal_takes_the_element_type_from_the_column() {
        // The same text, two columns, two different element variants — the
        // array-level twin of `a_literal_takes_the_type_of_the_column_opposite
        // _it`, and asserted with `matches!` for the same reason: `Value`'s
        // equality ranks `I64` and `U64` together, so an `assert_eq!` would
        // pass against a parser that ignored the element type.
        let strings = literal(
            &parse_constant("tags = ['a', 'b']")
                .unwrap()
                .lower(&caller()),
        );
        let Value::Array(elements) = &strings else {
            panic!("expected an array, got {strings:?}");
        };
        assert!(
            matches!(elements.as_slice(), [Value::Str(a), Value::Str(b)] if a == "a" && b == "b"),
            "{elements:?}"
        );

        let numbers = literal(&parse_constant("sizes = [1, -2]").unwrap().lower(&caller()));
        let Value::Array(elements) = &numbers else {
            panic!("expected an array, got {numbers:?}");
        };
        assert!(
            matches!(elements.as_slice(), [Value::I64(1), Value::I64(-2)]),
            "{elements:?}"
        );
    }

    #[test]
    fn an_empty_array_literal_is_a_value_and_not_a_null() {
        // `docs/arrays.md` makes empty-versus-null load-bearing in the kernel,
        // and a surface syntax that could not write `[]` would leave a stored
        // value nothing can name.
        let empty = literal(&parse_constant("tags = []").unwrap().lower(&caller()));
        assert_eq!(empty, Value::Array(vec![]));
        assert!(!empty.is_null(), "an empty list parsed as null");
    }

    #[test]
    fn an_array_literal_orders_as_well_as_equals() {
        // Ordering is the other half of what the kernel supports on an array,
        // so the parser has to reach it. Element-wise with a shorter prefix
        // first, which is the codec's rule.
        let below = parse_constant("tags < ['b']").unwrap().lower(&caller());
        assert!(
            matches!(&below, Expr::Compare { op: CmpOp::Lt, .. }),
            "{below:?}"
        );
    }

    /// Every refusal in this file was read for the weakness
    /// `ledger/2026-09-29-the-same-wrong-sentence-twice-in-one-file.md`
    /// recorded in the regex one: a message that names *what it found* and
    /// not what the reader wanted, so a correct sentence sends them the
    /// wrong way. Three more had it, and each pairs here with the words its
    /// refusal now has to contain.
    ///
    /// Asserted on the words rather than on `is_err()`, because every one of
    /// these was already an error before the change — the whole finding is
    /// that the error said the wrong thing.
    #[test]
    fn the_three_refusals_that_named_what_they_found() {
        for (source, expected) in [
            // `size BETWEEN 1 AND 5` said the word-spelled tests are `LIKE`,
            // `ILIKE`, `IN` and `IS NULL`. True, and it leaves the reader to
            // work out that a range is two comparisons.
            ("size BETWEEN 1 AND 5", "A range is two comparisons here"),
            // `count(id) > 1` said ``there is no column `count` here``,
            // which sends a reader looking for a column to add. The sharpest
            // of the three: the message is actively misleading, where the
            // regex one was merely unhelpful.
            ("count(id) > 1", "is a function call"),
            // And a function sharing a column's name, which is why this is
            // checked before resolution rather than after it fails.
            ("size(id) > 1", "is a function call"),
            // `docs.kind = 'a'` said ``` `.` cannot appear in an
            // expression ```, with no hint that a predicate here is scoped
            // to one table and names are therefore unqualified.
            ("docs.kind = 'a'", "scoped to one table"),
        ] {
            let error = parse_constant(source).unwrap_err();
            let text = error.render(source);
            assert!(text.contains(expected), "{source}\n{text}");
        }
    }

    /// And the hints stay apart: a regex spelling must not get the range
    /// sentence, nor a range word the regex one.
    ///
    /// Written because both live in one `hint` and a `||` between the two
    /// rosters would pass every case above.
    #[test]
    fn a_range_word_and_a_regex_word_get_different_hints() {
        let regex = parse_constant("kind matches 'a'").unwrap_err();
        let regex = regex.render("kind matches 'a'");
        assert!(regex.contains("spelt `~`"), "{regex}");
        assert!(!regex.contains("two comparisons"), "{regex}");

        let range = parse_constant("size BETWEEN 1 AND 5").unwrap_err();
        let range = range.render("size BETWEEN 1 AND 5");
        assert!(range.contains("two comparisons"), "{range}");
        assert!(!range.contains("spelt `~`"), "{range}");
    }

    /// A grouping paren after a *comparison* is still a grouping paren.
    ///
    /// The function-call refusal fires on a word followed by `(` in column
    /// position, and the control that it has not swallowed the grammar's
    /// only legitimate `(`.
    #[test]
    fn a_grouping_paren_is_not_a_function_call() {
        parse_constant("kind = 'a' AND (size > 1 OR size < 0)").expect("parses");
    }

    #[test]
    fn an_array_literal_opposite_a_scalar_column_names_both_sides() {
        let error = parse_constant("kind = ['a']").unwrap_err();
        let text = error.render("kind = ['a']");
        assert!(text.contains("list"), "{text}");
        assert!(text.contains("str"), "{text}");
    }

    #[test]
    fn an_element_of_the_wrong_type_is_refused() {
        // The element type is the column's, so a quoted string in a list of
        // `i64` is the same mistake as a quoted string opposite an `i64`
        // column, and reaches the same converter.
        let error = parse_constant("sizes = ['a']").unwrap_err();
        let text = error.render("sizes = ['a']");
        assert!(text.contains("i64"), "{text}");
    }

    #[test]
    fn a_list_inside_a_list_is_refused_with_the_reason() {
        // Nesting is expressible in the *syntax* and impossible in the schema:
        // an element type is a single `ValueType` and cannot itself name one,
        // so there is no column this could compare with.
        let source = "tags = [['a']]";
        let error = parse_constant(source).unwrap_err();
        let text = error.render(source);
        assert!(text.contains("element type"), "{text}");
    }

    #[test]
    fn a_null_element_is_refused_because_no_stored_row_could_match() {
        // `Row::validate` refuses a null element on every write, so a
        // predicate naming one would select nothing, forever, silently.
        let source = "tags = ['a', NULL]";
        let error = parse_constant(source).unwrap_err();
        let text = error.render(source);
        assert!(text.contains("NULL"), "{text}");
    }

    #[test]
    fn a_malformed_list_says_which_way_it_is_malformed() {
        // Each case pairs with the words its own refusal has to contain. The
        // first version of this test asserted only that *an* error came back,
        // and mutation testing showed why that is worthless here: dropping the
        // separator check entirely still produces an error, because the stray
        // element is eaten as a separator and the closing `]` then looks like
        // a trailing comma. Same refusal count, wrong sentence, test green.
        for (source, expected) in [
            ("tags = ['a'", "never closed"),
            ("tags = ['a',", "never closed"),
            ("tags = ['a',]", "end with a comma"),
            ("tags = ['a' 'b']", "expected `,` or `]`"),
        ] {
            let Err(error) = parse_constant(source) else {
                panic!("`{source}` parsed");
            };
            let text = error.render(source);
            assert!(
                text.contains(expected),
                "`{source}` should say {expected:?}, said: {text}"
            );
        }
    }

    #[test]
    fn a_column_reference_inside_a_list_is_refused_rather_than_resolved() {
        // An element is a literal and nothing else. `kind` here would have to
        // mean "this row's `kind`, as one element", which is expressible and
        // has no meaning anybody asked for — so it is refused by not being
        // parsed rather than by a special case that has to be kept correct.
        let source = "tags = [kind]";
        let error = parse_constant(source).unwrap_err();
        let text = error.render(source);
        assert!(text.contains("literal"), "{text}");
    }

    #[test]
    fn a_placeholder_inside_a_list_is_refused_even_in_a_policy() {
        // A policy may say `owner = :principal`; it may not say
        // `tags = [:principal]`, which would make the *literal* vary by caller
        // rather than the comparison.
        let source = "tags = [:principal]";
        let error = parse_policy(source).unwrap_err();
        let text = error.render(source);
        assert!(text.contains("literal"), "{text}");
    }
}
