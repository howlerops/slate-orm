//! The two tables the playground runs against.
//!
//! Deliberately the same `authors`/`books` shape the explorer demo and the
//! conformance runner use. Somebody who reads the demo, the site and this
//! should meet one schema, not three — and a reader comparing the playground's
//! `EXPLAIN` output to the README's examples is comparing like with like.
//!
//! `books.author_id` carries a secondary index, which is what makes the
//! playground worth having: filtering on it produces an index scan and
//! filtering on `title` produces a table scan, and the plan panel shows the
//! difference on the reader's own query rather than in a code block.

use slate_schema::{Catalog, IndexDef, IndexId, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

/// `authors`: id, name, country, born.
pub const AUTHORS: TableId = TableId(1);
/// `books`: id, author_id, title, year, price.
pub const BOOKS: TableId = TableId(2);

#[must_use]
pub fn authors() -> TableDef {
    TableDef::builder("authors", AUTHORS)
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .column("country", ValueType::Str)
        .column("born", ValueType::I64)
        .primary_key(["id"])
        .build()
        .expect("the authors schema is valid")
}

#[must_use]
pub fn books() -> TableDef {
    TableDef::builder("books", BOOKS)
        .column("id", ValueType::U64)
        .column("author_id", ValueType::U64)
        .column("title", ValueType::Str)
        .column("year", ValueType::I64)
        // In cents, at scale 2, so the workbench has a column where `19.99`
        // means what it says. Appended rather than inserted, so every ordinal
        // above it stays where the docs and the plan snapshots already say it
        // is.
        //
        // It is here because a SQL front end that reads `19.99` as an `f64`
        // and compares it to a decimal matches nothing and reports nothing,
        // and a fixture with no decimal column is a fixture that cannot show
        // the difference. The explorer's `books` grew the same column for the
        // same reason, one layer out.
        .decimal_column("price", 2)
        .primary_key(["id"])
        // The point of the whole exercise: with this, `author_id = 2` is an
        // index scan; without it, a table scan. The plan panel shows which.
        .index(IndexDef::builder("by_author", IndexId(10)).column("author_id"))
        // An inverted index, so `WHERE title CONTAINS 'earthsea'` has one to
        // reach and the schema tree has an index that is not an ordinary one.
        //
        // The planner will not choose it here and that is not a bug: there are
        // 4,848 books, and a non-covering index is worth taking at about one
        // row in 24,000 (`docs/full-text.md` measures it). The workbench has no
        // hint syntax, so every `CONTAINS` in it is a table scan and the plan
        // panel says so. The keyword still earns its place — it is a different
        // predicate, not a spelling of `LIKE`, and `LIKE '%game%'` matching
        // `Games` where this does not is the whole distinction.
        .index(
            IndexDef::builder("by_title_text", IndexId(11))
                .column("title")
                .text(),
        )
        .build()
        .expect("the books schema is valid")
}

#[must_use]
pub fn catalog() -> Catalog {
    Catalog::from_tables([authors(), books()]).expect("the fixture catalog is valid")
}

/// How many authors the generated tail adds beyond the named ones.
///
/// The size is not decoration. The planner is cost-based, and on a table of
/// twenty-four rows a full scan beats an index lookup plus point reads —
/// correctly. The first version of this fixture had exactly that, so
/// `author_id = 1` planned as a table scan and the playground could never have
/// shown the thing it exists to show. The tests caught it.
///
/// At this size a filter on `author_id` selects a few rows out of thousands,
/// the index wins on cost rather than on a hint, and the reader sees a real
/// decision. Still small enough to seed in a few milliseconds and to hold in a
/// browser tab without thinking about it.
const GENERATED_AUTHORS: u64 = 400;
/// Books per generated author.
const BOOKS_EACH: u64 = 12;

/// The six named authors, then a generated tail.
///
/// Named rows first so the table reads like something real when somebody
/// scrolls it, generated rows after so the statistics are worth having.
#[must_use]
pub fn author_rows() -> Vec<Row> {
    [
        (1_u64, "Ursula K. Le Guin", "US", 1929_i64),
        (2, "Italo Calvino", "IT", 1923),
        (3, "Octavia E. Butler", "US", 1947),
        (4, "Jorge Luis Borges", "AR", 1899),
        (5, "Ted Chiang", "US", 1967),
        (6, "Stanisław Lem", "PL", 1921),
    ]
    .into_iter()
    .map(|(id, name, country, born)| {
        Row::new(vec![
            Value::U64(id),
            Value::Str(name.to_owned()),
            Value::Str(country.to_owned()),
            Value::I64(born),
        ])
    })
    .chain((7..=6 + GENERATED_AUTHORS).map(|id| {
        const COUNTRIES: [&str; 8] = ["US", "IT", "AR", "PL", "JP", "NG", "FR", "BR"];
        Row::new(vec![
            Value::U64(id),
            Value::Str(format!("Author {id}")),
            Value::Str(
                COUNTRIES
                    .get((id as usize) % COUNTRIES.len())
                    .copied()
                    .unwrap_or("US")
                    .to_owned(),
            ),
            Value::I64(1900 + (id as i64 % 80)),
        ])
    }))
    .collect()
}

#[must_use]
pub fn book_rows() -> Vec<Row> {
    [
        (1_u64, 1_u64, "A Wizard of Earthsea", 1968_i64),
        (2, 1, "The Left Hand of Darkness", 1969),
        (3, 1, "The Dispossessed", 1974),
        (4, 1, "The Lathe of Heaven", 1971),
        (5, 2, "Invisible Cities", 1972),
        (6, 2, "If on a winter's night a traveler", 1979),
        (7, 2, "Cosmicomics", 1965),
        (8, 3, "Kindred", 1979),
        (9, 3, "Parable of the Sower", 1993),
        (10, 3, "Dawn", 1987),
        (11, 3, "Wild Seed", 1980),
        (12, 4, "Ficciones", 1944),
        (13, 4, "The Aleph", 1949),
        (14, 4, "Labyrinths", 1962),
        (15, 5, "Stories of Your Life and Others", 2002),
        (16, 5, "Exhalation", 2019),
        (17, 6, "Solaris", 1961),
        (18, 6, "The Cyberiad", 1965),
        (19, 6, "His Master's Voice", 1968),
        (20, 6, "The Futurological Congress", 1971),
        (21, 1, "Tehanu", 1990),
        (22, 2, "Mr Palomar", 1983),
        (23, 4, "The Book of Sand", 1975),
        (24, 5, "Tower of Babylon", 1990),
    ]
    .into_iter()
    .map(|(id, author, title, year)| {
        Row::new(vec![
            Value::U64(id),
            Value::U64(author),
            Value::Str(title.to_owned()),
            Value::I64(year),
            // Prices that straddle the round numbers a reader is likely to
            // type: ids 1..24 run 8.95 up to 24.70 in 68-cent steps, so
            // `price > 19.99` and `price >= 20.00` select different rows and a
            // query that quietly matched nothing would be obvious.
            Value::Decimal(895 + (id as i64 - 1) * 68),
        ])
    })
    .chain((0..GENERATED_AUTHORS * BOOKS_EACH).map(|n| {
        let author = 7 + n / BOOKS_EACH;
        let id = 25 + n;
        Row::new(vec![
            Value::U64(id),
            Value::U64(author),
            Value::Str(format!("Book {id}")),
            Value::I64(1950 + (n as i64 % 70)),
            Value::Decimal(500 + (n as i64 % 3_000)),
        ])
    }))
    .collect()
}
