//! Checking a client's copy of a table against this server's.
//!
//! # The hole this closes
//!
//! [`ColumnRef`](crate::proto::ColumnRef) removed the arithmetic *across*
//! tables: a client never adds a table width to an index, so a column added to
//! an earlier table cannot re-point a later reference. It did not remove the
//! ordinal *within* a table. A request still says "input 1's column 3", and a
//! client outside Rust knows `title` is column 3 only because somebody wrote
//! that down. The derive macro generates those constants from the same
//! declaration the server serves, so a Rust caller cannot disagree; a
//! hand-written table in another language can, and when it does the server
//! accepts the reference — it is a perfectly legitimate ordinal — and answers
//! a question about a different column. Same query, different answer, with no
//! error at any layer.
//!
//! So the client states what it believes and the server checks it. The check
//! is optional on the wire: a request without one is served exactly as before.
//!
//! # What is hashed
//!
//! Only what a client must restate in order to *address* a column, and only
//! what it can state: the table's name, each ordinal's column name and type in
//! order, a decimal's scale, and which ordinals form the primary key.
//!
//! **The scale is the one exception to that rule and it is deliberate.** It
//! addresses no column, so by the test below it does not belong — and the
//! failure it prevents is worse than the failure the test is about. A client
//! with an ordinal wrong reads the wrong column, which usually shows up as
//! nonsense. A client with a scale wrong reads the *right* column and renders
//! every value a power of ten out, consistently, for ever: the wire carries a
//! count of units and never the scale, so nothing downstream can notice. This
//! is the only place in the system where that can be caught.
//!
//! Hashing it is safe in the way that matters here, and that is what makes the
//! exception affordable rather than merely tempting: a scale cannot change
//! under a running client, because changing one is a refused migration. So it
//! cannot do what hashing a `CHECK` would — invalidate a fleet on an unrelated
//! schema change — since no such change exists to make. It is hashed only for
//! a decimal column, so a table without one hashes exactly as it did.
//!
//! Nullability, defaults, `CHECK`s and foreign keys are left out because none
//! of them addresses a column — a write that violates one is refused by name
//! at write time, which is a better error than a fingerprint mismatch, and
//! adding a `CHECK` must not invalidate every reader. Indexes are left out
//! because a hint is advice and an unusable one is already only a warning, so
//! adding an index for performance must not break a client that never names
//! it. Table ids, index ids and schema versions are left out because a client
//! cannot state them, and a fingerprint a client cannot compute is a constant
//! it has to be *told* — the schema on the wire by another route.
//!
//! # How it survives a migration
//!
//! The property comes from the schema layer rather than from here: **an
//! ordinal never moves**. A column is appended, a dropped column keeps its
//! ordinal for ever and holds nothing, and a rename records the previous name
//! and keeps resolving it. Every migration this project supports therefore
//! leaves every existing reference naming the same column, and this check is
//! built to agree:
//!
//! - **A column added** leaves the client's declaration a *prefix* of the
//!   table, and [`check`] hashes exactly that prefix. The old client keeps
//!   working across the deployment. This is the case that makes the whole
//!   design: a fingerprint over the whole catalog would take down every client
//!   in the fleet the moment anybody added a nullable column.
//! - **A column dropped** changes neither its ordinal nor its declaration, so
//!   the fingerprint does not change. A client still *writing* it is refused
//!   by name, which is the error it wanted.
//! - **A column renamed** is accepted under either name, because
//!   [`ColumnDef::answers_to`] is what the schema layer promises: code written
//!   against the old name keeps working. A check that broke on a rename would
//!   contradict the feature it is checking.
//! - **`DEFAULT`, `CHECK`, foreign keys** are not hashed at all.
//!
//! What it does refuse is a declaration that is *wrong*: a column inserted in
//! the middle, two same-typed columns swapped, a name that was never this
//! column's, a key of the wrong shape. Those are the ones that otherwise
//! return rows.
//!
//! # Why the client asserts rather than the server advertising
//!
//! The cheaper design is one opaque fingerprint on every response, compared by
//! the client at startup. It is rejected here because a mismatch is then
//! uninterpretable: the client cannot tell "your declaration is wrong" from
//! "the server has one more column than it did when you were written", and the
//! only safe reaction to an uninterpretable mismatch is to refuse to start —
//! which is the additive migration taking down the fleet, again. The party
//! that holds both statements is the server, so the claim travels to the
//! server, which can be exactly as tolerant as its own schema rules are, can
//! say which way the disagreement runs, and refuses *the request that would
//! have been wrong* rather than hoping somebody ran a startup check.

use crate::proto as pb;
use slate_schema::TableDef;
use tonic::{Code, Status};

/// FNV-1a's 64-bit offset basis.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a's 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// How many alias spellings of one declaration will be tried.
///
/// A renamed column has two acceptable spellings (its name and each previous
/// name), so a table with several renames has a product of them. The product
/// is one and two in every real schema, and the cap exists so that a
/// pathological one cannot turn a request into a hashing benchmark. Past it,
/// only the current names are accepted, and the refusal says so.
const MAX_SPELLINGS: usize = 64;

/// An FNV-1a hash being built, with the framing the canonical form uses.
///
/// Cloneable because the alias candidates share every byte up to the column
/// that has more than one name; branching is a clone rather than a re-hash
/// from the start.
#[derive(Debug, Clone, Copy)]
struct Fnv(u64);

impl Fnv {
    const fn new() -> Self {
        Self(FNV_OFFSET)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    /// A string, length-prefixed.
    ///
    /// Length-prefixed rather than delimited so that no column name can be
    /// spelled to look like the end of a field — `a\nb` and two columns must
    /// not hash alike.
    fn text(&mut self, text: &str) {
        self.bytes(text.len().to_string().as_bytes());
        self.bytes(b":");
        self.bytes(text.as_bytes());
    }

    /// A number, terminated.
    fn number(&mut self, n: usize) {
        self.bytes(n.to_string().as_bytes());
        self.bytes(b";");
    }
}

/// Every fingerprint this table would accept for a declaration of `columns`
/// columns.
///
/// More than one only where a column has been renamed: the client may be
/// spelling it either way, and both are correct.
fn accepted(table: &TableDef, name: &str, columns: usize) -> Vec<u64> {
    let mut start = Fnv::new();
    start.bytes(b"slate.v1.schema/1");
    start.text(name);

    let mut live = vec![start];
    for (ordinal, column) in table.columns().iter().take(columns).enumerate() {
        // The current name first, so that the truncation below keeps the
        // spelling a client written today would use.
        let mut names = Vec::with_capacity(1 + column.previous_names().len());
        names.push(column.name());
        names.extend(column.previous_names().iter().map(String::as_str));
        if live.len() * names.len() > MAX_SPELLINGS {
            names.truncate(1);
        }

        let mut next = Vec::with_capacity(live.len() * names.len());
        for state in &live {
            for name in &names {
                let mut state = *state;
                state.number(ordinal);
                state.text(name);
                state.text(column.value_type().name());
                // A decimal's scale, and only a decimal's.
                //
                // It addresses no column, which is the test every other
                // excluded property fails — and it is here anyway, because the
                // failure it prevents is worse than the one the test is about.
                // A client that has an ordinal wrong reads the wrong column
                // and usually notices; a client that has a scale wrong reads
                // the *right* column and renders every value a power of ten
                // out, consistently, for ever, with no error at any layer. The
                // wire carries units and never the scale, so nothing else in
                // the system can catch it.
                //
                // Hashing it is safe in the one way that matters here: a scale
                // cannot change under a running client, because changing one
                // is a refused migration (`changing_a_scale_is_a_migration
                // _refusal`). So this cannot do what hashing a `CHECK` would —
                // invalidate a fleet on an unrelated schema change — since
                // there is no such change to make.
                //
                // Only for a decimal, so a table without one hashes exactly as
                // it did and no client using such a table needs rebuilding.
                if let Some(scale) = column.scale() {
                    state.number(scale as usize);
                }
                // An array's element type, and only an array's, by exactly
                // the argument above one type over. It addresses no column;
                // a client that has it wrong reads the *right* column and
                // decodes every element as the wrong type, with the wire
                // carrying no element type to notice by. And it cannot change
                // under a running client for the same reason a scale cannot:
                // it is in the kernel's layout fingerprint too, so changing
                // one is a refused migration rather than a silent one.
                if let Some(element) = column.element_type() {
                    state.text(element.name());
                }
                next.push(state);
            }
        }
        live = next;
    }

    for state in &mut live {
        state.bytes(b"key");
        state.number(table.primary_key().len());
        for key in table.primary_key() {
            state.number(key.0);
        }
        state.bytes(b"columns");
        state.number(columns);
    }
    live.into_iter().map(|state| state.0).collect()
}

/// The fingerprint of a table's first `columns` columns, spelled with the
/// names it has now.
///
/// This is what a correct, up-to-date client computes. Exposed so that a test
/// — and a client library's own test suite, ported — can assert against a
/// known value rather than against whatever the implementation happens to
/// produce.
#[must_use]
pub fn of(table: &TableDef, columns: usize) -> u64 {
    // `accepted` puts the current-name spelling first, and there is always at
    // least one. The fallback keeps this total rather than indexing.
    accepted(table, table.name(), columns)
        .first()
        .copied()
        .unwrap_or(0)
}

/// The whole table's fingerprint, as a client declaring every column computes
/// it.
#[must_use]
pub fn of_table(table: &TableDef) -> u64 {
    of(table, table.columns().len())
}

/// Refuse a request whose client declares a different table from this one.
///
/// `None` is not a failure: the check is optional, and a client that sends no
/// claim is served as it always was.
pub fn check(table: &TableDef, claim: Option<&pb::SchemaCheck>) -> Result<(), Status> {
    check_named(table, table.name(), claim)
}

/// [`check`], for a request that reached `table` under a different name.
///
/// # Why a name is a parameter at all
///
/// The table's name is the first thing hashed, so a client's claim is a claim
/// about *the thing it called X*. For every path but one, X is the table's own
/// name and this is `check`. The exception is a read through a view: the
/// request says `classics`, the server resolves `books`, and a client that
/// declared `classics` — with the base table's columns, which is the only
/// declaration a view permits, because a view may not narrow them — hashes
/// under `classics` and could never match `books` however right it was.
///
/// Found by running the three-SDK conformance suite against a view rather than
/// by reading: the two clients that send no claim read through it and the one
/// that does was refused, so the disagreement reported itself as a client bug
/// three times before it reported itself as this.
///
/// Verifying under the name the caller used keeps the whole of what the check
/// is for. The alternative was to skip the check for a view, which would have
/// dropped the ordinal protection at exactly the point it is still needed —
/// a view's ordinals *are* the base table's, so a stale client declaration
/// misreads a view's rows precisely as it would misread the table's.
pub fn check_named(
    table: &TableDef,
    name: &str,
    claim: Option<&pb::SchemaCheck>,
) -> Result<(), Status> {
    let Some(claim) = claim else {
        return Ok(());
    };
    let declared = claim.columns as usize;
    let actual = table.columns().len();

    if declared > actual {
        return Err(Status::new(
            Code::InvalidArgument,
            format!(
                "the schema check on table `{}` declares {declared} columns and this table \
                 has {actual}: the client is addressing columns this catalog does not have. \
                 A client newer than the server is deployed the wrong way round.",
                name
            ),
        ));
    }

    // Before the comparison, not after. A prefix is accepted because every
    // ordinal in it still names the same column — but a declaration that stops
    // short of a key column cannot name this table's primary key at all, so it
    // is not a usable declaration of this table however its bytes hash, and
    // "the disagreement is inside those n columns" below would not be true of
    // it.
    if let Some(key) = table.primary_key().iter().find(|key| key.0 >= declared) {
        return Err(Status::new(
            Code::InvalidArgument,
            format!(
                "the schema check on table `{}` declares {declared} columns, which stops \
                 before the key column at ordinal {}: a declaration that short cannot name \
                 this table's primary key.",
                name, key.0
            ),
        ));
    }

    if accepted(table, name, declared).contains(&claim.fingerprint) {
        return Ok(());
    }

    // Deliberately not a diff. The client holds its own declaration and can
    // read it; what the server owes it is a refusal rather than a lesson, and
    // printing this table's columns back would be the `Describe` this protocol
    // refuses, delivered through an error message.
    let shorter = if declared < actual {
        format!(
            " The declaration covers the first {declared} of this table's {actual} columns, \
             which is allowed — a column added later does not invalidate an older client — \
             so the disagreement is inside those {declared}."
        )
    } else {
        String::new()
    };
    Err(Status::new(
        Code::InvalidArgument,
        format!(
            "the schema check on table `{}` does not match this catalog: the column names, \
             types or key positions the client declares are not this table's.{shorter} \
             Nothing was read or written. Fix the client's declaration; the ordinals it \
             would have sent name different columns here.",
            name
        ),
    ))
}

/// A claim a correct client makes about `table`, for tests and for the
/// in-process client the benchmarks use.
#[must_use]
pub fn claim(table: &TableDef) -> pb::SchemaCheck {
    pb::SchemaCheck {
        columns: table.columns().len() as u32,
        fingerprint: of_table(table),
    }
}

#[cfg(test)]
mod tests {
    //! The alias product, and the cap on it.
    //!
    //! `accepted` is where a renamed column becomes several acceptable
    //! fingerprints, and it had no test — the Go and TypeScript suites each
    //! cover *one* rename end to end, which exercises a product of one and
    //! says nothing about the multiplication or about what happens when it is
    //! cut off.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{MAX_SPELLINGS, accepted, check, check_named, claim, of_table};
    use slate_schema::{TableDef, TableId};
    use slate_tuple::ValueType;

    /// A table whose `kind` column has been renamed `renames` times, most
    /// recent last, and whose `label` column has been renamed once.
    fn table(renames: &[&str]) -> TableDef {
        let mut builder = TableDef::builder("papers", TableId(1))
            .column("id", ValueType::U64)
            .column("kind", ValueType::Str)
            .primary_key(["id"]);
        for previous in renames {
            builder = builder.renamed_column("kind", *previous);
        }
        builder.build().expect("valid schema")
    }

    /// The fingerprint a client computes for one spelling of `papers`.
    fn declared_as(name: &str) -> u64 {
        of_table(
            &TableDef::builder("papers", TableId(1))
                .column("id", ValueType::U64)
                .column(name, ValueType::Str)
                .primary_key(["id"])
                .build()
                .expect("valid schema"),
        )
    }

    /// A view's claim is verified against the base table, under the view's name.
    ///
    /// The case `check` alone cannot serve: a client reading through a view
    /// declares the view's *name* with the base table's columns — the only
    /// declaration a view permits, since a view may not narrow them — so it
    /// hashes under `classics` while the server holds `books`. Two assertions,
    /// because either alone is satisfied by a function that ignores the name.
    #[test]
    fn a_view_is_checked_under_the_name_the_caller_used() {
        let base = TableDef::builder("books", TableId(1))
            .column("id", ValueType::U64)
            .column("title", ValueType::Str)
            .primary_key(["id"])
            .build()
            .expect("valid schema");
        // What a client declaring the *view* computes: the base table's
        // columns, the view's name.
        let as_view = TableDef::builder("classics", TableId(2))
            .column("id", ValueType::U64)
            .column("title", ValueType::Str)
            .primary_key(["id"])
            .build()
            .expect("valid schema");
        let declared = Some(claim(&as_view));

        check_named(&base, "classics", declared.as_ref())
            .expect("a view's claim matches its base table under the view's name");
        let refused = check(&base, declared.as_ref())
            .expect_err("and does not match under the base table's own name");
        // The refusal names what the caller called it, not what it resolved
        // to: `check` was given `books`, so `books` is what it reports.
        assert!(refused.message().contains("`books`"), "{refused:?}");

        // The control in the other direction: the check still discriminates
        // under a view's name. A client declaring the wrong columns is refused
        // however right the name is.
        let wrong = TableDef::builder("classics", TableId(2))
            .column("id", ValueType::U64)
            .column("titel", ValueType::Str)
            .primary_key(["id"])
            .build()
            .expect("valid schema");
        check_named(&base, "classics", Some(claim(&wrong)).as_ref())
            .expect_err("a misspelled column is still caught under a view's name");
    }

    #[test]
    fn every_previous_spelling_of_a_column_is_accepted() {
        let table = table(&["category", "genre"]);
        let ok = accepted(&table, table.name(), table.columns().len());

        for spelling in ["kind", "category", "genre"] {
            assert!(
                ok.contains(&declared_as(spelling)),
                "a client declaring `{spelling}` is refused"
            );
        }
        // The control. Without it this passes against an `accepted` that
        // returned every u64 it could think of.
        assert!(
            !ok.contains(&declared_as("flavour")),
            "a name the table never had is accepted"
        );
    }

    /// The current spelling comes first, because `of` takes the first as *the*
    /// fingerprint — the one a correct client computes today.
    #[test]
    fn the_current_spelling_is_the_one_of_reports() {
        let table = table(&["category"]);
        assert_eq!(of_table(&table), declared_as("kind"));
        assert_ne!(of_table(&table), declared_as("category"));
    }

    /// Renames multiply, and the cap cuts the multiplication off.
    ///
    /// Past `MAX_SPELLINGS` the enumeration keeps only current names, so a
    /// table renamed pathologically often stops answering to its old ones.
    /// That is a deliberate cliff and this is where it is written down: the
    /// invariant that survives it is that the *current* spelling is always
    /// accepted, which is the one a client written today sends.
    #[test]
    fn the_alias_product_is_capped_but_never_drops_the_current_spelling() {
        // One column with n previous names yields n+1 spellings, so the cap
        // bites somewhere past MAX_SPELLINGS-1 renames.
        let many: Vec<String> = (0..MAX_SPELLINGS + 8).map(|i| format!("old{i}")).collect();
        let names: Vec<&str> = many.iter().map(String::as_str).collect();
        let table = table(&names);

        let ok = accepted(&table, table.name(), table.columns().len());
        assert!(
            ok.len() <= MAX_SPELLINGS,
            "{} spellings enumerated, cap is {MAX_SPELLINGS}",
            ok.len()
        );
        assert!(
            ok.contains(&declared_as("kind")),
            "the cap dropped the current spelling, which no client can avoid sending"
        );
    }

    /// Under the cap, the count is exactly the product.
    #[test]
    fn two_renames_give_three_spellings_and_no_more() {
        let table = table(&["category", "genre"]);
        assert_eq!(
            accepted(&table, table.name(), table.columns().len()).len(),
            3
        );
    }
}
