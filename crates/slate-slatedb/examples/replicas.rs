//! One writer, three replicas, and the two things that make that safe:
//! read-your-writes through a token, and tenant-affinity routing.
//!
//! ```sh
//! cargo run -p slate-slatedb --example replicas
//! ```

// An example is allowed to be direct about failure and about indexing a row it
// just built.
#![allow(clippy::expect_used, clippy::indexing_slicing)]

use slate_kernel::store::KvReadStore;
use slate_kernel::{
    Action, Expr, Freshness, Grant, Principal, ReadWatermark, RecordStore, ReplicaPool, ScanOrder,
    SecurityCatalog, SecurityContext,
};
use slate_schema::{Catalog, IndexDef, IndexId, Row, TableDef, TableId};
use slate_slatedb::{ReplicaMode, SlateReader, SlateStore};
use slate_tuple::{Value, ValueType};
use slatedb::config::DbReaderOptions;
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const PATH: &str = "/records";
const NOTES: TableId = TableId(1);

fn notes() -> TableDef {
    TableDef::builder("notes", NOTES)
        .column("tenant_id", ValueType::Uuid)
        .column("id", ValueType::U64)
        .column("title", ValueType::Str)
        .primary_key(["tenant_id", "id"])
        // The tenant leads the key, which is what makes both the security
        // boundary and the cache boundary physical.
        .tenant_column("tenant_id")
        .index(IndexDef::builder("by_title", IndexId(10)).column("title"))
        .build()
        .expect("valid schema")
}

fn note(tenant: Uuid, id: u64, title: &str) -> Row {
    Row::new(vec![
        Value::Uuid(tenant),
        Value::U64(id),
        Value::Str(title.to_owned()),
    ])
}

fn member(tenant: Uuid) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::Uuid(tenant))
            .with_role("member"),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
    let table = notes();
    let catalog = Catalog::from_tables([table.clone()])?;
    let security = SecurityCatalog::new().grant(Grant::new("member", NOTES, Action::ALL));

    // Exactly one writer. Opening a second would fence this one.
    let backend = Arc::new(SlateStore::open(PATH, Arc::clone(&object_store)).await?);
    let writer = RecordStore::new(Arc::clone(&backend), catalog.clone(), security.clone());

    // Replicas poll the manifest; the interval is most of their lag.
    let options = DbReaderOptions {
        manifest_poll_interval: Duration::from_millis(20),
        checkpoint_lifetime: Duration::from_secs(30),
        ..DbReaderOptions::default()
    };
    let mut replicas: Vec<Arc<dyn KvReadStore>> = Vec::new();
    for name in ["replica-a", "replica-b", "replica-c"] {
        replicas.push(Arc::new(
            SlateReader::open_with(
                name,
                PATH,
                Arc::clone(&object_store),
                ReplicaMode::Following,
                options.clone(),
            )
            .await?,
        ));
    }

    let pool = ReplicaPool::new(replicas, catalog, security)
        .with_writer(Arc::clone(&backend) as Arc<dyn KvReadStore>);

    let acme = Uuid::from_u128(1);
    let globex = Uuid::from_u128(2);

    // Write, and keep the token the commit hands back.
    let mut watermark = ReadWatermark::new();
    let token = writer
        .transact_tracked(async |txn| {
            let root = SecurityContext::superuser();
            txn.insert(&root, &table, &note(acme, 1, "quarterly plan"))
                .await?;
            txn.insert(&root, &table, &note(acme, 2, "retro")).await?;
            txn.insert(&root, &table, &note(globex, 1, "not yours"))
                .await?;
            Ok(())
        })
        .await?
        .1;
    watermark.observe_commit(token);
    println!("committed at sequence {:?}", token.map(|t| t.sequence()));

    // Reading with that token: a replica may only serve it once caught up, so
    // this is read-your-writes even though it is not the writer answering.
    let tenant = Value::Uuid(acme);
    let chosen = pool.route(watermark.freshness(), Some(&tenant)).await?;
    println!(
        "read-your-writes routed to {} (freshness can override affinity: the \n           preferred replica is skipped if it has not reached the token yet)",
        chosen.replica_name()
    );

    let snapshot = pool.snapshot(watermark.freshness(), Some(&tenant)).await?;
    let rows = snapshot
        .query(&member(acme), &table, Expr::True, ScanOrder::Ascending)
        .await?
        .collect()
        .await?;
    println!("Acme sees {} note(s):", rows.len());
    for row in &rows {
        println!("  {:?}", row.values()[2]);
    }
    drop(snapshot);

    // With nothing to prove, routing is pure affinity: the same tenant keeps
    // landing on the same replica, so its key range stays in that replica's
    // block cache.
    println!("\nsteady-state affinity (Freshness::Any):");
    for tenant_id in [1u128, 2, 3, 4, 5] {
        let value = Value::Uuid(Uuid::from_u128(tenant_id));
        let target = pool.route(Freshness::Any, Some(&value)).await?;
        println!("tenant {tenant_id} -> {}", target.replica_name());
    }

    // And the freshest possible read goes to the writer, because it is the only
    // view that can see a write before it reaches object storage.
    let latest = pool.route(Freshness::Latest, Some(&tenant)).await?;
    println!("Freshness::Latest -> {}", latest.replica_name());

    backend.close().await?;
    Ok(())
}
