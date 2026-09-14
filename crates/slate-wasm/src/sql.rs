//! A small SQL front end over the kernel's query spec.
//!
//! # What this is, and what it is not
//!
//! **slate has no SQL.** The kernel takes a [`Query`](slate_kernel::Query);
//! the three SDKs and the gRPC protocol take a structured spec. Nothing in
//! this file changes that, and the workbench says so on screen.
//!
//! What it is: a parser from a `SELECT` subset onto [`QuerySpec`] and
//! [`JoinSpec`] — *the same spec types the panel already built from
//! dropdowns, and the same ones the SDKs send on the wire*. The spec it
//! produces is shown beside the results, so typing SQL teaches the real API
//! rather than hiding it.
//!
//! That choice is the whole design. A parser that built
//! [`Query`](slate_kernel::Query) directly would be a second way to reach the
//! executor, and the two would drift: a filter that the dropdowns lower one
//! way and the parser lowers another is a bug nobody can see. Going through
//! the spec means SQL adds **no execution path at all** — it is a different
//! way to write a value that `build` already knew how to lower, which is why
//! the round-trip property in `tests/sql.rs` is worth what it is.
//!
//! # The grammar, in full
//!
//! ```text
//! SELECT  <* | expr-list> FROM <table>
//!         [ JOIN <table> ON <col> = <col> ]
//!         [ WHERE <cond> (AND <cond>)* ]
//!         [ GROUP BY <col> ]
//!         [ ORDER BY <col> [ASC|DESC] (, ...)* ]
//!         [ LIMIT <int> ] [ OFFSET <int> ]
//! INSERT  INTO <table> VALUES ( <literal>, ... )
//! UPDATE  <table> SET <col> = <literal> (, ...)* WHERE <pk> = <literal>
//! DELETE  FROM <table> WHERE <pk> = <literal>
//! ```
//!
//! `<cond>` is `col <op> literal`, with `op` one of `= != <> < <= > >=`,
//! `LIKE`, `ILIKE` or `~` (a regular expression). Statements may be separated
//! by `;`.
//!
//! Everything outside that grammar is refused with the position and what was
//! expected. There is no silent subset: `SELECT ... WHERE a = 1 OR b = 2`
//! does not quietly become an `AND`, it is rejected, because the spec has no
//! disjunction to lower it onto and a wrong answer is worse than a refusal.
//!
//! # Why hand-written
//!
//! A parser generator or `sqlparser` would handle far more grammar than the
//! spec can express, which is exactly the wrong shape: every construct it
//! accepted and the spec could not represent would become an error *after*
//! parsing, phrased in terms of an AST the reader never wrote. A hand-written
//! recursive-descent parser over this grammar refuses at the token that is
//! wrong, and the whole thing is smaller than the dependency's changelog.

use crate::{AggregateSpec, FilterSpec, JoinSpec, QuerySpec, SortSpec};
use slate_schema::TableDef;

/// A parsed statement, already lowered onto the spec types.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// A single-table read.
    Select(QuerySpec),
    /// The fixture's one join, grouped or not.
    Join(JoinSpec),
    /// One row, one value per column, as strings for [`crate::literal`].
    Insert { table: String, values: Vec<String> },
    /// A read-modify-write: the columns named, by primary key.
    Update {
        table: String,
        key: String,
        /// `(ordinal, text)`, in the order written.
        set: Vec<(u32, String)>,
    },
    /// By primary key.
    Delete { table: String, key: String },
}

/// Where a refusal happened, so the workbench can say more than "syntax
/// error". The column is a byte offset into the statement, not into the
/// buffer: the editor runs statements one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlError {
    pub message: String,
    pub at: usize,
}

impl std::fmt::Display for SqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// The tables a statement may name, and their columns.
///
/// Taking this rather than reaching for `fixture::` keeps the parser honest
/// about names: a column is resolved to an ordinal *here*, against the real
/// [`TableDef`], so `SELECT nosuch FROM books` fails at parse time with the
/// column named, not later with an ordinal nobody typed.
#[derive(Debug)]
pub struct Schema<'a>(pub &'a [TableDef]);

impl Schema<'_> {
    fn table(&self, name: &str) -> Option<&TableDef> {
        self.0.iter().find(|t| t.name().eq_ignore_ascii_case(name))
    }

    fn ordinal(&self, table: &TableDef, column: &str) -> Option<u32> {
        table
            .columns()
            .iter()
            .position(|c| c.name().eq_ignore_ascii_case(column))
            .map(|i| u32::try_from(i).unwrap_or(0))
    }
}

// --- tokens ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// A bare word: keyword, table or column. Kept as written so errors can
    /// quote the reader's own spelling.
    Word(String),
    Number(String),
    /// A `'...'` string, with `''` already collapsed to one quote.
    Text(String),
    Symbol(String),
}

#[derive(Debug, Clone)]
struct Spanned {
    tok: Tok,
    at: usize,
}

/// Split a statement into tokens.
///
/// Scanned as `char`s with their byte offsets, never as bytes. The first
/// version indexed `text.as_bytes()` and slid `i` forward while
/// `bytes[i] as char` looked alphabetic — which for `café` accepts the first
/// byte of `é` (`Ã`, alphabetic) and stops on the second (`©`, not), leaving
/// `i` inside a character and panicking at the slice. A parser's input is by
/// definition whatever the reader typed, so that is a panic anybody could
/// reach from a text box.
///
/// Token text is built by pushing chars rather than slicing for the same
/// reason, which also means no slicing operations for the workspace's
/// `indexing_slicing` lint to object to.
///
/// Comments (`--` to end of line) are dropped here rather than in the parser,
/// so every rule below can assume it is looking at something meaningful.
fn lex(text: &str) -> Result<Vec<Spanned>, SqlError> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let at = |i: usize| chars.get(i).map(|(_, c)| *c);
    let offset = |i: usize| chars.get(i).map_or(text.len(), |(o, _)| *o);

    let mut out = Vec::new();
    let mut i = 0;
    while let Some(c) = at(i) {
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && at(i + 1) == Some('-') {
            while at(i).is_some_and(|c| c != '\n') {
                i += 1;
            }
        } else if c.is_alphabetic() || c == '_' {
            let start = offset(i);
            let mut word = String::new();
            while let Some(c) = at(i) {
                if c.is_alphanumeric() || c == '_' || c == '.' {
                    word.push(c);
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(Spanned {
                tok: Tok::Word(word),
                at: start,
            });
        } else if c.is_ascii_digit() || (c == '-' && at(i + 1).is_some_and(|c| c.is_ascii_digit()))
        {
            let start = offset(i);
            let mut number = String::from(c);
            i += 1;
            while let Some(c) = at(i) {
                if c.is_ascii_digit() || c == '.' {
                    number.push(c);
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(Spanned {
                tok: Tok::Number(number),
                at: start,
            });
        } else if c == '\'' {
            let start = offset(i);
            i += 1;
            let mut value = String::new();
            loop {
                match at(i) {
                    None => {
                        return Err(SqlError {
                            message: "a string is not closed".to_owned(),
                            at: start,
                        });
                    }
                    // `''` inside a string is one quote, the SQL spelling.
                    // Backslash escapes are deliberately *not* honoured:
                    // supporting both spellings means a literal containing a
                    // backslash silently means something else.
                    Some('\'') if at(i + 1) == Some('\'') => {
                        value.push('\'');
                        i += 2;
                    }
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(c) => {
                        value.push(c);
                        i += 1;
                    }
                }
            }
            out.push(Spanned {
                tok: Tok::Text(value),
                at: start,
            });
        } else {
            // Two-character operators first, or `<=` lexes as `<` then `=`.
            let start = offset(i);
            let pair = [Some(c), at(i + 1)];
            let two = match pair {
                [Some(a), Some(b)] => format!("{a}{b}"),
                _ => String::new(),
            };
            let symbol = if matches!(two.as_str(), "<=" | ">=" | "!=" | "<>") {
                i += 2;
                two
            } else {
                i += 1;
                c.to_string()
            };
            out.push(Spanned {
                tok: Tok::Symbol(symbol),
                at: start,
            });
        }
    }
    Ok(out)
}

/// Split a buffer on `;`, ignoring semicolons inside strings and comments.
///
/// Returned with each statement's offset, so an error in the third statement
/// can be reported against the buffer the editor holds.
#[must_use]
pub fn split(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut piece = String::new();
    let mut start = 0;
    let mut in_string = false;
    let mut in_comment = false;
    let mut previous = ' ';

    for (offset, c) in text.char_indices() {
        if piece.is_empty() {
            start = offset;
        }
        if in_comment {
            piece.push(c);
            if c == '\n' {
                in_comment = false;
            }
            continue;
        }
        match c {
            '\'' => {
                in_string = !in_string;
                piece.push(c);
            }
            '-' if !in_string && previous == '-' => {
                in_comment = true;
                piece.push(c);
            }
            ';' if !in_string => {
                if !piece.trim().is_empty() {
                    out.push((start, std::mem::take(&mut piece)));
                } else {
                    piece.clear();
                }
            }
            _ => piece.push(c),
        }
        previous = c;
    }
    if !piece.trim().is_empty() {
        out.push((start, piece));
    }
    out
}

// --- the parser -----------------------------------------------------------

struct Parser<'a> {
    toks: Vec<Spanned>,
    i: usize,
    schema: &'a Schema<'a>,
    /// Where the end of input is, for an error past the last token.
    end: usize,
}

/// Parse one statement.
///
/// # Errors
///
/// Returns the position and what was expected, for anything outside the
/// grammar in this module's documentation.
pub fn parse(text: &str, schema: &Schema<'_>) -> Result<Statement, SqlError> {
    let toks = lex(text)?;
    if toks.is_empty() {
        return Err(SqlError {
            message: "there is nothing to run".to_owned(),
            at: 0,
        });
    }
    let mut parser = Parser {
        end: text.len(),
        toks,
        i: 0,
        schema,
    };
    let statement = parser.statement()?;
    // Trailing tokens are an error rather than ignored. A reader who writes
    // `SELECT * FROM books WERE id = 1` has a typo, and silently returning the
    // whole table is the worst possible response to it.
    if let Some(extra) = parser.toks.get(parser.i) {
        let at = extra.at;
        return Err(SqlError {
            message: format!("unexpected {}", parser.describe(parser.i)),
            at,
        });
    }
    Ok(statement)
}

impl Parser<'_> {
    fn describe(&self, i: usize) -> String {
        match self.toks.get(i) {
            None => "the end of the statement".to_owned(),
            Some(s) => match &s.tok {
                Tok::Word(w) => format!("`{w}`"),
                Tok::Number(n) => format!("`{n}`"),
                Tok::Text(t) => format!("the string '{t}'"),
                Tok::Symbol(s) => format!("`{s}`"),
            },
        }
    }

    fn at(&self) -> usize {
        self.toks.get(self.i).map_or(self.end, |s| s.at)
    }

    fn peek_word(&self) -> Option<String> {
        match self.toks.get(self.i) {
            Some(Spanned {
                tok: Tok::Word(w), ..
            }) => Some(w.to_ascii_lowercase()),
            _ => None,
        }
    }

    /// Consume `word` if it is next. Keywords are case-insensitive.
    fn eat(&mut self, word: &str) -> bool {
        if self.peek_word().as_deref() == Some(word) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn eat_symbol(&mut self, symbol: &str) -> bool {
        if matches!(self.toks.get(self.i), Some(Spanned { tok: Tok::Symbol(s), .. }) if s == symbol)
        {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, word: &str) -> Result<(), SqlError> {
        if self.eat(word) {
            return Ok(());
        }
        Err(SqlError {
            message: format!(
                "expected `{}`, found {}",
                word.to_uppercase(),
                self.describe(self.i)
            ),
            at: self.at(),
        })
    }

    fn expect_symbol(&mut self, symbol: &str) -> Result<(), SqlError> {
        if self.eat_symbol(symbol) {
            return Ok(());
        }
        Err(SqlError {
            message: format!("expected `{symbol}`, found {}", self.describe(self.i)),
            at: self.at(),
        })
    }

    /// A bare word that is not a keyword position: a table or column name.
    fn name(&mut self) -> Result<String, SqlError> {
        match self.toks.get(self.i) {
            Some(Spanned {
                tok: Tok::Word(w), ..
            }) => {
                let w = w.clone();
                self.i += 1;
                Ok(w)
            }
            _ => Err(SqlError {
                message: format!("expected a name, found {}", self.describe(self.i)),
                at: self.at(),
            }),
        }
    }

    /// A literal, as text. Kept as text because [`crate::literal`] parses it
    /// against the column's declared type — the one place that conversion
    /// happens, and where a string handed to a `u64` column is refused rather
    /// than silently stored as a `Str` that no numeric predicate can match.
    fn literal(&mut self) -> Result<String, SqlError> {
        match self.toks.get(self.i) {
            Some(Spanned {
                tok: Tok::Number(n),
                ..
            }) => {
                let n = n.clone();
                self.i += 1;
                Ok(n)
            }
            Some(Spanned {
                tok: Tok::Text(t), ..
            }) => {
                let t = t.clone();
                self.i += 1;
                Ok(t)
            }
            // `true`/`false`/`null` arrive as words.
            Some(Spanned {
                tok: Tok::Word(w), ..
            }) if matches!(w.to_ascii_lowercase().as_str(), "true" | "false") => {
                let w = w.to_ascii_lowercase();
                self.i += 1;
                Ok(w)
            }
            _ => Err(SqlError {
                message: format!("expected a value, found {}", self.describe(self.i)),
                at: self.at(),
            }),
        }
    }

    fn table(&mut self) -> Result<TableDef, SqlError> {
        let at = self.at();
        let name = self.name()?;
        self.schema.table(&name).cloned().ok_or_else(|| SqlError {
            message: format!(
                "no table named `{name}` — this database has {}",
                self.schema
                    .0
                    .iter()
                    .map(TableDef::name)
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
            at,
        })
    }

    /// Resolve a column against one table, accepting a `table.column`
    /// qualifier when it names that table.
    fn column(&mut self, table: &TableDef) -> Result<u32, SqlError> {
        let at = self.at();
        let raw = self.name()?;
        self.resolve(&raw, table, at)
    }

    fn resolve(&self, raw: &str, table: &TableDef, at: usize) -> Result<u32, SqlError> {
        let bare = match raw.split_once('.') {
            Some((qualifier, rest)) => {
                if !qualifier.eq_ignore_ascii_case(table.name()) {
                    return Err(SqlError {
                        message: format!("`{raw}` is not a column of `{}`", table.name()),
                        at,
                    });
                }
                rest
            }
            None => raw,
        };
        self.schema.ordinal(table, bare).ok_or_else(|| SqlError {
            message: format!(
                "`{}` has no column `{bare}` — it has {}",
                table.name(),
                table
                    .columns()
                    .iter()
                    .map(|c| c.name().to_owned())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            at,
        })
    }

    fn statement(&mut self) -> Result<Statement, SqlError> {
        match self.peek_word().as_deref() {
            Some("select") => self.select(),
            Some("insert") => self.insert(),
            Some("update") => self.update(),
            Some("delete") => self.delete(),
            _ => Err(SqlError {
                message: format!(
                    "expected SELECT, INSERT, UPDATE or DELETE, found {}",
                    self.describe(self.i)
                ),
                at: self.at(),
            }),
        }
    }

    // --- SELECT -----------------------------------------------------------

    fn select(&mut self) -> Result<Statement, SqlError> {
        self.expect("select")?;

        // The select list is parsed *before* the table is known, because that
        // is the order SQL is written in. So it is captured raw and resolved
        // after FROM — which also gives a better error: an unknown column is
        // reported against the table the reader actually named.
        let mut list = Vec::new();
        let star = self.eat_symbol("*");
        if !star {
            loop {
                list.push(self.select_item()?);
                if !self.eat_symbol(",") {
                    break;
                }
            }
        }

        self.expect("from")?;
        let table = self.table()?;

        if self.eat("join") || self.eat("inner") {
            // `INNER JOIN` — the `INNER` is optional and consumed above.
            if self
                .toks
                .get(self.i)
                .is_some_and(|s| matches!(&s.tok, Tok::Word(w) if w.eq_ignore_ascii_case("join")))
            {
                self.i += 1;
            }
            return self.join_tail(&table, &list, star);
        }

        if star && list.is_empty() {
            // `SELECT *`: every column, which the spec spells as an empty
            // projection. Not the same as naming them all — the kernel takes
            // the empty list to mean "no projection", and a projection listing
            // every column is what makes an index-only scan impossible.
        }

        let mut spec = QuerySpec {
            table: table.name().to_owned(),
            ..QuerySpec::default()
        };
        // The select list is resolved *after* GROUP BY is known, because what
        // a bare column means depends on it: with no grouping it is a
        // projection, and with grouping it has to be a group key. Resolving
        // eagerly here is how the first version came to accept
        // `SELECT title, count(*) ... GROUP BY author_id` and quietly drop the
        // title.
        if self.eat("where") {
            spec.filters = self.conditions(&table)?;
        }
        if self.eat("group") {
            self.expect("by")?;
            loop {
                spec.group_by.push(self.column(&table)?);
                if !self.eat_symbol(",") {
                    break;
                }
            }
        }

        let grouping = !spec.group_by.is_empty();
        for item in &list {
            match item {
                SelectItem::Column { raw, at } => {
                    let ordinal = self.resolve(raw, &table, *at)?;
                    if grouping {
                        // Every SQL engine has this error. The spec cannot
                        // express "a column that is not in the group key", so
                        // it is refused rather than silently dropped from the
                        // output — which is the behaviour that makes a reader
                        // trust a number that is not what they asked for.
                        if !spec.group_by.contains(&ordinal) {
                            return Err(SqlError {
                                message: format!(
                                    "`{raw}` is neither a group key nor an aggregate; add it \
                                     to GROUP BY or wrap it in one"
                                ),
                                at: *at,
                            });
                        }
                    } else {
                        spec.columns.push(ordinal);
                    }
                }
                SelectItem::Aggregate { kind, argument, at } => {
                    if !grouping {
                        return Err(SqlError {
                            message: "an aggregate needs a GROUP BY — try \
                                      `SELECT pickup_zone, count(*) FROM trips GROUP BY \
                                      pickup_zone`"
                                .to_owned(),
                            at: *at,
                        });
                    }
                    spec.aggregates
                        .push(self.aggregate(kind, argument.as_deref(), &table, *at)?);
                }
            }
        }
        if grouping && spec.aggregates.is_empty() && !list.is_empty() {
            // `SELECT zone FROM trips GROUP BY zone` — the distinct keys. The
            // binding adds `count(*)` so the answer is not a bare column.
        }
        if self.eat("order") {
            self.expect("by")?;
            loop {
                // With a GROUP BY, ORDER BY orders the *groups*, and a group
                // is `[keys..., aggregates...]` — a space with its own
                // ordinals that has nothing to do with the table's.
                //
                // This clause used to be lowered onto the query either way,
                // which put the sort on the rows going *into* the grouping.
                // That is not a different way of saying the same thing: the
                // groups come back ordered by key regardless, so the reader's
                // ORDER BY was silently discarded and the sort was wasted
                // work. A test asserting the refusal is what found it.
                let at = self.at();
                let item = self.select_item()?;
                let column = if grouping {
                    self.group_ordinal(&item, &spec, &table, at)?
                } else {
                    match &item {
                        SelectItem::Column { raw, at } => self.resolve(raw, &table, *at)?,
                        SelectItem::Aggregate { at, .. } => {
                            return Err(SqlError {
                                message: "an aggregate in ORDER BY needs a GROUP BY".to_owned(),
                                at: *at,
                            });
                        }
                    }
                };
                let descending = if self.eat("desc") {
                    true
                } else {
                    self.eat("asc");
                    false
                };
                spec.sort.push(SortSpec { column, descending });
                if !self.eat_symbol(",") {
                    break;
                }
            }
        }
        if self.eat("limit") {
            spec.limit = Some(self.count("LIMIT")?);
        }
        if self.eat("offset") {
            spec.offset = self.count("OFFSET")?;
        }
        Ok(Statement::Select(spec))
    }

    /// Where an ORDER BY item sits in a group, which is `[keys, aggregates]`.
    ///
    /// Resolved against what the query already said rather than against the
    /// table: `ORDER BY count(*)` means "the aggregate I asked for", and
    /// ordering by an aggregate the select list does not compute would be
    /// ordering by a number that is not in the answer.
    fn group_ordinal(
        &self,
        item: &SelectItem,
        spec: &QuerySpec,
        table: &TableDef,
        at: usize,
    ) -> Result<u32, SqlError> {
        match item {
            SelectItem::Column { raw, at } => {
                let ordinal = self.resolve(raw, table, *at)?;
                spec.group_by
                    .iter()
                    .position(|k| *k == ordinal)
                    .map(|i| u32::try_from(i).unwrap_or(0))
                    .ok_or_else(|| SqlError {
                        message: format!(
                            "`{raw}` is not a group key, so the groups cannot be ordered by it"
                        ),
                        at: *at,
                    })
            }
            SelectItem::Aggregate { kind, argument, .. } => {
                let wanted = self.aggregate(kind, argument.as_deref(), table, at)?;
                let position = spec
                    .aggregates
                    .iter()
                    .position(|a| a.kind == wanted.kind && a.column == wanted.column)
                    .ok_or_else(|| SqlError {
                        message: "ORDER BY names an aggregate this query does not compute"
                            .to_owned(),
                        at,
                    })?;
                Ok(u32::try_from(spec.group_by.len() + position).unwrap_or(0))
            }
        }
    }

    /// `count(*)`, `avg(total)`, `count(distinct zone)`.
    fn aggregate(
        &self,
        kind: &str,
        argument: Option<&str>,
        table: &TableDef,
        at: usize,
    ) -> Result<AggregateSpec, SqlError> {
        let (kind, argument) = match (kind, argument) {
            ("count", None) => ("count".to_owned(), None),
            ("count", Some(name)) => {
                // `count(column)` and `count(*)` differ on nulls, and the
                // difference is not decoration: the kernel has both. Rather
                // than silently treat one as the other, take the column.
                ("count_column".to_owned(), Some(name))
            }
            (other, argument) => (other.to_owned(), argument),
        };
        let column = match argument {
            None => 0,
            Some(name) => self.resolve(name, table, at)?,
        };
        if !matches!(
            kind.as_str(),
            "count" | "count_column" | "min" | "max" | "sum" | "avg" | "count_distinct"
        ) {
            return Err(SqlError {
                message: format!(
                    "no such aggregate: `{kind}` — this has count, min, max, sum and avg"
                ),
                at,
            });
        }
        if kind != "count" && argument.is_none() {
            return Err(SqlError {
                message: format!("`{kind}` needs a column"),
                at,
            });
        }
        Ok(AggregateSpec { kind, column })
    }

    fn select_item(&mut self) -> Result<SelectItem, SqlError> {
        let at = self.at();
        let name = self.name()?;
        if self.eat_symbol("(") {
            // `count(*)`, `max(year)`, `count(distinct pickup_zone)`.
            let distinct = self.eat("distinct");
            let argument = if !distinct && self.eat_symbol("*") {
                None
            } else {
                Some(self.name()?)
            };
            if distinct {
                self.expect_symbol(")")?;
                return Ok(SelectItem::Aggregate {
                    kind: "count_distinct".to_owned(),
                    argument,
                    at,
                });
            }
            self.expect_symbol(")")?;
            return Ok(SelectItem::Aggregate {
                kind: name.to_ascii_lowercase(),
                argument,
                at,
            });
        }
        Ok(SelectItem::Column { raw: name, at })
    }

    fn count(&mut self, what: &str) -> Result<u64, SqlError> {
        let at = self.at();
        let text = self.literal()?;
        text.parse::<u64>().map_err(|_| SqlError {
            message: format!("{what} takes a whole number, found `{text}`"),
            at,
        })
    }

    fn conditions(&mut self, table: &TableDef) -> Result<Vec<FilterSpec>, SqlError> {
        let mut out = vec![self.condition(table)?];
        loop {
            if self.eat("and") {
                out.push(self.condition(table)?);
            } else if self.peek_word().as_deref() == Some("or") {
                return Err(SqlError {
                    message: "OR is not supported: the query spec ANDs its conditions, and \
                              lowering an OR onto it would answer a different question"
                        .to_owned(),
                    at: self.at(),
                });
            } else {
                return Ok(out);
            }
        }
    }

    fn condition(&mut self, table: &TableDef) -> Result<FilterSpec, SqlError> {
        let column = self.column(table)?;
        let at = self.at();
        let op = if self.eat_symbol("=") {
            "eq"
        } else if self.eat_symbol("!=") || self.eat_symbol("<>") {
            "ne"
        } else if self.eat_symbol("<=") {
            "le"
        } else if self.eat_symbol(">=") {
            "ge"
        } else if self.eat_symbol("<") {
            "lt"
        } else if self.eat_symbol(">") {
            "gt"
        } else if self.eat_symbol("~") {
            "matches"
        } else if self.eat("like") {
            "like"
        } else if self.eat("ilike") {
            "ilike"
        } else {
            return Err(SqlError {
                message: format!("expected a comparison, found {}", self.describe(self.i)),
                at,
            });
        };
        let value = self.literal()?;
        Ok(FilterSpec {
            column,
            op: op.to_owned(),
            value,
        })
    }

    // --- the join ---------------------------------------------------------

    /// `... FROM trips JOIN zones ON trips.pickup_zone = zones.id ...`
    ///
    /// Any pair of tables and any pair of columns, checked against the schema.
    /// The previous version hard-coded `authors JOIN books`, which was fine
    /// while that was the only join in the database and became a wall the
    /// moment the taxi zones arrived — a lookup table you cannot join is a
    /// list of names nobody can reach.
    ///
    /// What is still checked, because getting it wrong returns an empty result
    /// with no explanation: both sides of the `ON` must name a real column,
    /// and each must belong to a *different* one of the two tables.
    fn join_tail(
        &mut self,
        left: &TableDef,
        list: &[SelectItem],
        star: bool,
    ) -> Result<Statement, SqlError> {
        let right = self.table()?;
        if right.name().eq_ignore_ascii_case(left.name()) {
            return Err(SqlError {
                message: format!("`{}` cannot be joined to itself here", left.name()),
                at: self.at(),
            });
        }

        self.expect("on")?;
        let first_at = self.at();
        let first = self.name()?;
        self.expect_symbol("=")?;
        let second_at = self.at();
        let second = self.name()?;

        // Either order: `trips.pickup_zone = zones.id` and
        // `zones.id = trips.pickup_zone` are the same join.
        let (left_key, right_key) = match (
            self.resolve(&first, left, first_at),
            self.resolve(&second, &right, second_at),
        ) {
            (Ok(l), Ok(r)) => (l, r),
            _ => match (
                self.resolve(&second, left, second_at),
                self.resolve(&first, &right, first_at),
            ) {
                (Ok(l), Ok(r)) => (l, r),
                _ => {
                    return Err(SqlError {
                        message: format!(
                            "`{first} = {second}` does not name one column of `{}` and one of \
                             `{}`",
                            left.name(),
                            right.name()
                        ),
                        at: first_at,
                    });
                }
            },
        };

        let mut spec = JoinSpec {
            left: left.name().to_owned(),
            right: right.name().to_owned(),
            left_key,
            right_key,
            ..JoinSpec::default()
        };

        // WHERE is split by which table each column belongs to — the kernel
        // pushes each side's conditions into that side's own scan, which is
        // the difference between filtering 100,000 trips and filtering the
        // handful that survive. Sending them all to one side would still be
        // correct and would plan much worse.
        if self.eat("where") {
            loop {
                let at = self.at();
                let raw = self.name()?;
                let qualified_right = raw
                    .to_ascii_lowercase()
                    .starts_with(&format!("{}.", right.name().to_ascii_lowercase()));
                let on_left = !qualified_right && self.resolve(&raw, left, at).is_ok();
                let table = if on_left { left } else { &right };
                let column = self.resolve(&raw, table, at)?;
                // Rewind one token so `condition` reads the operator.
                self.i -= 1;
                let mut parsed = self.condition(table)?;
                parsed.column = column;
                if on_left {
                    spec.left_where.push(parsed);
                } else {
                    spec.right_where.push(parsed);
                }
                if !self.eat("and") {
                    break;
                }
            }
        }

        if self.eat("group") {
            self.expect("by")?;
            let at = self.at();
            let raw = self.name()?;
            let key = self.resolve(&raw, left, at).map_err(|_| SqlError {
                message: format!(
                    "GROUP BY takes a column of `{}` (the join's left side); `{raw}` is not one",
                    left.name()
                ),
                at,
            })?;
            spec.group_by = Some(key);
        }

        for item in list {
            match item {
                SelectItem::Aggregate { kind, argument, at } => {
                    let parsed = self.aggregate(kind, argument.as_deref(), &right, *at)?;
                    spec.aggregates.push(parsed);
                }
                SelectItem::Column { raw, at } => {
                    // Selecting a bare column beside a GROUP BY would be the
                    // "not in the group key" error every SQL engine has. The
                    // spec cannot express it, so it is refused rather than
                    // silently dropped from the output.
                    if spec.group_by.is_some() {
                        return Err(SqlError {
                            message: format!(
                                "`{raw}` is not the group key — a grouped query returns the \
                                 key and the aggregates"
                            ),
                            at: *at,
                        });
                    }
                }
            }
        }
        if spec.group_by.is_none() && !spec.aggregates.is_empty() {
            return Err(SqlError {
                message: "an aggregate needs a GROUP BY".to_owned(),
                at: self.at(),
            });
        }
        if !star && list.is_empty() {
            return Err(SqlError {
                message: "a join returns whole rows; write `SELECT *`".to_owned(),
                at: self.at(),
            });
        }

        if self.eat("order") {
            return Err(SqlError {
                message: "ORDER BY is not supported on a join yet".to_owned(),
                at: self.at(),
            });
        }
        if self.eat("limit") {
            spec.limit = Some(self.count("LIMIT")?);
        }
        Ok(Statement::Join(spec))
    }

    // --- writes -----------------------------------------------------------

    fn insert(&mut self) -> Result<Statement, SqlError> {
        self.expect("insert")?;
        self.expect("into")?;
        let table = self.table()?;
        // A column list would let a row arrive with holes, and the binding's
        // `insert` takes one value per column because the kernel's `Row` does.
        // Refusing is better than filling the gaps with a default this schema
        // has not declared.
        if self.eat_symbol("(") {
            return Err(SqlError {
                message: "INSERT takes every column in order, with no column list".to_owned(),
                at: self.at(),
            });
        }
        self.expect("values")?;
        self.expect_symbol("(")?;
        let mut values = Vec::new();
        loop {
            values.push(self.literal()?);
            if !self.eat_symbol(",") {
                break;
            }
        }
        self.expect_symbol(")")?;
        let wanted = table.columns().len();
        if values.len() != wanted {
            return Err(SqlError {
                message: format!(
                    "`{}` takes {wanted} values ({}), got {}",
                    table.name(),
                    table
                        .columns()
                        .iter()
                        .map(|c| c.name().to_owned())
                        .collect::<Vec<_>>()
                        .join(", "),
                    values.len()
                ),
                at: self.at(),
            });
        }
        Ok(Statement::Insert {
            table: table.name().to_owned(),
            values,
        })
    }

    fn update(&mut self) -> Result<Statement, SqlError> {
        self.expect("update")?;
        let table = self.table()?;
        self.expect("set")?;
        let mut set = Vec::new();
        loop {
            let column = self.column(&table)?;
            self.expect_symbol("=")?;
            set.push((column, self.literal()?));
            if !self.eat_symbol(",") {
                break;
            }
        }
        let key = self.by_primary_key(&table, "UPDATE")?;
        Ok(Statement::Update {
            table: table.name().to_owned(),
            key,
            set,
        })
    }

    fn delete(&mut self) -> Result<Statement, SqlError> {
        self.expect("delete")?;
        self.expect("from")?;
        let table = self.table()?;
        let key = self.by_primary_key(&table, "DELETE")?;
        Ok(Statement::Delete {
            table: table.name().to_owned(),
            key,
        })
    }

    /// `WHERE <pk> = <literal>`, and nothing else.
    ///
    /// Both writes go through the kernel's by-primary-key path. A predicate
    /// delete would be a scan plus a delete per row, which the binding does
    /// not offer — and quietly deleting more rows than the reader expected is
    /// the single worst thing this editor could do.
    fn by_primary_key(&mut self, table: &TableDef, what: &str) -> Result<String, SqlError> {
        self.expect("where")?;
        let at = self.at();
        let column = self.column(table)?;
        let key = table.primary_key();
        if key.len() != 1 || key.first().map(|o| o.0) != Some(column as usize) {
            return Err(SqlError {
                message: format!(
                    "{what} matches on the primary key only, which for `{}` is `{}`",
                    table.name(),
                    key.iter()
                        .filter_map(|o| table.column(*o).map(|c| c.name().to_owned()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                at,
            });
        }
        self.expect_symbol("=")?;
        let value = self.literal()?;
        if self.eat("and") {
            return Err(SqlError {
                message: format!("{what} takes the primary key alone"),
                at: self.at(),
            });
        }
        Ok(value)
    }
}

#[derive(Debug, Clone)]
enum SelectItem {
    Column {
        raw: String,
        at: usize,
    },
    Aggregate {
        kind: String,
        argument: Option<String>,
        at: usize,
    },
}
