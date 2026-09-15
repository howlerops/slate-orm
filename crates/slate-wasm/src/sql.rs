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
//! SELECT  <* | item-list> FROM <table>
//!         [ JOIN <table> ON <col> = <col> ]
//!         [ WHERE <cond> (AND <cond>)* ]
//!         [ GROUP BY <col> (, ...)* ]
//!         [ HAVING <group-cond> (AND <group-cond>)* ]
//!         [ ORDER BY <col | aggregate> [ASC|DESC] (, ...)* ]
//!         [ LIMIT <int> ] [ OFFSET <int> ]
//!
//!         -- on a join: one GROUP BY key, and ORDER BY needs it
//! INSERT  INTO <table> VALUES ( <literal>, ... )
//! UPDATE  <table> SET <col> = <literal> (, ...)* WHERE <pk> = <literal>
//! DELETE  FROM <table> WHERE <pk> = <literal>
//! ```
//!
//! `<cond>` is `col <op> literal`, with `op` one of `= != <> < <= > >=`,
//! `LIKE`, `ILIKE` or `~` (a regular expression). Statements may be separated
//! by `;`.
//!
//! An `<item>` is a column, an aggregate, or a **call**: `hour(pickup_time)`,
//! `round(distance)`. A call is a value computed per row and appended after
//! the table's own columns, so it can be a group key, a sort key or a HAVING
//! subject exactly as a column can. Writing the same call twice — once in the
//! select list, once in `GROUP BY` — names one computed column, not two.
//!
//! The calls are `hour`, `minute`, `second`, `year`, `month`, `day`,
//! `day_of_week`, `date` and `round`. There is no date *type*: a timestamp is
//! seconds since the epoch in an integer column, `day` is the day of the month
//! as `EXTRACT(DAY FROM t)` is in SQL, and `date` returns midnight of the day
//! as epoch seconds so that grouping by it orders chronologically.
//!
//! Every time function reads the timestamp in **UTC** unless it is given a
//! second argument: either a fixed offset, `hour(pickup_time, '-05:00')`, or
//! an IANA zone name, `hour(pickup_time, 'America/New_York')`. A name is
//! resolved through the kernel's transition table — a few kilobytes of sorted
//! integers, not the whole IANA database — so daylight saving is looked up at
//! each row's instant rather than guessed at. A name outside the table is
//! refused, and the refusal lists the ones that are in it.
//!
//! `hour(t)`, `hour(t, '-05:00')` and `hour(t, 'America/New_York')` are three
//! computed columns, because they are three questions: in New York the second
//! and third differ for a third of the year.
//!
//! A call works on a join as well, where it must be the group key and may read
//! either side: `SELECT hour(pickup_time), count(*) FROM trips JOIN zones ON
//! ... GROUP BY hour(pickup_time)`. On a join the computed column lands after
//! *both* tables' columns — the right table already owns the ordinals directly
//! after the left — which is why the kernel refuses a computed column declared
//! on a side.
//!
//! On a join an unqualified name is resolved against the left table first, so
//! a bare `id` over `trips JOIN zones` means `trips.id`. That is not refused —
//! refusing every ambiguous name would refuse `SELECT hour(pickup_time)` on
//! any schema where both tables happen to have one — but it is no longer
//! silent: the result carries a warning naming the side it chose and how to
//! say the other. See [`Parsed::warnings`].
//!
//! `ORDER BY` on a join needs a `GROUP BY`, and orders the **groups**: a group
//! is its key followed by its aggregates, so `ORDER BY count(*) DESC` is a
//! sort key on the group's second slot. An *ungrouped* join cannot be ordered
//! at all, and the refusal says why — `Join` has no sort field, because the
//! kernel orders groups and not joined rows.
//!
//! `<group-cond>` is the same, except the left side names a *group* — a group
//! key or one of the aggregates the select list computes — so
//! `HAVING count(*) > 100` is a condition on a number that does not exist
//! until every row has been read. That is the whole difference between it and
//! `WHERE`: a `WHERE` can become a scan bound and skip rows before they are
//! fetched, and a `HAVING` never can. `HAVING` without a `GROUP BY` is
//! refused rather than treated as a `WHERE`.
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

use crate::{AggregateSpec, ComputeSpec, FilterSpec, JoinSpec, QuerySpec, SortSpec};
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
    /// Things worth saying that are not errors. See [`Parsed::warnings`].
    warnings: Vec<String>,
}

/// One parsed statement, and anything worth telling the reader about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    /// The statement, lowered onto the spec types.
    pub statement: Statement,
    /// Warnings: true things about what was written that are not refusals.
    ///
    /// Only one kind so far, and it is the one this exists for. An unqualified
    /// name on a join resolves against the left table first, so on `trips JOIN
    /// zones` a bare `id` silently means `trips.id` — which is a reasonable
    /// rule and an unreasonable thing to do in silence when `zones.id` is the
    /// one the reader meant. The query still runs; the warning says which side
    /// was chosen and how to say the other.
    ///
    /// Warnings rather than a refusal, for the reason the resolution rule is
    /// left-first in the first place: refusing every ambiguous name would
    /// refuse `SELECT hour(pickup_time)` on any schema where both tables
    /// happen to have a `pickup_time`, and the reader who wrote it was not
    /// being ambiguous on purpose.
    ///
    /// Returned beside the statement rather than inside it, because a warning
    /// is about the *text* and the statement is what the text meant.
    pub warnings: Vec<String>,
}

/// Parse one statement.
///
/// # Errors
///
/// Returns the position and what was expected, for anything outside the
/// grammar in this module's documentation.
pub fn parse(text: &str, schema: &Schema<'_>) -> Result<Parsed, SqlError> {
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
        warnings: Vec::new(),
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
    Ok(Parsed {
        statement,
        warnings: parser.warnings,
    })
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

    /// The side and ordinal a name refers to, over a two-table join.
    ///
    /// Qualified wins: `zones.borough` names the right table even if `borough`
    /// would also resolve on the left. Unqualified tries the left first, which
    /// is what SQL does with an ambiguous name in every dialect that does not
    /// refuse it outright — and refusing would break `SELECT hour(pickup_time)`
    /// on a schema where both tables happen to have an `id`.
    ///
    /// It is no longer *silent* about that, though: a bare name that resolves
    /// on both sides adds a warning naming the side it chose and how to spell
    /// the other. The rule was documented here and nowhere the reader could
    /// see it, so `SELECT id FROM trips JOIN zones ON ...` answered about
    /// `trips.id` with nothing on screen to say so. `&mut self` rather than
    /// `&self` for exactly that reason.
    ///
    /// This is the whole of the joined-space fix at the parser. Every joined
    /// position used to be resolved against one *fixed* side: a computed
    /// column and the group key against the left, an aggregate against the
    /// right. That made "group by the hour and average the fare" over `trips
    /// JOIN zones` inexpressible, because both of those columns live on
    /// `trips` — the workbench example filtered on the right side and counted
    /// instead, and the note recording that called it a `JoinSpec` limitation.
    /// It was.
    fn resolve_side(
        &mut self,
        raw: &str,
        left: &TableDef,
        right: &TableDef,
        at: usize,
    ) -> Result<(u32, u32), SqlError> {
        if let Some((qualifier, _)) = raw.split_once('.') {
            if qualifier.eq_ignore_ascii_case(right.name()) {
                return Ok((1, self.resolve(raw, right, at)?));
            }
            return Ok((0, self.resolve(raw, left, at)?));
        }
        if let Ok(column) = self.resolve(raw, left, at) {
            // Both sides have it, and the left one won. Said once per name
            // rather than once per mention: `SELECT id ... GROUP BY id` names
            // the same column twice and a reader does not need telling twice.
            if self.resolve(raw, right, at).is_ok() {
                let warning = format!(
                    "`{raw}` is a column of both `{}` and `{}`; this read \
                     `{}.{raw}`. Qualify it to choose.",
                    left.name(),
                    right.name(),
                    left.name()
                );
                if !self.warnings.contains(&warning) {
                    self.warnings.push(warning);
                }
            }
            return Ok((0, column));
        }
        match self.resolve(raw, right, at) {
            Ok(column) => Ok((1, column)),
            Err(_) => Err(SqlError {
                message: format!(
                    "`{raw}` is not a column of `{}` or `{}`",
                    left.name(),
                    right.name()
                ),
                at,
            }),
        }
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
                // A select item rather than a column, so `GROUP BY
                // hour(pickup_time)` is expressible. The same call in the
                // select list has to land on the same ordinal, which is what
                // `value_ordinal`'s find-or-add is for — registering it twice
                // would group by two identical columns and return one group
                // per pair, which is the same answer with a duplicated column
                // and no error anywhere.
                let at = self.at();
                let item = self.select_item()?;
                let ordinal = self.value_ordinal(&item, &mut spec, &table, at, "GROUP BY")?;
                spec.group_by.push(ordinal);
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
                SelectItem::Call { at, .. } => {
                    let at = *at;
                    let ordinal = self.value_ordinal(item, &mut spec, &table, at, "SELECT")?;
                    if grouping {
                        // Same rule a bare column follows: with a grouping,
                        // every non-aggregate in the list must be a key.
                        if !spec.group_by.contains(&ordinal) {
                            return Err(SqlError {
                                message: "that is neither a group key nor an aggregate; add it \
                                          to GROUP BY or wrap it in one"
                                    .to_owned(),
                                at,
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
        if self.eat("having") {
            if !grouping {
                return Err(SqlError {
                    message: "HAVING needs a GROUP BY — it filters groups, and without one \
                              there are no groups to filter. Did you mean WHERE?"
                        .to_owned(),
                    at: self.at(),
                });
            }
            // Parsed here, after the select list has been resolved, because
            // `HAVING count(*) > 100` names an aggregate by what the query
            // *computes* — the same rule ORDER BY follows — and `spec.aggregates`
            // is not populated until the loop above has run.
            loop {
                spec.having.push(self.having_condition(&spec, &table)?);
                if self.eat("and") {
                    continue;
                }
                if self.peek_word().as_deref() == Some("or") {
                    return Err(SqlError {
                        message: "OR is not supported in HAVING, for the reason it is not \
                                  supported in WHERE"
                            .to_owned(),
                        at: self.at(),
                    });
                }
                break;
            }
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
                    self.group_ordinal(&item, &spec, &table, at, "ORDER BY")?
                } else {
                    match &item {
                        SelectItem::Aggregate { at, .. } => {
                            return Err(SqlError {
                                message: "an aggregate in ORDER BY needs a GROUP BY".to_owned(),
                                at: *at,
                            });
                        }
                        // A column or a call. `ORDER BY hour(pickup_time)`
                        // without a grouping sorts the rows by that value,
                        // which is what it says, and registers the
                        // computation if the select list did not.
                        other => self.value_ordinal(other, &mut spec, &table, at, "ORDER BY")?,
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

    /// The ordinal a column or a computed call denotes, registering the
    /// computation if it is new.
    ///
    /// Find-or-add on `(function, column)`, so the same call written twice —
    /// once in the select list, once in `GROUP BY` — is one computed column
    /// and one ordinal. Two entries would be two identical group keys: the
    /// same answer with a column repeated, no error, and nothing to notice.
    ///
    fn value_ordinal(
        &self,
        item: &SelectItem,
        spec: &mut QuerySpec,
        table: &TableDef,
        at: usize,
        clause: &str,
    ) -> Result<u32, SqlError> {
        match item {
            SelectItem::Column { raw, at } => self.resolve(raw, table, *at),
            SelectItem::Aggregate { kind, .. } => Err(SqlError {
                // An unknown name parses as an aggregate, because that is what
                // anything `word(...)` that is not a time function is. Saying
                // "an aggregate cannot be a group key" about `nosuch(x)` names
                // a category the reader never used, so check first.
                message: if AGGREGATES.contains(&kind.as_str()) {
                    format!("an aggregate cannot be used as a value in {clause}")
                } else {
                    format!(
                        "no such function: `{kind}` — this has {} and the aggregates {}",
                        TIME_FUNCTIONS.join(", "),
                        AGGREGATES.join(", ")
                    )
                },
                at,
            }),
            SelectItem::Call {
                function,
                argument,
                offset,
                zone,
                ..
            } => {
                let column = self.resolve(argument, table, at)?;
                let wanted = ComputeSpec {
                    function: function.clone(),
                    // One table, so one side. `input` distinguishes the two
                    // sides of a join and means nothing here.
                    input: 0,
                    column,
                    offset: *offset,
                    zone: zone.clone(),
                };
                let position = spec
                    .compute
                    .iter()
                    .position(|c| *c == wanted)
                    .unwrap_or_else(|| {
                        spec.compute.push(wanted);
                        spec.compute.len() - 1
                    });
                // Computed columns sit immediately after the table's own, the
                // same arithmetic `Query::computed` does. The binding checks
                // this against the schema; here it is the one place the
                // parser has to know the layout, and it is written down.
                Ok(u32::try_from(table.columns().len() + position).unwrap_or(0))
            }
        }
    }

    /// The joined-space ordinal a join's group key denotes, registering a
    /// computed column if it is one.
    ///
    /// The single-table twin is [`Self::value_ordinal`]; this one differs in
    /// where a computed column lands. On one table they sit after that table's
    /// columns; on a join they sit after *both*, because the right table
    /// already occupies the ordinals immediately after the left. Getting this
    /// wrong is not an error but a wrong answer — the group key would be the
    /// right table's first column — which is why the kernel now refuses a
    /// side's own computed column outright rather than letting it land there.
    ///
    /// Find-or-add, for the reason the single-table one is: the same call
    /// written in the select list and in `GROUP BY` is one computed column,
    /// and registering it twice would return one group per pair.
    /// Which slot of a *group* a name denotes, over a grouped join.
    ///
    /// A group is `[key, aggregates...]`, so this returns 0 for the group key
    /// and `1 + n` for the `n`th aggregate the select list computes. That is a
    /// space of its own: it has nothing to do with either table's ordinals,
    /// and lowering an ORDER BY onto the joined row instead would sort the
    /// rows going *into* the grouping — which the groups then discard, so the
    /// reader's ordering disappears and the sort is wasted work. The
    /// single-table path had exactly that bug and a refusal test found it.
    ///
    /// Looked up rather than registered, for the reason `group_ordinal` is:
    /// `ORDER BY max(year)` over a grouping that computes no `max(year)` is
    /// an ordering by a value the groups do not have.
    fn join_group_ordinal(
        &mut self,
        item: &SelectItem,
        spec: &JoinSpec,
        left: &TableDef,
        right: &TableDef,
        at: usize,
    ) -> Result<u32, SqlError> {
        match item {
            SelectItem::Aggregate { kind, argument, at } => {
                // Matched by what the select list already computes, on either
                // side, so `count(*)` and `max(fare)` both resolve and
                // `max(year)` over a grouping that averages it does not.
                let wanted = self
                    .aggregate(kind, argument.as_deref(), left, *at)
                    .map(|mut a| {
                        a.input = 0;
                        a
                    })
                    .or_else(|_| {
                        self.aggregate(kind, argument.as_deref(), right, *at)
                            .map(|mut a| {
                                a.input = 1;
                                a
                            })
                    })
                    .map_err(|_| SqlError {
                        message: format!(
                            "`{kind}()` reads a column of `{}` or `{}`; `{}` is neither",
                            left.name(),
                            right.name(),
                            argument.as_deref().unwrap_or("*")
                        ),
                        at: *at,
                    })?;
                spec.aggregates
                    .iter()
                    .position(|a| *a == wanted)
                    .and_then(|i| u32::try_from(i + 1).ok())
                    .ok_or_else(|| SqlError {
                        message: format!(
                            "ORDER BY names `{kind}({})`, which this query does not \
                             compute — add it to the select list",
                            argument.as_deref().unwrap_or("*")
                        ),
                        at: *at,
                    })
            }
            // A column or a call: it has to *be* the group key, since a
            // grouped join has exactly one and the groups carry nothing else.
            other => {
                // Resolved against the joined row so it can be compared with
                // `group_by`, which is in that space. Registering a new
                // computed column here would be wrong — but `group_by` is
                // already set by the time ORDER BY is parsed, so a call the
                // grouping did not name simply fails the comparison below.
                let mut copy = spec.clone();
                let ordinal = self.join_value_ordinal(other, &mut copy, left, right, at)?;
                if spec.group_by == Some(ordinal) {
                    return Ok(0);
                }
                Err(SqlError {
                    message: "ORDER BY on a grouped join names the group key or one of \
                              its aggregates; a group carries nothing else"
                        .to_owned(),
                    at,
                })
            }
        }
    }

    fn join_value_ordinal(
        &mut self,
        item: &SelectItem,
        spec: &mut JoinSpec,
        left: &TableDef,
        right: &TableDef,
        at: usize,
    ) -> Result<u32, SqlError> {
        match item {
            SelectItem::Column { raw, at } => {
                let (input, column) = self.resolve_side(raw, left, right, *at)?;
                let base = if input == 0 { 0 } else { left.columns().len() };
                u32::try_from(base + column as usize).map_err(|_| SqlError {
                    message: "too many columns".to_owned(),
                    at: *at,
                })
            }
            SelectItem::Call {
                function,
                argument,
                offset,
                zone,
                ..
            } => {
                let (input, column) = self.resolve_side(argument, left, right, at)?;
                let wanted = ComputeSpec {
                    function: function.clone(),
                    input,
                    column,
                    offset: *offset,
                    zone: zone.clone(),
                };
                let position = spec
                    .compute
                    .iter()
                    .position(|c| *c == wanted)
                    .unwrap_or_else(|| {
                        spec.compute.push(wanted);
                        spec.compute.len() - 1
                    });
                let width = left.columns().len() + right.columns().len();
                u32::try_from(width + position).map_err(|_| SqlError {
                    message: "too many columns".to_owned(),
                    at,
                })
            }
            SelectItem::Aggregate { .. } => Err(SqlError {
                message: "an aggregate cannot be a group key".to_owned(),
                at,
            }),
        }
    }

    /// Where a group-space item sits in `[keys..., aggregates...]`.
    ///
    /// Resolved against what the query already said rather than against the
    /// table: `count(*)` means "the aggregate I asked for", and naming one the
    /// select list does not compute would be naming a number that is not in
    /// the answer.
    ///
    /// `clause` is only for the error text. It is a parameter because both
    /// ORDER BY and HAVING resolve through here, and the messages said
    /// "ORDER BY" for a HAVING that had never mentioned it — an error naming a
    /// clause the reader did not write sends them looking in the wrong place.
    fn group_ordinal(
        &self,
        item: &SelectItem,
        spec: &QuerySpec,
        table: &TableDef,
        at: usize,
        clause: &str,
    ) -> Result<u32, SqlError> {
        match item {
            SelectItem::Column { raw, at } => {
                let ordinal = self.resolve(raw, table, *at)?;
                spec.group_by
                    .iter()
                    .position(|k| *k == ordinal)
                    .map(|i| u32::try_from(i).unwrap_or(0))
                    .ok_or_else(|| SqlError {
                        message: format!("`{raw}` is not a group key, so {clause} cannot use it"),
                        at: *at,
                    })
            }
            SelectItem::Call {
                function,
                argument,
                offset,
                zone,
                ..
            } => {
                // The call must already be a group key — resolved against
                // what the query said, exactly as a bare column is. Looking it
                // up rather than registering it is the point: `ORDER BY
                // hour(x)` over a grouping that did not group by `hour(x)`
                // orders by a value the groups do not have.
                let column = self.resolve(argument, table, at)?;
                let wanted = ComputeSpec {
                    function: function.clone(),
                    input: 0,
                    column,
                    offset: *offset,
                    zone: zone.clone(),
                };
                let ordinal = spec
                    .compute
                    .iter()
                    .position(|c| *c == wanted)
                    .map(|i| table.columns().len() + i)
                    .and_then(|o| u32::try_from(o).ok())
                    .ok_or_else(|| SqlError {
                        message: format!(
                            "{clause} calls `{function}()` on a value this query \
                                          does not compute"
                        ),
                        at,
                    })?;
                spec.group_by
                    .iter()
                    .position(|k| *k == ordinal)
                    .map(|i| u32::try_from(i).unwrap_or(0))
                    .ok_or_else(|| SqlError {
                        message: format!(
                            "`{function}({argument})` is not a group key, so {clause} cannot \
                             use it"
                        ),
                        at,
                    })
            }
            SelectItem::Aggregate { kind, argument, .. } => {
                let wanted = self.aggregate(kind, argument.as_deref(), table, at)?;
                let position = spec
                    .aggregates
                    .iter()
                    .position(|a| a.kind == wanted.kind && a.column == wanted.column)
                    .ok_or_else(|| SqlError {
                        message: format!(
                            "{clause} names an aggregate this query does not compute — \
                             add it to the select list"
                        ),
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
        let input = 0;
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
        Ok(AggregateSpec {
            kind,
            input,
            column,
        })
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
            // An optional second argument, only ever a timezone:
            // `hour(pickup_time, '-05:00')`. Read before the `)` and refused
            // for a name that is not a time function, so `max(a, b)` says the
            // aggregate takes one column rather than complaining about a zone.
            let name_lower = name.to_ascii_lowercase();
            let mut offset = 0;
            let mut zone = String::new();
            if self.eat_symbol(",") {
                let zone_at = self.at();
                let text = self.literal()?;
                if !TIME_FUNCTIONS.contains(&name_lower.as_str()) {
                    return Err(SqlError {
                        message: format!("{name_lower}() takes one column"),
                        at: zone_at,
                    });
                }
                (offset, zone) = parse_zone(&text).map_err(|message| SqlError {
                    message,
                    at: zone_at,
                })?;
            }
            self.expect_symbol(")")?;
            let name = name_lower;
            // Told apart by name, because they are told apart by nothing else:
            // both are `word(column)`. The alternative — deciding later, from
            // whether the name resolves as an aggregate — would put the
            // "no such aggregate: hour" error on a query that never meant one.
            if TIME_FUNCTIONS.contains(&name.as_str()) {
                let Some(argument) = argument else {
                    return Err(SqlError {
                        message: format!("{name}() needs a column, not `*`"),
                        at,
                    });
                };
                return Ok(SelectItem::Call {
                    function: name,
                    argument,
                    offset,
                    zone,
                    at,
                });
            }
            return Ok(SelectItem::Aggregate {
                kind: name,
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
        let (op, value) = self.comparison_tail()?;
        Ok(FilterSpec { column, op, value })
    }

    /// The operator and the literal after it, shared by `WHERE` and `HAVING`.
    ///
    /// One function rather than two, so the two clauses cannot come to accept
    /// different operators — a reader who writes `>=` in a WHERE and finds it
    /// rejected in a HAVING has found a bug in the parser, not in their query.
    fn comparison_tail(&mut self) -> Result<(String, String), SqlError> {
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
        Ok((op.to_owned(), value))
    }

    /// One `HAVING` comparison: `count(*) > 100`, `avg(total) >= 25.0`.
    ///
    /// The left side is a select item rather than a column, and it resolves
    /// into *group* space through the same `group_ordinal` ORDER BY uses — so
    /// `HAVING count(*) > 100` and `ORDER BY count(*)` agree on which number
    /// they mean, and both refuse an aggregate the query does not compute
    /// rather than silently computing a second one.
    ///
    /// The operator half is `condition`'s, deliberately: a reader who can
    /// write `WHERE total >= 25` should not discover that HAVING spells it
    /// differently. `LIKE` and `~` come along for free and are meaningful on a
    /// string group key.
    fn having_condition(
        &mut self,
        spec: &QuerySpec,
        table: &TableDef,
    ) -> Result<FilterSpec, SqlError> {
        let at = self.at();
        let item = self.select_item()?;
        let column = self.group_ordinal(&item, spec, table, at, "HAVING")?;
        let (op, value) = self.comparison_tail()?;
        Ok(FilterSpec { column, op, value })
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
            // A select item rather than a bare column, so `GROUP BY
            // hour(pickup_time)` reaches the same find-or-add the single-table
            // path uses. `Join::compute` appends after *both* tables, which is
            // where `join_value_ordinal` puts it.
            let item = self.select_item()?;
            let key = self.join_value_ordinal(&item, &mut spec, left, &right, at)?;
            spec.group_by = Some(key);
        }

        for item in list {
            match item {
                SelectItem::Aggregate { kind, argument, at } => {
                    // Either side. This used to resolve against `right` only,
                    // which is why an aggregate over the left table -- the
                    // common case, since the left is the fact table -- was
                    // "not a column of zones".
                    let mut parsed = self.aggregate(kind, argument.as_deref(), left, *at);
                    let mut input = 0;
                    if parsed.is_err() && argument.is_some() {
                        let on_right = self.aggregate(kind, argument.as_deref(), &right, *at);
                        if on_right.is_ok() {
                            parsed = on_right;
                            input = 1;
                        }
                    }
                    let mut parsed = parsed.map_err(|_| SqlError {
                        message: format!(
                            "`{kind}()` reads a column of `{}` or `{}`; `{}` is neither",
                            left.name(),
                            right.name(),
                            argument.as_deref().unwrap_or("*")
                        ),
                        at: *at,
                    })?;
                    parsed.input = input;
                    spec.aggregates.push(parsed);
                }
                SelectItem::Call { at, .. } => {
                    // Registered by find-or-add, so `SELECT hour(t), count(*)
                    // ... GROUP BY hour(t)` names one computed column rather
                    // than two. It must already be the group key: a computed
                    // column beside a grouping that did not group by it is the
                    // same error a bare column gets below, for the same reason.
                    let ordinal = self.join_value_ordinal(item, &mut spec, left, &right, *at)?;
                    if spec.group_by != Some(ordinal) {
                        return Err(SqlError {
                            message: "a computed column on a join has to be the group key — \
                                      a join returns whole rows or one row per group, and \
                                      there is no third shape"
                                .to_owned(),
                            at: *at,
                        });
                    }
                }
                SelectItem::Column { raw, at } => {
                    // Selecting a bare column beside a GROUP BY is the "not in
                    // the group key" error every SQL engine has — *unless* it
                    // is the group key, which is the ordinary shape of a
                    // grouped query and was refused here along with everything
                    // else. `SELECT borough, count(*) ... GROUP BY borough`
                    // came back as "`borough` is not the group key", which was
                    // both wrong and confusing, because it was.
                    //
                    // The check was `spec.group_by.is_some()` and never
                    // compared the two ordinals. It went unnoticed because
                    // every test of a grouped join keyed on a *computed*
                    // column, which takes the branch above.
                    if let Some(key) = spec.group_by {
                        let named = self
                            .resolve_side(raw, left, &right, *at)
                            .map(|(input, column)| {
                                let base = if input == 0 { 0 } else { left.columns().len() };
                                base as u32 + column
                            })
                            .ok();
                        if named != Some(key) {
                            return Err(SqlError {
                                message: format!(
                                    "`{raw}` is not the group key — a grouped query returns \
                                     the key and the aggregates"
                                ),
                                at: *at,
                            });
                        }
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
            self.expect("by")?;
            // Only over groups. `Join` has no sort field — the kernel orders
            // *groups* and not joined rows — so an ungrouped join's ORDER BY
            // has nowhere to be lowered, and the refusal now says which of the
            // two shapes the reader is in rather than "not supported yet",
            // which was true of both and explained neither.
            if spec.group_by.is_none() {
                return Err(SqlError {
                    message: "ORDER BY on a join needs a GROUP BY: the kernel orders groups, not \
                              joined rows, so there is nothing to lower an ordering of \
                              whole rows onto"
                        .to_owned(),
                    at: self.at(),
                });
            }
            loop {
                let at = self.at();
                let item = self.select_item()?;
                let column = self.join_group_ordinal(&item, &spec, left, &right, at)?;
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

/// The functions `select_item` treats as computed columns rather than
/// aggregates. Kept beside the parser and checked against `compute_scalar`'s
/// match by `every_time_function_the_parser_accepts_is_one_the_binding_lowers`
/// — two lists that must agree, with a test rather than a comment holding them
/// together.
/// A second argument to a time function: a fixed offset, or a named zone.
///
/// Returns `(seconds, name)` with exactly one side filled in — a fixed offset
/// in seconds and an empty name, or zero and an IANA name. Two returns rather
/// than an enum because both are already fields on `ComputeSpec`, and a
/// two-variant enum wrapping two integers to be unwrapped at the only call
/// site is ceremony.
///
/// `'-05:00'`, `'+05:30'`, `'UTC'` and `'Z'` are fixed. `+05:30` and `-09:30`
/// are real zones, so minutes are parsed rather than assumed to be zero; India
/// and Newfoundland are not edge cases anyone should have to work around.
///
/// `'America/New_York'` is a name, and the kernel's table resolves it to the
/// offset in force at each row's instant. That was refused for two rounds with
/// a reason that was true of a timezone *database* — megabytes, a parser, a
/// release cadence — and not of a timezone *table*, which for a curated list
/// of zones is a few kilobytes of sorted integers. A name outside the list is
/// still refused, and the refusal now says which names are there rather than
/// saying the feature does not exist.
fn parse_zone(text: &str) -> Result<(i64, String), String> {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("utc") || trimmed.eq_ignore_ascii_case("z") {
        return Ok((0, String::new()));
    }
    if slate_kernel::zones::has(trimmed) {
        return Ok((0, trimmed.to_owned()));
    }
    let malformed = || {
        // A name-shaped argument gets the zone list and an offset-shaped one
        // gets the offset syntax: `America/new_york` is a spelling mistake and
        // `-5:00` is a syntax one, and one message for both helps neither.
        if trimmed.contains('/') || trimmed.chars().next().is_some_and(char::is_alphabetic) {
            format!(
                "no such timezone: `{trimmed}`. IANA names are case-sensitive, \
                 and this has {}",
                slate_kernel::zones::listing()
            )
        } else {
            format!("`{trimmed}` is not a UTC offset — write one like '-05:00' or '+05:30'")
        }
    };
    let (sign, rest) = match trimmed.split_at_checked(1) {
        Some(("+", rest)) => (1, rest),
        Some(("-", rest)) => (-1, rest),
        _ => return Err(malformed()),
    };
    let (hours, minutes) = match rest.split_once(':') {
        Some((h, m)) => (h, m),
        // `-05` is unambiguous and common enough to accept.
        None => (rest, "00"),
    };
    // Digit counts checked explicitly: `parse` accepts `+5` and `5 `, and an
    // offset that reads loosely is one that reads a typo as a time.
    let two_digits = |s: &str| s.len() == 2 && s.chars().all(|c| c.is_ascii_digit());
    if !two_digits(hours) || !two_digits(minutes) {
        return Err(malformed());
    }
    let (Ok(hours), Ok(minutes)) = (hours.parse::<i64>(), minutes.parse::<i64>()) else {
        return Err(malformed());
    };
    if hours > 14 || minutes > 59 {
        return Err(format!(
            "`{trimmed}` is not a real offset: they run from -12:00 to +14:00"
        ));
    }
    Ok((sign * (hours * 3_600 + minutes * 60), String::new()))
}

/// The aggregate names, for telling an unknown function from a misplaced
/// aggregate. `aggregate()` remains the authority on what is accepted; this is
/// only for the error text, and a test keeps the two in step.
const AGGREGATES: &[&str] = &["count", "min", "max", "sum", "avg"];

const TIME_FUNCTIONS: &[&str] = &[
    "hour",
    "minute",
    "second",
    "year",
    "month",
    "day",
    "day_of_week",
    "date",
    "month_start",
    "year_start",
    // Not a time function, and here anyway: this list is really "the names
    // that are computed columns rather than aggregates", and one arithmetic
    // function does not earn a second list to be the only member of.
    "round",
];

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
    /// `hour(pickup_time)`: a function of one column, computed per row.
    ///
    /// The optional second argument — `hour(pickup_time, '-05:00')` or
    /// `hour(pickup_time, 'America/New_York')` — is a fixed offset in
    /// `offset`, in seconds, or an IANA name in `zone`. Never both: a literal
    /// is one or the other, and `parse_zone` decides which.
    ///
    /// Part of the item's identity, so `hour(t)`, `hour(t, '-05:00')` and
    /// `hour(t, 'America/New_York')` are three computed columns and a query
    /// may select all three; they are three different questions, and in New
    /// York the second and third differ for a third of the year.
    Call {
        function: String,
        argument: String,
        offset: i64,
        zone: String,
        at: usize,
    },
}
