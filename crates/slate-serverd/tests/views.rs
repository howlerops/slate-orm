//! Reading through a view, against the real binary.
//!
//! `tests/refusals.rs` covers declaring one — what is accepted, what will not
//! start. This file covers the half that only a running server can show: that
//! a read through a view sees the view's rows and not the table's, that the
//! caller's own filter composes with it rather than replacing it, and that
//! every path which has *not* opted in still refuses the name.
//!
//! That last group is the point of the file. `docs/views.md` §3a's whole
//! argument is that a view held outside the catalog is refused everywhere by
//! construction; an argument from construction is exactly the kind that stops
//! being true quietly, so it is asserted against a live server rather than
//! read off the source.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::needless_update,
    unreachable_pub
)]

mod harness;

use harness::{
    Files, Identity, Serving, compare, connect, proto, query, row, rows, str_value, u64_value,
};

/// One table, and a view over it that admits half the rows.
///
/// `kind = "note"` rather than something computed, because this file is about
/// where the predicate is applied and not about what a predicate can say —
/// `tests/refusals.rs` and the SQL front end's own suite cover the second.
const CONFIG: &str = r#"
[listen]
address = "127.0.0.1:0"

[auth]
mode = "trusted-header"

[shutdown]
grace = "2s"

[storage]
backend = "memory"

[[tables]]
name = "docs"
id = 1
columns = [
  { name = "id",   type = "u64" },
  { name = "kind", type = "str" },
]
primary_key = ["id"]

[[views]]
name = "notes"
query = "SELECT * FROM docs WHERE kind = 'note'"

[[security.grants]]
role = "app"
tables = ["docs"]
actions = ["everything"]
"#;

/// Three rows, two of which the view admits.
///
/// Two rather than one, so a read through the view returning a single row
/// cannot pass by accident — a predicate applied twice, or the wrong one
/// applied, would be indistinguishable at one row.
const SEED: &str = r#"
[[seed]]
table = "docs"
rows = [
  { id = 1, kind = "note" },
  { id = 2, kind = "memo" },
  { id = 3, kind = "note" },
]
"#;

const APP: Identity = Identity::app("u64:1", "u64:1");

/// Authenticated, and granted nothing.
///
/// A role name and not [`Identity::nobody`]: that one sends no principal at
/// all and gets `UNAUTHENTICATED`, which is a different answer to a different
/// question. The one this file needs is "you are somebody, and you may not
/// read this". Roles are not declared anywhere — a role is whatever a grant
/// names — so an ungranted name is a role with no grants.
const OUTSIDER: Identity = Identity {
    principal: "u64:2",
    tenant: Some("u64:1"),
    roles: "outsider",
    bearer: None,
};

fn serving(files: &Files) -> Serving {
    let config = files.write("head.toml", CONFIG);
    let seed = files.write("seed.toml", SEED);
    Serving::start(&[
        "--config",
        &config.display().to_string(),
        "--seed",
        &seed.display().to_string(),
    ])
}

fn ids(rows: &[proto::Row]) -> Vec<u64> {
    rows.iter()
        .map(|r| match r.values[0].kind.as_ref() {
            Some(proto::value::Kind::Uint64Value(n)) => *n,
            other => panic!("the first column should be the u64 id, got {other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn a_read_through_a_view_sees_the_views_rows() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let mut through = ids(&rows(&mut client, &APP, query("notes"))
        .await
        .expect("the view"));
    through.sort_unstable();
    assert_eq!(through, vec![1, 3]);

    // The same read against the table, to show the view is doing the work and
    // the seed is not simply two rows wide.
    let mut direct = ids(&rows(&mut client, &APP, query("docs"))
        .await
        .expect("the table"));
    direct.sort_unstable();
    assert_eq!(direct, vec![1, 2, 3]);
}

/// The caller's filter is ANDed with the view's, not substituted for it.
///
/// Asked the way a caller would get it wrong: a filter that *would* match a
/// row the view excludes. If the composition were a replacement — or an `OR` —
/// row 2 comes back, and a view would be a suggestion rather than a bound.
#[tokio::test]
async fn a_callers_filter_cannot_reach_past_the_view() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let mut narrowed = query("notes");
    narrowed.filter = Some(compare(0, proto::CmpOp::Le, u64_value(2)));
    let seen = ids(&rows(&mut client, &APP, narrowed)
        .await
        .expect("a filtered view"));
    assert_eq!(seen, vec![1], "row 2 is `memo`, which the view excludes");

    // And the caller's filter is not ignored either: without it the view
    // returns two rows, so a narrowing that did nothing would show here.
    let both = ids(&rows(&mut client, &APP, query("notes"))
        .await
        .expect("the view"));
    assert_eq!(both.len(), 2);
}

/// Ordinals are the base table's on both sides.
///
/// The composition is a plain `AND` only because a view may not narrow
/// columns; if it ever could, this is the test that would start returning the
/// wrong rows rather than failing to compile.
#[tokio::test]
async fn the_callers_ordinals_are_the_base_tables() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let mut by_kind = query("notes");
    by_kind.filter = Some(compare(1, proto::CmpOp::Eq, str_value("note")));
    let seen = ids(&rows(&mut client, &APP, by_kind)
        .await
        .expect("column 1 is `kind`"));
    assert_eq!(seen.len(), 2);

    // Column 1 named against the *view* would be `kind` too — the view has the
    // same shape — so the case that distinguishes them is a column the view's
    // own predicate reads. `kind = 'memo'` is refused by the view and asked
    // for by the caller, which can only be empty if both address the same
    // column.
    let mut contradiction = query("notes");
    contradiction.filter = Some(compare(1, proto::CmpOp::Eq, str_value("memo")));
    let none = rows(&mut client, &APP, contradiction)
        .await
        .expect("a query with no answer is not an error");
    assert!(none.is_empty(), "{none:?}");
}

/// Every path that has not opted in still refuses the name, and says why.
///
/// This is `docs/views.md` §3a asserted rather than argued. An insert, a
/// predicate delete, a point get, a join, an aggregate and an explain all
/// resolve through `Catalog::table_by_name`, so a view is not there to find —
/// and the day one of them starts resolving views too, this fails instead of
/// silently accepting a write through one, which §4 refuses.
///
/// Six of the paths, not all of them: `chain` shares `authorize_join_inputs`
/// with `join`, and the relation steps share `Head::table` with everything
/// else, so the argument that covers them is structural and the sample is what
/// is asserted. The structural half is that every one of them reaches
/// `Head::table`, which is the only function producing this refusal.
#[tokio::test]
async fn only_the_query_path_knows_what_a_view_is() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let insert = client
        .insert(APP.on(proto::InsertRequest {
            table: "notes".to_owned(),
            rows: vec![row(vec![u64_value(9), str_value("note")])],
            ..Default::default()
        }))
        .await
        .expect_err("a write through a view is refused; see `views.md` §4");
    assert_eq!(insert.code(), tonic::Code::NotFound, "{insert:?}");

    let delete = client
        .delete_where(APP.on(proto::DeleteWhereRequest {
            table: "notes".to_owned(),
            ..Default::default()
        }))
        .await
        .expect_err("a predicate delete through a view is refused too");
    assert_eq!(delete.code(), tonic::Code::NotFound, "{delete:?}");

    let explained = client
        .explain(APP.on(proto::ExplainRequest {
            query: Some(query("notes")),
            ..Default::default()
        }))
        .await
        .expect_err("explain has not opted in");
    assert_eq!(explained.code(), tonic::Code::NotFound, "{explained:?}");

    let joined = client
        .join(APP.on(proto::JoinRequest {
            join: Some(proto::JoinQuery {
                inputs: vec![
                    proto::JoinInput {
                        query: Some(query("docs")),
                        ..Default::default()
                    },
                    proto::JoinInput {
                        query: Some(query("notes")),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
            ..Default::default()
        }))
        .await
        .expect_err("a join naming a view is refused; `views.md` §1 has no answer for one");
    assert_eq!(joined.code(), tonic::Code::NotFound, "{joined:?}");

    let got = client
        .get(APP.on(proto::GetRequest {
            table: "notes".to_owned(),
            primary_key: Some(row(vec![u64_value(1)])),
            ..Default::default()
        }))
        .await
        .expect_err("a point get through a view is refused");
    assert_eq!(got.code(), tonic::Code::NotFound, "{got:?}");

    let grouped = client
        .aggregate(APP.on(proto::AggregateRequest {
            aggregate: Some(proto::AggregateQuery {
                input: Some(query("notes")),
                ..Default::default()
            }),
            ..Default::default()
        }))
        .await
        .expect_err("an aggregate over a view is refused");
    assert_eq!(grouped.code(), tonic::Code::NotFound, "{grouped:?}");

    // Every refusal above says *view*, not "no such table".
    //
    // Asserted on the message and not only on the code, because the code
    // cannot tell the two apart and the difference is the whole of what this
    // costs an operator: a name they declared themselves, reported as though
    // they had mistyped it. This is also the step-2 lesson applied — a test
    // that asserts a status code cannot see a change in what the status
    // code's message contains.
    for refusal in [&insert, &delete, &explained, &joined, &got, &grouped] {
        assert!(
            refusal.message().contains("`notes` is a view over `docs`"),
            "a declared view should be named as one, over its base table: {refusal:?}"
        );
    }

    // The table itself still explains, so the refusals above are about the
    // *name* and not about the handler being broken.
    client
        .explain(APP.on(proto::ExplainRequest {
            query: Some(query("docs")),
            ..Default::default()
        }))
        .await
        .expect("explain answers for a table");
}

/// A name that is neither a table nor a view still reads as a typo.
///
/// The contrast case for the test above, and the reason it is separate: the
/// two messages are one `if` apart, so a fixture asserting only "a view says
/// view" would pass just as well if *every* unknown name said it. What an
/// operator needs is the discrimination, not either half of it.
#[tokio::test]
async fn a_name_that_is_neither_gets_the_ordinary_refusal() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let refused = rows(&mut client, &APP, query("nowhere"))
        .await
        .expect_err("an unknown name is refused");
    assert_eq!(refused.code(), tonic::Code::NotFound, "{refused:?}");
    assert!(
        refused.message().contains("no table named `nowhere`"),
        "an unknown name is a typo, not a view: {refused:?}"
    );
    assert!(
        !refused.message().contains("is a view"),
        "and it must not be described as one: {refused:?}"
    );
    assert!(
        !refused.message().contains("docs"),
        "nor point at a table it has nothing to do with: {refused:?}"
    );
}

/// A caller with no grant on the base table cannot read through a view.
///
/// §2 says a view is not a privilege boundary, and the direction that matters
/// is this one: it must not be a privilege *escalation* either. The grant is
/// checked on `docs`, so a role granted nothing gets the same refusal it would
/// get naming the table.
#[tokio::test]
async fn a_view_grants_nothing_the_base_table_does_not() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let refused = rows(&mut client, &OUTSIDER, query("notes"))
        .await
        .expect_err("a caller with no grant reads nothing through a view");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied, "{refused:?}");
}

/// Security finding 8, on the view path.
///
/// The grant is checked *before* the request is converted, and the reason is
/// not that the rows would otherwise leak — the kernel authorises again inside
/// the planner and that is what protects them. It is that converting resolves
/// a `ColumnRef` against the table's width and **says the width in the
/// refusal**: "the projection names column 99 of table `docs`, which has 2
/// columns". A caller with no grant reads a schema out of the error messages,
/// one request at a time. `docs/security-review.md` finding 8 is that, and it
/// was fixed twice because the first fix covered the handlers it was written
/// about while the read paths went on doing it.
///
/// A view is a new way to reach that conversion, so it needs its own probe.
/// **Written because a mutation demanded it**: replacing
/// `authorized_table` with the bare `self.table` inside
/// `authorized_read_source` survived the four tests above, all of which assert
/// a status code that the kernel's own later check produces either way. The
/// difference between the two is *when* the refusal happens and therefore what
/// it can say, and only a probe shaped like the original finding can see it.
#[tokio::test]
async fn a_view_does_not_leak_the_base_tables_width_to_a_caller_with_no_grant() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let mut wide = query("notes");
    wide.projection = Some(proto::Projection {
        all_columns: false,
        columns: vec![proto::ColumnRef {
            input: 0,
            of: Some(proto::column_ref::Of::Column(99)),
        }],
    });
    let refused = rows(&mut client, &OUTSIDER, wide)
        .await
        .expect_err("column 99 does not exist, and the caller may not ask");

    // The code, and then the thing the code is standing in for.
    assert_eq!(refused.code(), tonic::Code::PermissionDenied, "{refused:?}");
    assert!(
        !refused.message().contains("2 columns"),
        "the refusal must not report the base table's width: {refused:?}"
    );
    assert!(
        !refused.message().contains("99"),
        "nor confirm which ordinals exist by naming the one that does not: {refused:?}"
    );
}
