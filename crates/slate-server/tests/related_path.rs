//! A path of relationships, resolved level by level in one request.
//!
//! # What this closes
//!
//! `slate-orm` has had `load_related_through`, `load_through` and
//! `load_nested` since P4, and every one of them was reachable only from Rust.
//! A Python, Go or TypeScript caller wanting an article's tags issued two
//! `Related` calls and regrouped by hand — the same shape as the N+1 this
//! whole layer exists to prevent, one level up.
//!
//! # Why a path needs a limit when `load_nested` did not
//!
//! `load_nested`'s own documentation argues, correctly, that it needs no depth
//! limit: each level is a *type parameter*, so the depth of a call is fixed
//! when it compiles and "a limit nobody can exceed is a limit nobody
//! maintains". It then names the form that would need one — "an `include` list
//! on the wire … is a string whose depth a request chooses".
//!
//! That is this. `repeated RelatedStep path` puts the depth in a message, one
//! step is one read, and so an unbounded path is a caller choosing how many
//! times the server goes to storage. `max_relation_depth` is the refusal that
//! comment predicted, and it is tested here rather than assumed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use slate_kernel::memory::MemoryStore;
use slate_kernel::{Action, Grant, RecordStore, SecurityCatalog, SecurityContext};
use slate_schema::{Catalog, ForeignKeyDef, IndexDef, IndexId, Row, TableDef, TableId};
use slate_server::HeadConfig;
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_tuple::{Value, ValueType};
use std::sync::Arc;
use tonic::Code;
use tonic::transport::Channel;

const ARTICLES: TableId = TableId(1);
const TAGS: TableId = TableId(2);
const ARTICLE_TAGS: TableId = TableId(3);

/// A many-to-many, which is the shape `through` exists for.
///
/// `articles → article_tags → tags` is two steps in *opposite* directions —
/// down to the join rows by the article they hold, then up to the tags they
/// point at — which is the case worth testing: a path that only ever went one
/// way would pass with the two directions confused.
fn articles() -> TableDef {
    TableDef::builder("articles", ARTICLES)
        .column("id", ValueType::U64)
        .column("title", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn tags() -> TableDef {
    TableDef::builder("tags", TAGS)
        .column("id", ValueType::U64)
        .column("name", ValueType::Str)
        .primary_key(["id"])
        .build()
        .expect("valid schema")
}

fn article_tags() -> TableDef {
    TableDef::builder("article_tags", ARTICLE_TAGS)
        .column("id", ValueType::U64)
        .column("article_id", ValueType::U64)
        .column("tag_id", ValueType::U64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_article", IndexId(30)).column("article_id"))
        .index(IndexDef::builder("by_tag", IndexId(31)).column("tag_id"))
        .foreign_key(ForeignKeyDef::builder("at_article", ARTICLES).column("article_id"))
        .foreign_key(ForeignKeyDef::builder("at_tag", TAGS).column("tag_id"))
        .build()
        .expect("valid schema")
}

fn catalog() -> Catalog {
    Catalog::from_tables([articles(), tags(), article_tags()]).expect("catalog")
}

fn security() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("app", ARTICLES, Action::EVERYTHING))
        .grant(Grant::new("app", TAGS, Action::EVERYTHING))
        .grant(Grant::new("app", ARTICLE_TAGS, Action::EVERYTHING))
        // Reads the join table and the articles but *not* the tags, so that a
        // path refused at its last step can be told from one refused at its
        // first.
        .grant(Grant::new("halfway", ARTICLES, [Action::Read]))
        .grant(Grant::new("halfway", ARTICLE_TAGS, [Action::Read]))
}

/// Articles 1 and 2; tags 10, 11, 12; article 1 has tags 10 and 11, article 2
/// has tag 11. Article 3 exists with no tags at all, which is the key that
/// must come back as *absent* rather than as an empty group.
async fn serving_with(depth: Option<usize>) -> common::Serving {
    let backing = Arc::new(MemoryStore::new());
    {
        // This test's own catalog, not `common::store`'s: that one is built
        // from the shared fixture, whose `docs` table already owns `TableId(1)`,
        // and seeding through it wrote these rows into the docs keyspace. The
        // symptom was a decode failure naming a table this file never mentions.
        let store = RecordStore::new(Arc::clone(&backing), catalog(), security());
        let context = SecurityContext::superuser();
        let txn = store.begin().await.unwrap();
        txn.insert_many(
            &context,
            &articles(),
            &[
                Row::new(vec![Value::U64(1), Value::Str("first".to_owned())]),
                Row::new(vec![Value::U64(2), Value::Str("second".to_owned())]),
                Row::new(vec![Value::U64(3), Value::Str("untagged".to_owned())]),
            ],
        )
        .await
        .unwrap();
        txn.insert_many(
            &context,
            &tags(),
            &[
                Row::new(vec![Value::U64(10), Value::Str("rust".to_owned())]),
                Row::new(vec![Value::U64(11), Value::Str("databases".to_owned())]),
                Row::new(vec![Value::U64(12), Value::Str("orphan".to_owned())]),
            ],
        )
        .await
        .unwrap();
        txn.insert_many(
            &context,
            &article_tags(),
            &[
                Row::new(vec![Value::U64(100), Value::U64(1), Value::U64(10)]),
                Row::new(vec![Value::U64(101), Value::U64(1), Value::U64(11)]),
                Row::new(vec![Value::U64(102), Value::U64(2), Value::U64(11)]),
            ],
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
    }
    let leadership = common::leader().await;
    common::serve(common::head_with(
        backing,
        Vec::new(),
        leadership,
        HeadConfig::new(catalog(), security()).with_limits(slate_server::Limits {
            max_relation_depth: depth,
            ..slate_server::Limits::default()
        }),
    ))
    .await
}

fn step(table: &str, key: &str, direction: pb::relation::Direction) -> pb::RelatedStep {
    pb::RelatedStep {
        relation: Some(pb::Relation {
            table: table.to_owned(),
            foreign_key: key.to_owned(),
            direction: direction as i32,
        }),
        schema: None,
    }
}

/// `article → article_tags → tags`, the many-to-many.
fn through() -> Vec<pb::RelatedStep> {
    vec![
        step(
            "article_tags",
            "at_article",
            pb::relation::Direction::Children,
        ),
        step("article_tags", "at_tag", pb::relation::Direction::Parents),
    ]
}

fn keys(values: &[u64]) -> Vec<pb::Value> {
    values
        .iter()
        .map(|v| slate_server::convert::value_to_proto(&Value::U64(*v)))
        .collect()
}

fn u64_at(row: &pb::Row, ordinal: usize) -> u64 {
    match row.values[ordinal].kind.as_ref().expect("a value") {
        pb::value::Kind::Uint64Value(v) => *v,
        other => panic!("expected a u64, got {other:?}"),
    }
}

async fn path_request(
    client: &mut RecordsClient<Channel>,
    path: Vec<pb::RelatedStep>,
    ids: &[u64],
) -> Result<pb::RelatedResponse, tonic::Status> {
    client
        .related(common::app(pb::RelatedRequest {
            transaction: String::new(),
            relation: None,
            keys: keys(ids),
            freshness: None,
            schema: None,
            path,
        }))
        .await
        .map(tonic::Response::into_inner)
}

/// Rebuild the tree the way a client must, and name the tags of one article.
///
/// This is the regrouping every SDK has to perform, written once here against
/// the wire shape: take the article's group in level 0, read each row's
/// `key_ordinal` column as level 1 names it, and look *that* up in level 1.
fn tags_of(response: &pb::RelatedResponse, article: u64) -> Vec<u64> {
    let level0 = &response.levels[0];
    let level1 = &response.levels[1];
    let mut out = Vec::new();
    for group in &level0.groups {
        if group.key.as_ref().map(|k| &k.kind) != Some(&Some(pb::value::Kind::Uint64Value(article)))
        {
            continue;
        }
        for row in &group.rows {
            let key = u64_at(row, level1.key_ordinal as usize);
            for far in &level1.groups {
                if far.key.as_ref().map(|k| &k.kind)
                    == Some(&Some(pb::value::Kind::Uint64Value(key)))
                {
                    out.extend(far.rows.iter().map(|r| u64_at(r, 0)));
                }
            }
        }
    }
    out.sort_unstable();
    out
}

#[tokio::test]
async fn a_two_step_path_resolves_a_many_to_many() {
    let serving = serving_with(Some(4)).await;
    let mut client = serving.client().await;
    let response = path_request(&mut client, through(), &[1, 2, 3])
        .await
        .expect("the path resolves");

    assert_eq!(response.levels.len(), 2, "one level per step");
    assert_eq!(tags_of(&response, 1), vec![10, 11]);
    assert_eq!(tags_of(&response, 2), vec![11]);
    // Article 3 has no join rows, so it is absent from level 0 rather than
    // present and empty — the same contract the single-level shape has.
    assert_eq!(tags_of(&response, 3), Vec::<u64>::new());
}

#[tokio::test]
async fn the_far_level_holds_one_group_per_distinct_key_not_one_per_parent() {
    let serving = serving_with(Some(4)).await;
    let mut client = serving.client().await;
    let response = path_request(&mut client, through(), &[1, 2])
        .await
        .expect("the path resolves");

    // Tag 11 is on both articles. Two parents sharing a far row get one group
    // between them, which is the saving the whole design exists for: repeating
    // the rows per parent would undo on the response what the deduplicated
    // read just bought.
    assert_eq!(
        response.levels[1].groups.len(),
        2,
        "tags 10 and 11, once each"
    );
    // And tag 12, which nothing points at, is not there at all.
    let names: Vec<u64> = response.levels[1]
        .groups
        .iter()
        .flat_map(|g| g.rows.iter().map(|r| u64_at(r, 0)))
        .collect();
    assert_eq!(names, vec![10, 11]);
}

#[tokio::test]
async fn a_path_of_one_step_answers_what_a_relation_answers() {
    // The two request shapes are one implementation, and this is what says so.
    // A `relation` request is turned into a path of one before anything reads,
    // so if these ever disagree the projection at the edge is wrong.
    let serving = serving_with(Some(4)).await;
    let mut client = serving.client().await;

    let one = vec![step(
        "article_tags",
        "at_article",
        pb::relation::Direction::Children,
    )];
    let by_path = path_request(&mut client, one.clone(), &[1, 2])
        .await
        .expect("the path resolves");
    let by_relation = client
        .related(common::app(pb::RelatedRequest {
            transaction: String::new(),
            relation: one[0].relation.clone(),
            keys: keys(&[1, 2]),
            freshness: None,
            schema: None,
            path: Vec::new(),
        }))
        .await
        .expect("the relation resolves")
        .into_inner();

    assert_eq!(by_path.levels.len(), 1);
    assert!(by_path.groups.is_empty(), "a path answers in `levels`");
    assert!(
        by_relation.levels.is_empty(),
        "a relation answers in `groups`"
    );
    assert_eq!(by_path.levels[0].groups, by_relation.groups);
}

#[tokio::test]
async fn a_path_deeper_than_the_limit_is_refused() {
    // One step is one read, so the depth is how many reads one request buys.
    let serving = serving_with(Some(1)).await;
    let mut client = serving.client().await;
    let error = path_request(&mut client, through(), &[1])
        .await
        .expect_err("two steps against a limit of one");
    assert_eq!(error.code(), Code::InvalidArgument);
    assert!(
        error.message().contains("max_relation_depth"),
        "the refusal names the knob: {}",
        error.message()
    );
}

#[tokio::test]
async fn the_depth_limit_is_off_by_nothing() {
    // The boundary, from both sides: a path of exactly the limit is allowed.
    // Without this, `>` and `>=` are the same test.
    let serving = serving_with(Some(2)).await;
    let mut client = serving.client().await;
    path_request(&mut client, through(), &[1])
        .await
        .expect("two steps against a limit of two");
}

#[tokio::test]
async fn a_path_that_does_not_compose_is_refused_by_name() {
    // Step two reads `article_tags` again, whose keys come from `articles` —
    // but step one returned `article_tags` rows. Nothing could be read from
    // that, and the refusal says which step and which tables.
    let serving = serving_with(Some(4)).await;
    let mut client = serving.client().await;
    let path = vec![
        step(
            "article_tags",
            "at_article",
            pb::relation::Direction::Children,
        ),
        step(
            "article_tags",
            "at_article",
            pb::relation::Direction::Children,
        ),
    ];
    let error = path_request(&mut client, path, &[1])
        .await
        .expect_err("a path that does not compose");
    assert_eq!(error.code(), Code::InvalidArgument);
    let message = error.message();
    assert!(message.contains("step 1"), "names the step: {message}");
    assert!(message.contains("compose"), "says why: {message}");
}

#[tokio::test]
async fn a_step_the_caller_may_not_read_is_refused_before_anything_is_read() {
    // `halfway` may read the articles and the join rows but not the tags. The
    // refusal has to arrive instead of the first level's rows, not after them:
    // a partial answer to a refused request is indistinguishable from a
    // complete one.
    let serving = serving_with(Some(4)).await;
    let mut client = serving.client().await;
    let error = client
        .related(common::as_principal(
            pb::RelatedRequest {
                transaction: String::new(),
                relation: None,
                keys: keys(&[1]),
                freshness: None,
                schema: None,
                path: through(),
            },
            "str:someone",
            None,
            "halfway",
        ))
        .await
        .expect_err("the last step is not readable");
    assert_eq!(error.code(), Code::PermissionDenied);
}

#[tokio::test]
async fn an_unreadable_step_is_refused_even_when_nothing_would_reach_it() {
    // This is the test that pins the *ordering*, and the one above does not.
    //
    // Article 3 has no join rows, so a path resolved lazily would find level 1
    // empty, skip level 2 entirely, and never discover that `halfway` may not
    // read the tags — answering successfully to a request it is not allowed to
    // make. Checking every step up front refuses it whatever the data holds,
    // which is the property, and only this fixture can tell the two apart: a
    // refusal is the answer either way when the path does have rows.
    let serving = serving_with(Some(4)).await;
    let mut client = serving.client().await;
    let error = client
        .related(common::as_principal(
            pb::RelatedRequest {
                transaction: String::new(),
                relation: None,
                keys: keys(&[3]),
                freshness: None,
                schema: None,
                path: through(),
            },
            "str:someone",
            None,
            "halfway",
        ))
        .await
        .expect_err("permission does not depend on how many rows there were");
    assert_eq!(error.code(), Code::PermissionDenied);
}

#[tokio::test]
async fn both_a_relation_and_a_path_is_refused_rather_than_resolved() {
    // Two ways to say what to read, saying different things. Picking one
    // silently is how a client bug reaches production.
    let serving = serving_with(Some(4)).await;
    let mut client = serving.client().await;
    let error = client
        .related(common::app(pb::RelatedRequest {
            transaction: String::new(),
            relation: through()[0].relation.clone(),
            keys: keys(&[1]),
            freshness: None,
            schema: None,
            path: through(),
        }))
        .await
        .expect_err("both set");
    assert_eq!(error.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn neither_a_relation_nor_a_path_is_refused() {
    let serving = serving_with(Some(4)).await;
    let mut client = serving.client().await;
    let error = client
        .related(common::app(pb::RelatedRequest {
            transaction: String::new(),
            relation: None,
            keys: keys(&[1]),
            freshness: None,
            schema: None,
            path: Vec::new(),
        }))
        .await
        .expect_err("neither set");
    assert_eq!(error.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn a_path_that_runs_dry_still_answers_one_level_per_step() {
    // Article 3 has no join rows, so level 1 has nothing to resolve. It is
    // still present and still empty: a client walking `levels` beside its own
    // `path` should not have to special-case the short answer.
    let serving = serving_with(Some(4)).await;
    let mut client = serving.client().await;
    let response = path_request(&mut client, through(), &[3])
        .await
        .expect("the path resolves");
    assert_eq!(response.levels.len(), 2);
    assert!(response.levels[0].groups.is_empty());
    assert!(response.levels[1].groups.is_empty());
}

#[tokio::test]
async fn no_keys_answers_one_empty_level_per_step() {
    // The same shape for the caller who had no parents at all, which takes a
    // different branch: this one never reads.
    let serving = serving_with(Some(4)).await;
    let mut client = serving.client().await;
    let response = path_request(&mut client, through(), &[])
        .await
        .expect("no keys is not an error");
    assert_eq!(response.levels.len(), 2);
    assert!(response.levels.iter().all(|l| l.groups.is_empty()));
    // Nothing served this, so nothing claims to have.
    assert!(response.served_by.is_none());
}

#[tokio::test]
async fn the_key_ordinal_names_the_column_that_relates_the_levels() {
    // `article_tags.tag_id` is ordinal 2, and that is the column a client
    // reads to find a join row's tag. Asserted directly, because every client
    // rebuilds the tree from this number and a wrong one would silently group
    // by `article_id` instead — which for this fixture would still produce
    // *some* answer.
    let serving = serving_with(Some(4)).await;
    let mut client = serving.client().await;
    let response = path_request(&mut client, through(), &[1])
        .await
        .expect("the path resolves");
    assert_eq!(response.levels[1].key_ordinal, 2, "article_tags.tag_id");
}
