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

use crate::{
    AggregateSpec, ChainInputSpec, ChainOnSpec, ChainSpec, ComputeSpec, FilterSpec, JoinSpec,
    QuerySpec, SortSpec,
};
use slate_schema::TableDef;
use slate_tuple::ValueType;

/// Can a candidate of type `inner` ever equal a value of type `outer`?
///
/// Used to refuse `WHERE title IN (SELECT id FROM authors)` at parse time
/// rather than answer it with nothing. That query is not an error anywhere
/// downstream: the binding renders each candidate to text and parses it back
/// against the outer column's type, and `1` parses perfectly well as the
/// string `"1"` — so a comparison that can never be true returns zero rows and
/// looks like a fact about the data. Zero rows with no explanation is the worst
/// answer this front end can give, because it is indistinguishable from a
/// correct one.
///
/// The rule is *same type, or both numeric*, and the width mixing is
/// deliberate: `u64 IN (SELECT an_i64 …)` is an ordinary thing to write, the
/// text round-trip handles it, and a negative candidate against a `u64` column
/// already fails loudly at run time with the value in the message. Refusing
/// that pair here would refuse a query that works.
fn comparable(outer: ValueType, inner: ValueType) -> bool {
    fn numeric(t: ValueType) -> bool {
        matches!(
            t,
            ValueType::I64 | ValueType::U64 | ValueType::F64 | ValueType::Decimal
        )
    }
    outer == inner || (numeric(outer) && numeric(inner))
}

/// A parsed statement, already lowered onto the spec types.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// A single-table read.
    Select(QuerySpec),
    /// Two tables, grouped or not.
    Join(JoinSpec),
    /// Three or more, grouped or not.
    ///
    /// A separate variant rather than a `Join` with a longer list, because the
    /// kernel has two entry points and they are not interchangeable: `Join`
    /// narrows each side's projection for a grouped read and `Chain` cannot, so
    /// the two produce different plans. `slate-server` splits them the same way
    /// and for the same reason. The parser decides which by counting tables,
    /// which is the only place the count is known, and never emits a two-input
    /// `Chain` — that would be a second path to a query the first already
    /// answers, which is exactly the drift this front end exists to avoid.
    Chain(ChainSpec),
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
/// What a grouped join or chain has decided so far, as the group space sees it.
///
/// A group is `[keys..., aggregates...]` and a key may be a computed value, so
/// resolving any name in that space needs all three of these — they travel
/// together at every call. They were three parameters until clippy counted
/// eight, and bundling them is better than the `allow` would have been: the
/// grouping is one thing, and a signature that says so cannot be handed two of
/// the three from one query and the third from another.
#[derive(Debug, Clone, Copy)]
struct GroupSpace<'a> {
    keys: &'a [u32],
    aggregates: &'a [AggregateSpec],
    compute: &'a [ComputeSpec],
}

/// One input of a join or a chain: a table, and the name this query calls it by.
///
/// The name is the alias where there is one and the table's own name otherwise,
/// and it is what a qualifier resolves against. Two inputs may hold the *same*
/// `TableDef` — that is the entire point of an alias — so nothing downstream may
/// identify an input by its table. Everything that used to take `&[TableDef]`
/// takes `&[Input]` for exactly that reason: with two `zones` in the list, a
/// lookup by table name has two answers and no way to choose.
#[derive(Debug, Clone)]
struct Input {
    table: TableDef,
    name: String,
}

impl Input {
    fn name(&self) -> &str {
        &self.name
    }
}

/// The words that may legally follow a table name here, and so cannot be a
/// bare alias.
///
/// `FROM trips JOIN zones` would otherwise take `JOIN` as an alias for `trips`
/// and then fail on `zones` with "expected ON" — an error about the wrong
/// token, three words after the actual mistake, which is no mistake at all.
/// `AS` needs no such list, which is why both forms are worth having.
const FOLLOWS_A_TABLE: &[&str] = &[
    "join", "inner", "on", "where", "group", "order", "limit", "offset", "having", "as",
];

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
/// Where an `OVER (` appears, if one does.
///
/// Two tokens rather than the word alone: `over` is not reserved here and a
/// column could plausibly be called that, while `over` immediately followed by
/// an open paren is a window clause in every SQL dialect and is nothing else
/// in this grammar.
fn window_clause(toks: &[Spanned]) -> Option<usize> {
    toks.windows(2).find_map(|pair| {
        let [word, paren] = pair else { return None };
        match (&word.tok, &paren.tok) {
            (Tok::Word(name), Tok::Symbol(symbol))
                if name.eq_ignore_ascii_case("over") && symbol == "(" =>
            {
                Some(word.at)
            }
            _ => None,
        }
    })
}

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
    // `OVER (` before parsing rather than after, because the word before it is
    // a function name the grammar does not have — `ROW_NUMBER() OVER (…)`
    // would otherwise fail at `ROW_NUMBER` with "unknown function", which is
    // true and unhelpful: the answer is not that the name is wrong, it is that
    // this front end cannot ask for a window.
    //
    // A refusal rather than a gap in the parser, and the distinction matters:
    // the kernel *has* the operator (`slate_kernel::window`), with
    // `ROW_NUMBER`, `RANK`, `DENSE_RANK`, `LAG`, `LEAD` and any aggregate over
    // a partition. What it does not have is a way to ask for one from here —
    // `QuerySpec` has no window field and neither does the wire. Saying so is
    // the difference between "not built" and "built, and you cannot reach it",
    // which are different things to do about it.
    if let Some(at) = window_clause(&toks) {
        return Err(SqlError {
            message: "OVER is not supported by this front end. The kernel does have window                  functions — ROW_NUMBER, RANK, DENSE_RANK, LAG, LEAD and any aggregate over                  a partition, with SQL's own frame rule — but nothing can ask for one from                  here: the query spec this compiles to has no window, and neither does the                  gRPC protocol the three clients speak. GROUP BY gives you one row per                  partition where a window gives you one per input row; if the aggregate is                  what you want rather than the per-row value, that is the clause to reach                  for"
                .to_owned(),
            at,
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
        // The set operators get their own message. "unexpected `UNION`" is
        // true and reads as a parser that has not heard of it, when the real
        // answer is that there is nowhere for it to go: a statement compiles
        // to one `QuerySpec`, which names one table and one plan, and that is
        // the same structure the three clients put on the wire.
        if let Tok::Word(word) = &extra.tok {
            let word = word.to_ascii_lowercase();
            if matches!(word.as_str(), "union" | "intersect" | "except") {
                return Err(SqlError {
                    message: format!(
                        "{} is not supported: a statement compiles to one query spec — one \
                         table, one filter set, one plan — and the spec has no set \
                         operator, so there is nothing to lower this onto.{} Two \
                         statements separated by `;` run both halves and show you \
                         both plans",
                        word.to_uppercase(),
                        if word == "union" {
                            " Combining the two answers is the easy half; it is the \
                             deduplication across them that nothing here can do, because \
                             each statement is planned and executed on its own."
                        } else {
                            ""
                        }
                    ),
                    at,
                });
            }
        }
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

    /// A table and the name this query will call it by: `zones`,
    /// `zones AS pickup` or `zones pickup`.
    fn input(&mut self) -> Result<Input, SqlError> {
        let table = self.table()?;
        let name = self.alias()?.unwrap_or_else(|| table.name().to_owned());
        Ok(Input { table, name })
    }

    /// [`Self::input`] for the table right after `FROM`, where a bare alias is
    /// taken only when a `JOIN` follows it.
    ///
    /// The extra condition is there because of a typo. `FOLLOWS_A_TABLE` keeps
    /// `JOIN` and `WHERE` from being read as aliases, but it cannot keep a
    /// *misspelled* keyword from being one: `SELECT * FROM books WERE id = 1`
    /// read `WERE` as an alias for `books` and reported "`WERE` aliases a
    /// single table", burying the actual mistake under a rule the reader had
    /// never heard of. The old message was "unexpected `WERE`", which is the
    /// right one, and no list of keywords can get it back — a typo is by
    /// definition not in the list.
    ///
    /// So a bare alias here has to be followed by the thing that makes an alias
    /// worth having. `FROM books b` alone is not an alias, it is a syntax
    /// error, and `FROM books AS b` still reaches the single-table refusal,
    /// because `AS` says plainly what was meant.
    fn first_input(&mut self) -> Result<Input, SqlError> {
        let table = self.table()?;
        if self.eat("as") {
            let name = self.name()?;
            return Ok(Input { table, name });
        }
        let bare = self
            .peek_word()
            .filter(|word| !FOLLOWS_A_TABLE.contains(&word.as_str()))
            .filter(|_| {
                matches!(
                    self.toks.get(self.i + 1),
                    Some(Spanned { tok: Tok::Word(w), .. })
                        if w.eq_ignore_ascii_case("join") || w.eq_ignore_ascii_case("inner")
                )
            })
            .is_some();
        let name = if bare {
            self.name()?
        } else {
            table.name().to_owned()
        };
        Ok(Input { table, name })
    }

    /// `AS x`, or a bare `x` that is not the next clause's keyword.
    ///
    /// Both forms, because both are written. The bare one needs the
    /// [`FOLLOWS_A_TABLE`] guard and the `AS` one does not, which is the
    /// argument for keeping `AS` rather than only accepting the bare form and
    /// growing that list every time the grammar does.
    ///
    /// The table after `FROM` uses [`Self::first_input`] instead, which is
    /// stricter about the bare form for a reason worth reading there.
    fn alias(&mut self) -> Result<Option<String>, SqlError> {
        if self.eat("as") {
            // After an explicit `AS`, whatever follows is the alias — a
            // keyword there is a refusal from `name`, not a missing alias,
            // because `zones AS group` is a mistake worth naming.
            return self.name().map(Some);
        }
        match self.peek_word() {
            Some(word) if !FOLLOWS_A_TABLE.contains(&word.as_str()) => self.name().map(Some),
            _ => Ok(None),
        }
    }

    /// Resolve a column against one table, accepting a `table.column`
    /// qualifier when it names that table.
    fn column(&mut self, table: &TableDef) -> Result<u32, SqlError> {
        let at = self.at();
        let raw = self.name()?;
        self.resolve(&raw, table, at)
    }

    /// `JOIN` or `INNER JOIN`, consumed if it is next.
    ///
    /// One function because `select` reads the first one and `join_tail` reads
    /// every one after it, and the two-token `INNER JOIN` dance was written out
    /// once when there was only ever one join to read. A second copy of it is
    /// how `a JOIN b INNER JOIN c` would come to mean something different from
    /// `a INNER JOIN b JOIN c`.
    fn eat_join(&mut self) -> bool {
        if !(self.eat("join") || self.eat("inner")) {
            return false;
        }
        if self
            .toks
            .get(self.i)
            .is_some_and(|s| matches!(&s.tok, Tok::Word(w) if w.eq_ignore_ascii_case("join")))
        {
            self.i += 1;
        }
        // A bare `INNER` with no `JOIN` is taken as a join, which is lax and is
        // what this did before it was factored out. Tightening it is a separate
        // change from making it happen in one place instead of two.
        true
    }

    /// The input and ordinal a name refers to, over the joined tables.
    ///
    /// Qualified wins: `zones.borough` names that table even if `borough`
    /// would also resolve on an earlier one. Unqualified takes the first input
    /// that has it, which is what SQL does with an ambiguous name in every
    /// dialect that does not refuse it outright — and refusing would break
    /// `SELECT hour(pickup_time)` on a schema where two tables happen to share
    /// an `id`.
    ///
    /// It is not *silent* about that, though: a bare name that resolves on more
    /// than one input adds a warning naming the one it chose and how to spell
    /// the others. The rule was documented here and nowhere the reader could
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
    ///
    /// Taking a slice rather than a left and a right is what lets a chain
    /// reuse all of it. The two-table version had `left`/`right` in its
    /// signature and in five error messages, which is five places a third
    /// table would have been missing from.
    fn resolve_side(
        &mut self,
        raw: &str,
        inputs: &[Input],
        at: usize,
    ) -> Result<(u32, u32), SqlError> {
        if let Some((qualifier, _)) = raw.split_once('.') {
            // A qualifier names one of these tables or none of them. Reported
            // as its own error rather than tried against each in turn: for two
            // tables `resolve` against the left happened to produce a sensible
            // message, and for three `nosuch.id` would have been reported as
            // not being a column of the first table, which is true and useless.
            let named = inputs
                .iter()
                .enumerate()
                .find(|(_, i)| i.name().eq_ignore_ascii_case(qualifier));
            let Some((at_input, input)) = named else {
                return Err(SqlError {
                    message: format!(
                        "`{raw}` is qualified with `{qualifier}`, which this query does not \
                         read — it reads {}",
                        name_list(inputs, "and")
                    ),
                    at,
                });
            };
            return Ok((
                u32::try_from(at_input).unwrap_or(0),
                self.resolve_named(raw, &input.table, input.name(), at)?,
            ));
        }
        let hits: Vec<(usize, &Input)> = inputs
            .iter()
            .enumerate()
            .filter(|(_, i)| self.resolve_named(raw, &i.table, i.name(), at).is_ok())
            .collect();
        let Some((&(chosen, input), rest)) = hits.split_first() else {
            return Err(SqlError {
                message: format!("`{raw}` is not a column of {}", name_list(inputs, "or")),
                at,
            });
        };
        if !rest.is_empty() {
            // Said once per name rather than once per mention: `SELECT id ...
            // GROUP BY id` names the same column twice and a reader does not
            // need telling twice.
            let all: Vec<&Input> = hits.iter().map(|(_, input)| *input).collect();
            let warning = format!(
                "`{raw}` is a column of {}{}; this read `{}.{raw}`. Qualify it to choose.",
                // "both" only when there are two of them. It read "a column of
                // both `a` and `b`" when there could only ever be two, and a
                // third table would have made that sentence wrong rather than
                // merely long.
                if rest.len() == 1 { "both " } else { "" },
                name_list_refs(&all, "and"),
                input.name()
            );
            if !self.warnings.contains(&warning) {
                self.warnings.push(warning);
            }
        }
        Ok((
            u32::try_from(chosen).unwrap_or(0),
            self.resolve_named(raw, &input.table, input.name(), at)?,
        ))
    }

    fn resolve(&self, raw: &str, table: &TableDef, at: usize) -> Result<u32, SqlError> {
        self.resolve_named(raw, table, table.name(), at)
    }

    /// [`Self::resolve`], against a name that may be an alias.
    ///
    /// The qualifier is matched against `name` and the *column* is looked up in
    /// `table`, which is the whole of what an alias is. Every error here names
    /// `name` rather than the table, because that is what the reader wrote:
    /// "`pickup` has no column `nosuch`" sends them to their own query, where
    /// "`zones` has no column `nosuch`" sends them to a table they may have
    /// named twice.
    fn resolve_named(
        &self,
        raw: &str,
        table: &TableDef,
        name: &str,
        at: usize,
    ) -> Result<u32, SqlError> {
        let bare = match raw.split_once('.') {
            Some((qualifier, rest)) => {
                if !qualifier.eq_ignore_ascii_case(name) {
                    return Err(SqlError {
                        message: format!("`{raw}` is not a column of `{name}`"),
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
                name,
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
            // `WITH` gets its own message, for the reason the set operators do
            // one line below: "found `WITH`" is true and reads as a parser
            // that has not heard of it, when the answer is that `WITH` covers
            // three constructs with three different answers. Saying "CTEs are
            // not supported" would be wrong about one of the three, and the
            // one it would be wrong about is the one somebody could build.
            //
            // Worked through in `docs/ctes.md`; the split is summarised here
            // because an error message a reader has to leave to understand is
            // most of the way back to "unexpected `WITH`".
            // `CREATE` for the same reason as `WITH`, and with the same
            // shape of answer: the interesting part is not that it is
            // unsupported but *what a view would have to be here*. A caller
            // reaching for one is usually reaching for a privilege boundary,
            // and it cannot be one — there is no owner for a view to run as,
            // so a caller needs the grant on the base table either way.
            // Saying that at the moment they ask is the whole point; saying
            // it after they have built a permission model on the opposite
            // assumption is the failure mode `docs/views.md` is about.
            Some("create") => Err(SqlError {
                message: "CREATE is not supported: this front end queries a catalog \
                     rather than defining one — tables are declared in the daemon's \
                     configuration, not by a statement. A view in particular could not \
                     be a privilege boundary here the way it is in Postgres: grants key \
                     on a table id and nothing owns a view, so a caller would still need \
                     the grant on the base table, and having it could read the columns \
                     the view leaves out. See docs/views.md"
                    .to_owned(),
                at: self.at(),
            }),
            Some("with") => Err(SqlError {
                message: "WITH is not supported, and the three things it means have \
                     different reasons. A recursive CTE is a fixpoint loop and a \
                     statement compiles to one plan with nothing to iterate. A CTE \
                     referenced more than once has to be computed once and read twice, \
                     which is a second plan in the same statement — the same reason \
                     UNION is refused. A non-recursive CTE referenced *once* is neither: \
                     it inlines into the outer query, and that inlining is the same \
                     mechanism a view needs, so it is a gap rather than a refusal. See \
                     docs/ctes.md. Meanwhile an uncorrelated subquery works in \
                     `IN (SELECT …)`"
                    .to_owned(),
                at: self.at(),
            }),
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
        // DISTINCT is not an operator here. It is `GROUP BY` over exactly the
        // columns selected, which is what it means and what the kernel already
        // does: a `Grouping` with those keys and no aggregates yields the
        // distinct combinations, in key order. Adding a `Distinct` node to the
        // query spec would have been a second way to say the same thing, with
        // its own planning and its own bugs.
        let distinct = self.eat("distinct");

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

        if distinct && star {
            // Every table here has a primary key, and a primary key already
            // makes the rows distinct — so `SELECT DISTINCT *` is `SELECT *`
            // with a grouping over every column bolted on, which reads every
            // row, hashes all of it, and returns exactly what it was given.
            // Answering it would be correct and would be the slowest possible
            // way to do nothing.
            return Err(SqlError {
                message: "`SELECT DISTINCT *` returns every row: the primary key already \
                          makes rows distinct, so this only costs a grouping. Name the \
                          columns you want the distinct combinations of."
                    .to_owned(),
                at: self.at(),
            });
        }

        self.expect("from")?;
        let first = self.first_input()?;

        if self.eat_join() {
            return self.join_tail(first, &list, star, distinct);
        }

        // An alias on a *single* table is refused rather than ignored. There is
        // nothing to disambiguate from, so it buys only a second spelling of
        // one name — and supporting it would mean threading that name through
        // every single-table helper (`conditions`, `condition`, `column`,
        // `value_ordinal`, `group_ordinal`, `having_condition`, `aggregate`)
        // so that `b.title` resolves. That is a wide change for no
        // disambiguation, and silently ignoring the alias is worse than either:
        // `SELECT b.title FROM books b` would then fail with "`b.title` is not
        // a column of `books`", which is true and explains nothing.
        if !first.name.eq_ignore_ascii_case(first.table.name()) {
            return Err(SqlError {
                message: format!(
                    "`{}` aliases a single table, which has nothing to be told apart from. \
                     An alias is for reading one table twice: \
                     `trips JOIN zones AS pickup ... JOIN zones AS dropoff ...`",
                    first.name
                ),
                at: self.at(),
            });
        }
        let table = first.table;

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
        if distinct {
            if self.peek_word().as_deref() == Some("group") {
                return Err(SqlError {
                    message: "DISTINCT and GROUP BY are the same request written twice. \
                              GROUP BY says which columns make a group; DISTINCT says the \
                              selected ones do. Keep one."
                        .to_owned(),
                    at: self.at(),
                });
            }
            // Before the select list is walked below, because that walk
            // branches on whether there is a grouping — and this *is* the
            // grouping. Resolved through the same `value_ordinal` GROUP BY
            // uses, so `SELECT DISTINCT hour(pickup_time)` registers the
            // computed column exactly once and groups on it.
            for item in &list {
                let at = item.at();
                if matches!(item, SelectItem::Aggregate { .. }) {
                    return Err(SqlError {
                        message: "DISTINCT applies to the rows a query returns, and an \
                                  aggregate returns one row per group — there is nothing \
                                  left to deduplicate. Did you mean `count(distinct x)`?"
                            .to_owned(),
                        at,
                    });
                }
                let ordinal = self.value_ordinal(item, &mut spec, &table, at, "DISTINCT")?;
                // `SELECT DISTINCT a, a` is one key written twice. Keeping
                // both would group on the pair — the same answer, with the
                // column repeated in every row and no error anywhere. The
                // same rule GROUP BY's join path already applies.
                if !spec.group_by.contains(&ordinal) {
                    spec.group_by.push(ordinal);
                }
            }
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
                    // No GROUP BY is not an error: it is one group, over every
                    // row. The kernel has always supported a grouping with no
                    // keys — `Grouping::by([], ...)` returns a single group
                    // whose key is empty — so `SELECT count(*) FROM trips` was
                    // refused here and nowhere else, by a guard that mistook
                    // "the usual shape" for "the only shape".
                    spec.aggregates
                        .push(self.aggregate(kind, argument.as_deref(), &table, *at)?);
                }
            }
        }
        // Every non-aggregate in the list has to be a group key, and with no
        // GROUP BY there are none — so a bare column beside an aggregate is
        // the same error it is with a grouping, and has to say so here because
        // the loop above only checks it when `grouping` is true.
        if !grouping && !spec.aggregates.is_empty() && !spec.columns.is_empty() {
            return Err(SqlError {
                message: "a column beside an aggregate needs a GROUP BY — without one the \
                          query returns a single row over the whole table, and there is no \
                          one value for that column to take"
                    .to_owned(),
                at: self.at(),
            });
        }
        // `SELECT zone FROM trips GROUP BY zone` — a grouping with keys and no
        // aggregates — is the distinct keys, and comes back as exactly that.
        // There used to be an empty `if` here saying the binding appended a
        // `count(*)` "so the answer is not a bare column". It did, and the
        // answer was then two columns wide with one the query never mentioned.
        // The default is gone; `SELECT DISTINCT` is the same lowering asked for
        // by name.
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

    /// The joined-space ordinal a join or chain's group key denotes,
    /// registering a computed column if it is one.
    ///
    /// The single-table twin is [`Self::value_ordinal`]; this one differs in
    /// where a computed column lands. On one table they sit after that table's
    /// columns; here they sit after *every* table's, because each later table
    /// already occupies the ordinals after the one before it. Getting this
    /// wrong is not an error but a wrong answer — the group key would be some
    /// later table's column — which is why the kernel now refuses a side's own
    /// computed column outright rather than letting it land there.
    ///
    /// Find-or-add, for the reason the single-table one is: the same call
    /// written in the select list and in `GROUP BY` is one computed column,
    /// and registering it twice would return one group per pair.
    ///
    /// Takes the `compute` list rather than a whole spec, because there are two
    /// specs now — [`JoinSpec`] and [`ChainSpec`] — and the list is the only
    /// part of either this needs. Taking `&mut JoinSpec` is what made the
    /// two-table version unusable for a chain.
    fn join_value_ordinal(
        &mut self,
        item: &SelectItem,
        compute: &mut Vec<ComputeSpec>,
        inputs: &[Input],
        at: usize,
    ) -> Result<u32, SqlError> {
        match item {
            SelectItem::Column { raw, at } => {
                let (input, column) = self.resolve_side(raw, inputs, *at)?;
                joined_at(inputs, input, column, *at)
            }
            SelectItem::Call {
                function,
                argument,
                offset,
                zone,
                ..
            } => {
                let (input, column) = self.resolve_side(argument, inputs, at)?;
                let wanted = ComputeSpec {
                    function: function.clone(),
                    input,
                    column,
                    offset: *offset,
                    zone: zone.clone(),
                };
                let position = compute
                    .iter()
                    .position(|c| *c == wanted)
                    .unwrap_or_else(|| {
                        compute.push(wanted);
                        compute.len() - 1
                    });
                let width: usize = inputs.iter().map(|i| i.table.columns().len()).sum();
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

    /// An aggregate over whichever of the joined tables has its column.
    ///
    /// Tried against each in turn, because which table a column is on is not
    /// something the reader spells: `avg(fare)` means an aggregate over the
    /// input that has `fare`. This used to resolve against the right table
    /// only, which is why an aggregate over the left — the common case, since
    /// the left is usually the fact table — was "not a column of zones".
    ///
    /// The refusal is the *specific* one wherever there is one to give. Before,
    /// every failure became "reads a column of `a` or `b`; `x` is neither",
    /// including `SELECT nosuchagg(fare) FROM trips JOIN zones` — which said
    /// `fare` was not a column of either table. It is one of both. So a
    /// failure whose argument does resolve somewhere is re-run against that
    /// table and its own error returned, and only a column that is genuinely
    /// nowhere gets the list.
    fn join_aggregate(
        &self,
        kind: &str,
        argument: Option<&str>,
        inputs: &[Input],
        at: usize,
    ) -> Result<AggregateSpec, SqlError> {
        for (at_input, input) in inputs.iter().enumerate() {
            if let Ok(mut spec) = self.aggregate_named(kind, argument, input, at) {
                spec.input = u32::try_from(at_input).unwrap_or(0);
                return Ok(spec);
            }
        }
        let Some(first) = inputs.first() else {
            return Err(SqlError {
                message: "a join reads at least one table".to_owned(),
                at,
            });
        };
        let Some(name) = argument else {
            // No argument at all: `count(*)` cannot get here, so this is
            // `avg(*)` or an unknown name, and the first input's refusal says
            // which.
            return self.aggregate_named(kind, argument, first, at);
        };
        match inputs
            .iter()
            .find(|i| self.resolve_named(name, &i.table, i.name(), at).is_ok())
        {
            Some(input) => self.aggregate_named(kind, argument, input, at),
            None => Err(SqlError {
                message: format!(
                    "`{kind}()` reads a column of {}; `{name}` is {}",
                    name_list(inputs, "or"),
                    if inputs.len() == 2 {
                        "neither"
                    } else {
                        "none of them"
                    }
                ),
                at,
            }),
        }
    }

    /// Which slot of a *group* a name denotes, over a grouped join or chain.
    ///
    /// A group is `[keys..., aggregates...]`, so this returns a key's position
    /// among the keys, and `keys.len() + n` for the `n`th aggregate the select
    /// list computes. That is a space of its own: it has nothing to do with any
    /// table's ordinals,
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
        group: GroupSpace<'_>,
        inputs: &[Input],
        at: usize,
        clause: &str,
    ) -> Result<u32, SqlError> {
        let GroupSpace {
            keys: group_by,
            aggregates,
            compute,
        } = group;
        match item {
            SelectItem::Aggregate { kind, argument, at } => {
                // Matched by what the select list already computes, on any
                // input, so `count(*)` and `max(fare)` both resolve and
                // `max(year)` over a grouping that averages it does not.
                let wanted = self.join_aggregate(kind, argument.as_deref(), inputs, *at)?;
                aggregates
                    .iter()
                    .position(|a| *a == wanted)
                    // Past the keys, however many there are. This was `i + 1`,
                    // correct while a grouped join had exactly one key and
                    // silently off by one for every key after the first --
                    // ordering by `count(*)` over two keys would have ordered
                    // by the second key instead, which is a different answer
                    // with nothing to report it.
                    .and_then(|i| u32::try_from(group_by.len() + i).ok())
                    .ok_or_else(|| SqlError {
                        message: format!(
                            "{clause} names `{kind}({})`, which this query does not \
                             compute — add it to the select list",
                            argument.as_deref().unwrap_or("*")
                        ),
                        at: *at,
                    })
            }
            // A column or a call: it has to *be* the group key, since a
            // grouped join or chain has exactly one and the groups carry
            // nothing else.
            other => {
                // Resolved against the joined row so it can be compared with
                // the group key, which is in that space. Registering a new
                // computed column here would be wrong, so this gets a copy of
                // the list -- the group key is already chosen by the time
                // ORDER BY is parsed, so a call the grouping did not name
                // simply fails the comparison below.
                let mut copy = compute.to_vec();
                let ordinal = self.join_value_ordinal(other, &mut copy, inputs, at)?;
                if let Some(at_key) = group_by.iter().position(|key| *key == ordinal) {
                    // Its position among the keys, not 0: with two keys,
                    // `ORDER BY the_second_one` sorts by the second column of
                    // the group and returning 0 would sort by the first.
                    return u32::try_from(at_key).map_err(|_| SqlError {
                        message: "too many group keys".to_owned(),
                        at,
                    });
                }
                Err(SqlError {
                    message: format!(
                        "{clause} on a grouped {} names one of its group keys or one of \
                         its aggregates; a group carries nothing else",
                        shape(inputs)
                    ),
                    at,
                })
            }
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
    /// [`Self::aggregate`], against one input, so an alias resolves its
    /// argument: `avg(pickup.total)` reads `total` of whatever table `pickup`
    /// names.
    fn aggregate_named(
        &self,
        kind: &str,
        argument: Option<&str>,
        input: &Input,
        at: usize,
    ) -> Result<AggregateSpec, SqlError> {
        self.aggregate_on(kind, argument, &input.table, input.name(), at)
    }

    fn aggregate(
        &self,
        kind: &str,
        argument: Option<&str>,
        table: &TableDef,
        at: usize,
    ) -> Result<AggregateSpec, SqlError> {
        self.aggregate_on(kind, argument, table, table.name(), at)
    }

    fn aggregate_on(
        &self,
        kind: &str,
        argument: Option<&str>,
        table: &TableDef,
        name: &str,
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
            Some(raw) => self.resolve_named(raw, table, name, at)?,
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
        let at = self.at();

        // `EXISTS` and `NOT EXISTS` open a condition rather than follow a
        // column, so without this they reach `column` and come back as "no
        // such column: `exists`" — which sends a reader to their schema
        // looking for a word they got out of SQL. Both are refused by name,
        // and the first one is refused with the form that does work.
        let negated = self.peek_word().as_deref() == Some("not");
        let exists_at = if negated { self.i + 1 } else { self.i };
        if matches!(
            self.toks.get(exists_at),
            Some(Spanned { tok: Tok::Word(w), .. }) if w.eq_ignore_ascii_case("exists")
        ) {
            return Err(SqlError {
                message: if negated {
                    // Correlated for the same reason `EXISTS` is, and now it
                    // has the same kind of answer to point at: `NOT IN` used to
                    // be refused a screen down, and is not any more.
                    "NOT EXISTS is not supported: it is correlated — the inner query \
                     asks something about each outer row — and a subquery here runs \
                     once, before the outer query, to build a candidate list. Written \
                     as `key NOT IN (SELECT other_key FROM other WHERE …)` the same \
                     question works, and that is the form this compiles"
                        .to_owned()
                } else {
                    "EXISTS is not supported: it is correlated — the inner query asks \
                     something about each outer row — and a subquery here runs once, \
                     before the outer query, to build a candidate list. Written as \
                     `key IN (SELECT other_key FROM other WHERE …)` the same question \
                     works, and that is the form this compiles"
                        .to_owned()
                },
                at,
            });
        }

        let column = self.column(table)?;
        // `NOT IN` lowers to `Expr::not(Expr::In { .. })`, and that is exactly
        // right rather than approximately right.
        //
        // **This was refused**, on the reasoning that `IN` is three-valued here
        // — a null candidate makes the answer unknown rather than false — so
        // "the negation a reader expects and the one the kernel would give
        // differ exactly where nulls are involved". The first half is true and
        // the conclusion does not follow: standard SQL's `NOT IN` is three-
        // valued in precisely the same way, `Truth::negate` maps unknown to
        // unknown, and `Expr::Not` over `Expr::In` therefore gives the answer
        // the standard specifies, surprise and all. The surprise belongs to
        // SQL, not to this implementation, and refusing a construct because
        // users find the standard counter-intuitive is a different decision
        // from the one that comment was making.
        //
        // The README said the blocker was that "the kernel has `Expr::In` and
        // no negation of it". It has `Expr::Not`, which composes over anything.
        //
        // The planner is the part that had to be checked rather than assumed:
        // `Expr::conjuncts` stops at a `Not`, so `collect_constraints` never
        // sees the `In` inside one and derives no range from it. A `NOT IN`
        // is a residual filter over whatever access path the rest of the
        // predicate chooses, which is both correct and the only thing it could
        // be — the complement of a set of points is not a range.
        if self.peek_word().as_deref() == Some("not")
            && matches!(
                self.toks.get(self.i + 1),
                Some(Spanned { tok: Tok::Word(w), .. }) if w.eq_ignore_ascii_case("in")
            )
        {
            self.i += 2;
            return self.in_tail(column, at, table, true);
        }
        if self.eat("in") {
            return self.in_tail(column, at, table, false);
        }
        let (op, value) = self.comparison_tail()?;
        Ok(FilterSpec {
            column,
            op,
            value,
            ..FilterSpec::default()
        })
    }

    /// `IN (1, 2, 3)`, or `IN (SELECT one_column FROM other WHERE …)`.
    ///
    /// Both forms end as the same `Expr::In`, which the planner already turns
    /// into point gets or an index range — that is why a subquery needed no new
    /// kernel operator. The difference is only where the list comes from: a
    /// literal one is in the spec, and a queried one is filled in by the
    /// binding, which runs the inner query first.
    ///
    /// Only an *uncorrelated* subquery fits that shape. The inner query runs
    /// once, before the outer one, so it cannot see a row of the outer table —
    /// and because it is parsed against its own table, a column of the outer
    /// one is already "no such column" there.
    fn in_tail(
        &mut self,
        column: u32,
        at: usize,
        table: &TableDef,
        negated: bool,
    ) -> Result<FilterSpec, SqlError> {
        let op = if negated { "notIn" } else { "in" };
        self.expect_symbol("(")?;

        if self.peek_word().as_deref() == Some("select") {
            let (subquery, candidate) = self.subquery()?;
            self.expect_symbol(")")?;
            let outer = table
                .columns()
                .get(column as usize)
                .map(slate_schema::ColumnDef::value_type);
            if let (Some(outer), Some(inner)) = (outer, candidate)
                && !comparable(outer, inner)
            {
                return Err(SqlError {
                    message: format!(
                        "`{}` is {} and the subquery produces {}: no candidate could ever \
                         equal it, so this would answer nothing rather than fail",
                        table
                            .columns()
                            .get(column as usize)
                            .map_or("that column", slate_schema::ColumnDef::name),
                        outer.name(),
                        inner.name()
                    ),
                    at,
                });
            }
            return Ok(FilterSpec {
                column,
                op: op.to_owned(),
                subquery: Some(Box::new(subquery)),
                ..FilterSpec::default()
            });
        }

        let mut values = vec![self.literal()?];
        while self.eat_symbol(",") {
            values.push(self.literal()?);
        }
        self.expect_symbol(")")?;
        Ok(FilterSpec {
            column,
            op: op.to_owned(),
            values,
            ..FilterSpec::default()
        })
    }

    /// The inner `SELECT` of an `IN (…)`: one column, one table, an optional
    /// `WHERE`, and nothing else.
    ///
    /// Everything else is refused by name rather than ignored. A subquery with
    /// a `GROUP BY` or a `JOIN` is a reasonable thing to write and this cannot
    /// run it; accepting the text and quietly answering a different question is
    /// the failure this parser has been bitten by before.
    /// Returns the spec and, when the selected item is a plain column of the
    /// inner table, its type — which is what [`Self::in_tail`] type-checks
    /// against the outer column. `None` for a computed item such as
    /// `hour(pickup_time)`, whose result type is the function's rather than the
    /// column's and is not worth a second table here; the run-time parse still
    /// catches a mismatch there, with the offending value in the message.
    fn subquery(&mut self) -> Result<(QuerySpec, Option<ValueType>), SqlError> {
        self.expect("select")?;
        if self.peek_word().as_deref() == Some("distinct") {
            // Harmless and redundant: `Expr::In` compares against a list, and a
            // repeated candidate changes no answer. Saying so beats either
            // accepting a word that does nothing or leaving a reader to guess
            // whether it mattered.
            return Err(SqlError {
                message: "DISTINCT inside `IN (…)` is redundant: a repeated candidate \
                          changes no answer, because the list is compared against rather \
                          than scanned"
                    .to_owned(),
                at: self.at(),
            });
        }

        let at = self.at();
        let item = self.select_item()?;
        if self.eat_symbol(",") {
            return Err(SqlError {
                message: "a subquery in `IN (…)` returns one column: it is the candidate \
                          list, and there is nothing for a second column to be compared to"
                    .to_owned(),
                at,
            });
        }
        if matches!(item, SelectItem::Aggregate { .. }) {
            return Err(SqlError {
                message: "an aggregate inside `IN (…)` is one value rather than a list, \
                          and a subquery here builds a list"
                    .to_owned(),
                at,
            });
        }

        self.expect("from")?;
        let inner = self.table()?;
        let mut spec = QuerySpec {
            table: inner.name().to_owned(),
            ..QuerySpec::default()
        };

        // Resolved against the *inner* table, which is what makes a correlated
        // subquery a refusal rather than a wrong answer: a column of the outer
        // table is simply not one of this table's.
        let ordinal = self.value_ordinal(&item, &mut spec, &inner, at, "IN (SELECT …)")?;
        spec.columns.push(ordinal);
        let candidate = match item {
            SelectItem::Column { .. } => inner
                .columns()
                .get(ordinal as usize)
                .map(slate_schema::ColumnDef::value_type),
            _ => None,
        };

        if self.eat("where") {
            spec.filters = self.conditions(&inner)?;
        }
        for clause in ["group", "order", "limit", "offset", "having", "join"] {
            if self.peek_word().as_deref() == Some(clause) {
                return Err(SqlError {
                    message: format!(
                        "`{}` inside `IN (…)` is not supported: the subquery runs once to \
                         build a candidate list, so it is one table, one column and an \
                         optional WHERE. Run it as its own statement if you need more",
                        // `GROUP` and `ORDER` are one word to the lexer and two
                        // to the reader, and a message naming half a clause
                        // reads as a typo in the parser.
                        match clause {
                            "group" | "order" => format!("{} BY", clause.to_uppercase()),
                            other => other.to_uppercase(),
                        }
                    ),
                    at: self.at(),
                });
            }
        }
        Ok((spec, candidate))
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
        Ok(FilterSpec {
            column,
            op,
            value,
            ..FilterSpec::default()
        })
    }

    // --- the join ---------------------------------------------------------

    /// `... FROM trips JOIN zones ON trips.pickup_zone = zones.id ...`, and
    /// as many further `JOIN <table> ON <col> = <col>` clauses as are written.
    ///
    /// Any tables and any columns, checked against the schema. Two versions
    /// ago this hard-coded `authors JOIN books`; one version ago it took any
    /// *pair*, which was a wall the moment a question needed three tables —
    /// and the kernel, the wire and all three SDKs had done chains for months.
    ///
    /// The whole body is written over `tables: Vec<TableDef>` rather than a
    /// left and a right, and only the last dozen lines care how many there
    /// are: **two lower onto [`JoinSpec`] and three or more onto
    /// [`ChainSpec`]**, because the kernel has `Join` and `Chain` as separate
    /// entry points with different plans. Deciding that here, once, at the end,
    /// is what keeps `WHERE`, `GROUP BY`, the select list and `ORDER BY` from
    /// each having a two-table and an n-table version to drift apart.
    ///
    /// What is still checked, because getting it wrong returns an empty result
    /// with no explanation: each `ON` must name one column of the table being
    /// joined and one of a table already read, and no table may appear twice.
    fn join_tail(
        &mut self,
        first: Input,
        list: &[SelectItem],
        star: bool,
        distinct: bool,
    ) -> Result<Statement, SqlError> {
        // `(earlier input, its column, this table's column)` per table after
        // the first — `JoinKey` in the joined space, and what both specs want.
        let mut inputs = vec![first];
        let mut keys: Vec<(u32, u32, u32)> = Vec::new();
        loop {
            let at = self.at();
            let next = self.input()?;
            if let Some(seen) = inputs
                .iter()
                .find(|i| i.name().eq_ignore_ascii_case(next.name()))
            {
                // Two inputs under one *name*, which is a refusal: every column
                // reference resolves against a name, so two inputs sharing one
                // make `id` and `zones.id` alike ambiguous with no way to say
                // which was meant. The same table twice under *different* names
                // is exactly what an alias is for and is fine — this compares
                // names, not tables, which is the whole difference.
                return Err(SqlError {
                    message: format!(
                        "`{}` is read twice under one name. Give one of them an alias: \
                         `{} AS something`",
                        seen.name(),
                        next.table.name()
                    ),
                    at,
                });
            }
            inputs.push(next);
            let key = self.join_key(&inputs)?;
            keys.push(key);
            if !self.eat_join() {
                break;
            }
        }

        let mut filters: Vec<Vec<FilterSpec>> = vec![Vec::new(); inputs.len()];
        let mut compute: Vec<ComputeSpec> = Vec::new();
        let mut aggregates: Vec<AggregateSpec> = Vec::new();
        let mut sort: Vec<SortSpec> = Vec::new();
        let mut having: Vec<FilterSpec> = Vec::new();
        let mut group_by: Vec<u32> = Vec::new();
        let mut limit: Option<u64> = None;
        let mut offset = 0;

        // WHERE is split by which table each column belongs to — the kernel
        // pushes each table's conditions into that table's own scan, which is
        // the difference between filtering 100,000 trips and filtering the
        // handful that survive. Sending them all to one input would still be
        // correct and would plan much worse.
        if self.eat("where") {
            loop {
                let at = self.at();
                let raw = self.name()?;
                // Through `resolve_side`, so a bare name that two tables share
                // warns here as it does in a select list. The two-table version
                // had its own copy of the qualified-or-not rule and no warning,
                // so `WHERE id = 1` over a join picked a side in silence.
                let (input, column) = self.resolve_side(&raw, &inputs, at)?;
                // `resolve_side` returns an input it found in this very slice,
                // so this lookup cannot miss. It is a lookup rather than
                // indexing anyway: the workspace forbids indexing here, and the
                // reason it does is that every byte reaching this parser is
                // something a visitor typed. A panic would be reachable from a
                // text box, so "cannot happen" is written as a refusal.
                let Some(mine) = filters.get_mut(input as usize) else {
                    return Err(SqlError {
                        message: format!("`{raw}` resolved to an input that is not there"),
                        at,
                    });
                };
                // Only the operator and the literal are left: the column was
                // resolved above and `condition` would resolve it again.
                //
                // That second resolution used to happen — this rewound a token
                // and called `condition`, which threw the ordinal away and
                // replaced it. Harmless while every input was a bare table, and
                // wrong the moment one had an alias: `condition` matches a
                // qualifier against the *table's* name, so `WHERE
                // pickup.borough = 'Manhattan'` came back as "`pickup.borough`
                // is not a column of `zones`" — about a column `zones`
                // certainly has, on a query that never named `zones`.
                let (op, value) = self.comparison_tail()?;
                mine.push(FilterSpec {
                    column,
                    op,
                    value,
                    ..FilterSpec::default()
                });
                if !self.eat("and") {
                    break;
                }
            }
        }

        if distinct {
            if self.peek_word().as_deref() == Some("group") {
                return Err(SqlError {
                    message: "DISTINCT and GROUP BY are the same request written twice. \
                              GROUP BY says which columns make a group; DISTINCT says the \
                              selected ones do. Keep one."
                        .to_owned(),
                    at: self.at(),
                });
            }
            // The same lowering the single-table path uses, over the joined
            // space: the keys are the selected columns and there are no
            // aggregates. `join_value_ordinal` refuses an aggregate item on
            // its own, but with a message about group keys — so the DISTINCT
            // case says what is actually wrong before reaching it.
            for item in list {
                let at = item.at();
                if matches!(item, SelectItem::Aggregate { .. }) {
                    return Err(SqlError {
                        message: "DISTINCT applies to the rows a query returns, and an \
                                  aggregate returns one row per group — there is nothing \
                                  left to deduplicate. Did you mean `count(distinct x)`?"
                            .to_owned(),
                        at,
                    });
                }
                let key = self.join_value_ordinal(item, &mut compute, &inputs, at)?;
                if !group_by.contains(&key) {
                    group_by.push(key);
                }
            }
        }
        if self.eat("group") {
            self.expect("by")?;
            loop {
                let at = self.at();
                // A select item rather than a bare column, so `GROUP BY
                // hour(pickup_time)` reaches the same find-or-add the
                // single-table path uses. A computed value is appended after
                // *every* table, which is where `join_value_ordinal` puts it.
                let item = self.select_item()?;
                let key = self.join_value_ordinal(&item, &mut compute, &inputs, at)?;
                // Deduplicated: `GROUP BY borough, borough` is one key written
                // twice, and keeping both would return one group per pair with
                // the column repeated in every row -- the same answer, wider,
                // and no error anywhere. `join_value_ordinal`'s find-or-add
                // already does this for a *computed* key; this is the same
                // rule for a stored one.
                if !group_by.contains(&key) {
                    group_by.push(key);
                }
                if !self.eat_symbol(",") {
                    break;
                }
            }
        }

        for item in list {
            match item {
                SelectItem::Aggregate { kind, argument, at } => {
                    aggregates.push(self.join_aggregate(
                        kind,
                        argument.as_deref(),
                        &inputs,
                        *at,
                    )?);
                }
                SelectItem::Call { at, .. } => {
                    // Registered by find-or-add, so `SELECT hour(t), count(*)
                    // ... GROUP BY hour(t)` names one computed column rather
                    // than two. It must already be the group key: a computed
                    // column beside a grouping that did not group by it is the
                    // same error a bare column gets below, for the same reason.
                    let ordinal = self.join_value_ordinal(item, &mut compute, &inputs, *at)?;
                    if !group_by.contains(&ordinal) {
                        return Err(SqlError {
                            message: format!(
                                "a computed column on a {} has to be one of the group keys \
                                 — a {} returns whole rows or one row per group, and there \
                                 is no third shape",
                                shape(&inputs),
                                shape(&inputs)
                            ),
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
                    // The check was `group_by.is_some()` and never compared the
                    // two ordinals. It went unnoticed because every test of a
                    // grouped join keyed on a *computed* column, which takes
                    // the branch above.
                    if !group_by.is_empty() {
                        let named = self
                            .resolve_side(raw, &inputs, *at)
                            .and_then(|(input, column)| joined_at(&inputs, input, column, *at))
                            .ok();
                        if !named.is_some_and(|ordinal| group_by.contains(&ordinal)) {
                            return Err(SqlError {
                                message: format!(
                                    "`{raw}` is not {} — a grouped query returns the keys \
                                     and the aggregates",
                                    if group_by.len() == 1 {
                                        "the group key"
                                    } else {
                                        "one of the group keys"
                                    }
                                ),
                                at: *at,
                            });
                        }
                    }
                }
            }
        }
        if group_by.is_empty() && !aggregates.is_empty() {
            return Err(SqlError {
                message: "an aggregate needs a GROUP BY".to_owned(),
                at: self.at(),
            });
        }
        if group_by.is_empty() && !star {
            // An ungrouped join or chain returns whole rows, so a named select
            // list has nowhere to go — and this used to *silently* be true.
            // The guard here was `!star && list.is_empty()`, which `select`
            // cannot produce: it sets `star` from a leading `*` and otherwise
            // parses at least one item, so the two conditions are never both
            // met and the check never fired once.
            //
            // What that let through is `SELECT title FROM authors JOIN books
            // ON ...`, which came back with all eight columns of both tables
            // and a header saying so. Not an error, not the projection asked
            // for: the single worst kind of wrong answer, because the header
            // is right about the rows and the rows are right about the
            // database and only the query has been ignored.
            //
            // Found by a chain test asserting the refusal, which is the
            // argument for writing the refusal tests for a shape you are
            // generalising rather than only the answers.
            return Err(SqlError {
                message: format!("a {} returns whole rows; write `SELECT *`", shape(&inputs)),
                at: self.at(),
            });
        }

        if self.eat("having") {
            if group_by.is_empty() {
                return Err(SqlError {
                    message: format!(
                        "HAVING needs a GROUP BY — it filters groups, and an ungrouped {} \
                         has none to filter. Did you mean WHERE?",
                        shape(&inputs)
                    ),
                    at: self.at(),
                });
            }
            // Parsed here, after the select list has been resolved, because
            // `HAVING count(*) > 100` names an aggregate by what the query
            // *computes* — the rule ORDER BY follows — and `aggregates` is not
            // populated until the loop above has run. The single-table clause
            // sits in the same place for the same reason.
            loop {
                let at = self.at();
                let item = self.select_item()?;
                let column = self.join_group_ordinal(
                    &item,
                    GroupSpace {
                        keys: &group_by,
                        aggregates: &aggregates,
                        compute: &compute,
                    },
                    &inputs,
                    at,
                    "HAVING",
                )?;
                let (op, value) = self.comparison_tail()?;
                having.push(FilterSpec {
                    column,
                    op,
                    value,
                    ..FilterSpec::default()
                });
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
            // Only over groups. Neither `Join` nor `Chain` has a sort field —
            // the kernel orders *groups* and not joined rows — so an ungrouped
            // one's ORDER BY has nowhere to be lowered, and the refusal now
            // says which of the two shapes the reader is in rather than "not
            // supported yet", which was true of both and explained neither.
            if group_by.is_empty() {
                return Err(SqlError {
                    message: format!(
                        "ORDER BY on a {} needs a GROUP BY: the kernel orders groups, not \
                         joined rows, so there is nothing to lower an ordering of whole rows \
                         onto",
                        shape(&inputs)
                    ),
                    at: self.at(),
                });
            }
            loop {
                let at = self.at();
                let item = self.select_item()?;
                let column = self.join_group_ordinal(
                    &item,
                    GroupSpace {
                        keys: &group_by,
                        aggregates: &aggregates,
                        compute: &compute,
                    },
                    &inputs,
                    at,
                    "ORDER BY",
                )?;
                let descending = if self.eat("desc") {
                    true
                } else {
                    self.eat("asc");
                    false
                };
                sort.push(SortSpec { column, descending });
                if !self.eat_symbol(",") {
                    break;
                }
            }
        }
        if self.eat("limit") {
            limit = Some(self.count("LIMIT")?);
        }
        if self.eat("offset") {
            offset = self.count("OFFSET")?;
        }

        // Two tables are a `Join`; three or more are a `Chain`. See
        // [`Statement::Chain`] for why that is a fork and not a length.
        // One slice pattern over all three, rather than three indexed
        // lookups: the two `left_where`/`right_where` fields are not
        // interchangeable, and `right_where` holding the left table's
        // conditions would push each predicate into the wrong scan and answer a
        // different question without erring anywhere. A pattern that names them
        // in order cannot make that mistake, where `filters[0]` and
        // `filters[1]` two lines apart can.
        if let ([left, right], [(_, left_key, right_key)], [left_where, right_where]) =
            (&inputs[..], &keys[..], &filters[..])
        {
            return Ok(Statement::Join(JoinSpec {
                left: left.table.name().to_owned(),
                right: right.table.name().to_owned(),
                left_alias: alias_of(left),
                right_alias: alias_of(right),
                left_key: *left_key,
                right_key: *right_key,
                left_where: left_where.clone(),
                right_where: right_where.clone(),
                having,
                compute,
                group_by,
                aggregates,
                sort,
                limit,
                offset,
            }));
        }

        let spec_inputs = inputs
            .iter()
            .enumerate()
            .zip(filters)
            .map(|((at, input), filters)| ChainInputSpec {
                table: input.table.name().to_owned(),
                // Empty when the query called the table by its own name, so a
                // spec without aliases is byte for byte what it was before this
                // existed — and a reader of the Spec tab sees an `alias` only
                // where there is one.
                alias: alias_of(input),
                // The first table joins to nothing; `keys[at - 1]` is the step
                // that *produced* table `at`, which is why the index is
                // shifted. Off by one here is a chain whose last table has no
                // key and whose second has two, which the binding refuses.
                on: at
                    .checked_sub(1)
                    .and_then(|step| keys.get(step))
                    .map(|&(input, column, own)| ChainOnSpec { input, column, own }),
                filters,
            })
            .collect();
        Ok(Statement::Chain(ChainSpec {
            inputs: spec_inputs,
            compute,
            having,
            group_by,
            aggregates,
            sort,
            limit,
            offset,
        }))
    }

    /// `ON <name> = <name>` for the input that was just added.
    ///
    /// Returns `(earlier input, its column, this table's column)`.
    ///
    /// Either order: `trips.pickup_zone = zones.id` and `zones.id =
    /// trips.pickup_zone` are the same join. The own side is resolved first and
    /// commits the orientation, which matters because the *earlier* side goes
    /// through `resolve_side` and may push an ambiguity warning — trying the
    /// earlier side first would warn about a name in an orientation the parser
    /// then abandoned.
    ///
    /// Any earlier table, not only the previous one, because `JoinKey` is in
    /// the joined space and always has been: `a JOIN b JOIN c ON a.x = c.y` is
    /// a chain the kernel plans, not a shape to refuse.
    fn join_key(&mut self, inputs: &[Input]) -> Result<(u32, u32, u32), SqlError> {
        let (own, earlier) = inputs
            .split_last()
            .expect("join_key is called with the new input already pushed");
        self.expect("on")?;
        let first_at = self.at();
        let first = self.name()?;
        self.expect_symbol("=")?;
        let second_at = self.at();
        let second = self.name()?;

        for (own_raw, own_at, other_raw, other_at) in [
            (&second, second_at, &first, first_at),
            (&first, first_at, &second, second_at),
        ] {
            let Ok(mine) = self.resolve_named(own_raw, &own.table, own.name(), own_at) else {
                continue;
            };
            if let Ok((input, column)) = self.resolve_side(other_raw, earlier, other_at) {
                return Ok((input, column, mine));
            }
        }

        Err(SqlError {
            message: if let [only] = earlier {
                format!(
                    "`{first} = {second}` does not name one column of `{}` and one of `{}`",
                    only.name(),
                    own.name()
                )
            } else {
                format!(
                    "`{first} = {second}` does not name one column of `{}` and one of a table \
                     read before it ({})",
                    own.name(),
                    name_list(earlier, "or")
                )
            },
            at: first_at,
        })
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
/// `` `a` ``, `` `a` or `b` ``, `` `a`, `b` or `c` `` — table names for an
/// error message, with `last` as the final conjunction.
///
/// A function because six messages and one warning all want it, and the
/// two-table versions each wrote out their own `"`{} or `{}`"`. That is six
/// places a third table would simply have been missing from, with no error
/// anywhere — the reader would be told a column is not on either of two tables
/// while looking at a query that names three.
fn name_list(inputs: &[Input], last: &str) -> String {
    let refs: Vec<&Input> = inputs.iter().collect();
    name_list_refs(&refs, last)
}

/// [`name_list`] over borrowed tables, for a caller that has a subset.
fn name_list_refs(inputs: &[&Input], last: &str) -> String {
    match inputs {
        [] => String::new(),
        [one] => format!("`{}`", one.name()),
        [head @ .., tail] => {
            let front = head
                .iter()
                .map(|i| format!("`{}`", i.name()))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{front} {last} `{}`", tail.name())
        }
    }
}

/// Where input `input`'s column `column` sits in the joined row: the widths of
/// every earlier table, summed, plus the column.
///
/// The only place the parser knows the joined layout, and it is written down.
/// The two-table version was `if input == 0 { 0 } else { left.columns().len() }`
/// inline in three places, which is both the same arithmetic three times and
/// the arithmetic that cannot be generalised by adding a table.
fn joined_at(inputs: &[Input], input: u32, column: u32, at: usize) -> Result<u32, SqlError> {
    let base: usize = inputs
        .iter()
        .take(input as usize)
        .map(|i| i.table.columns().len())
        .sum();
    u32::try_from(base + column as usize).map_err(|_| SqlError {
        message: "too many columns".to_owned(),
        at,
    })
}

/// "join" or "chain", for a refusal that would otherwise name the wrong one.
///
/// Every message in `join_tail` said "join", which is what it was. Telling a
/// reader who wrote three tables that "a join returns whole rows" names a
/// construct they did not write.
/// The alias a spec should carry for an input, or empty when there is none.
///
/// Empty rather than always the name, so a spec for a query that used no alias
/// is byte for byte the spec it was before aliases existed: the field is
/// `skip_serializing_if = "String::is_empty"`, the Spec tab shows an `alias`
/// only where the reader wrote one, and every JSON written against the old
/// shape still parses.
fn alias_of(input: &Input) -> String {
    if input.name().eq_ignore_ascii_case(input.table.name()) {
        String::new()
    } else {
        input.name().to_owned()
    }
}

fn shape(inputs: &[Input]) -> &'static str {
    if inputs.len() > 2 { "chain" } else { "join" }
}

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

impl SelectItem {
    /// Where in the source this item started.
    ///
    /// Every variant carries it already; this exists so that code walking a
    /// list of items does not have to match three ways to report against one.
    fn at(&self) -> usize {
        match self {
            Self::Column { at, .. } | Self::Aggregate { at, .. } | Self::Call { at, .. } => *at,
        }
    }
}
