//! An end-to-end tour: schema, policies, writes, queries, and the two things
//! that are hard to get right — a tenant boundary that is physical, and a
//! policy that survives whichever access path the planner picks.
//!
//! ```sh
//! cargo run -p slate-orm --example multi_tenant
//! ```

use slate_orm::{
    Action, Catalog, CmpOp, Expr, Grant, Policy, Principal, Record, RecordStore, Records,
    ScanOrder, SecurityCatalog, SecurityContext, Value, memory::MemoryStore,
};
use uuid::Uuid;

const DOCUMENTS: slate_orm::TableId = slate_orm::TableId(1);

#[derive(Record, Debug, Clone, PartialEq)]
#[record(table = "documents", id = 1, version = 1, tenant = "tenant_id")]
#[record(index(
    name = "by_owner_recent",
    id = 12,
    columns("owner_id", desc("updated_at"))
))]
struct Document {
    // The tenant leads the primary key, which is what makes tenant scoping a
    // key prefix rather than a filter. The derive refuses to compile if it does
    // not.
    #[record(pk)]
    tenant_id: Uuid,
    #[record(pk)]
    id: u64,
    owner_id: Uuid,
    #[record(index(name = "by_slug", id = 10, unique))]
    slug: String,
    title: String,
    updated_at: i64,
    summary: Option<String>,
}

fn document(tenant: Uuid, id: u64, owner: Uuid, slug: &str, title: &str, at: i64) -> Document {
    Document {
        tenant_id: tenant,
        id,
        owner_id: owner,
        slug: slug.to_owned(),
        title: title.to_owned(),
        updated_at: at,
        summary: None,
    }
}

/// Authors may do anything to their own documents; editors may read everything
/// in their tenant.
fn security() -> SecurityCatalog {
    SecurityCatalog::new()
        .grant(Grant::new("author", DOCUMENTS, Action::ALL))
        .grant(Grant::new("editor", DOCUMENTS, [Action::Read]))
        .policy(Policy::new(
            "own_documents",
            DOCUMENTS,
            Action::ALL,
            |ctx: &SecurityContext| {
                Expr::eq(Document::COLUMNS.owner_id, ctx.principal().id.clone())
            },
        ))
        .policy(
            Policy::new("editors_read_all", DOCUMENTS, [Action::Read], |_: &_| {
                Expr::True
            })
            .for_role("editor"),
        )
}

fn user(id: Uuid, tenant: Uuid, role: &str) -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::Uuid(id))
            .with_tenant(Value::Uuid(tenant))
            .with_role(role),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let acme = Uuid::from_u128(1);
    let globex = Uuid::from_u128(2);
    let dana = Uuid::from_u128(100);
    let raj = Uuid::from_u128(200);

    let catalog = Catalog::from_tables([Document::table().clone()])?;
    let store = RecordStore::new(MemoryStore::new(), catalog, security());

    // Seed as superuser — the one explicit, greppable way past the policies.
    let root = SecurityContext::superuser();
    let txn = store.begin().await?;
    for doc in [
        document(acme, 1, dana, "q3-plan", "Q3 plan", 300),
        document(acme, 2, dana, "retro", "Retro", 200),
        document(acme, 3, raj, "budget", "Budget", 100),
        document(globex, 1, dana, "secret", "Globex secret", 400),
    ] {
        txn.insert_record(&root, &doc).await?;
    }
    txn.commit().await?;

    let dana_at_acme = user(dana, acme, "author");
    let editor_at_acme = user(raj, acme, "editor");

    let txn = store.begin().await?;

    // The policy restricts an author to their own rows, and the tenant term
    // keeps them inside Acme — even though Dana owns a Globex document too.
    let mine: Vec<Document> = txn
        .find_records(&dana_at_acme, Expr::True, ScanOrder::Ascending)
        .await?;
    println!("Dana sees {} document(s):", mine.len());
    for doc in &mine {
        println!("  {} ({})", doc.title, doc.slug);
    }

    // An editor sees the whole tenant, and still only that tenant.
    let all: Vec<Document> = txn
        .find_records(&editor_at_acme, Expr::True, ScanOrder::Ascending)
        .await?;
    println!("The editor sees {} document(s) in Acme", all.len());

    // Asking for another tenant explicitly cannot widen the scan: the tenant
    // term is conjoined, so a caller's filter can only narrow it further.
    let poached: Vec<Document> = txn
        .find_records(
            &editor_at_acme,
            Expr::eq(Document::COLUMNS.tenant_id, Value::Uuid(globex)),
            ScanOrder::Ascending,
        )
        .await?;
    println!("Asking for Globex returns {} document(s)", poached.len());

    // A filter on an indexed column routes through an index scan. Because the
    // tenant leads every index key, it is the policy's own tenant term that
    // makes the index usable here.
    let filter = Expr::eq(Document::COLUMNS.owner_id, Value::Uuid(dana)).and(Expr::compare(
        Document::COLUMNS.updated_at,
        CmpOp::Ge,
        Value::I64(250),
    ));
    let recent: Vec<Document> = txn
        .find_records(&dana_at_acme, filter, ScanOrder::Ascending)
        .await?;
    println!("Dana's documents updated at/after 250: {}", recent.len());

    // The unique index is enforced by key collision, so a duplicate slug is
    // refused rather than silently accepted.
    let clash = document(acme, 99, dana, "q3-plan", "Another Q3 plan", 500);
    match txn.insert_record(&dana_at_acme, &clash).await {
        Err(error) => println!("Duplicate slug refused: {error}"),
        Ok(()) => println!("BUG: the unique index let a duplicate through"),
    }

    // And a write that the policy would hide is refused up front, so a row can
    // never be stored that its writer could not read back.
    let planted = document(acme, 98, raj, "planted", "Planted", 500);
    match txn.insert_record(&dana_at_acme, &planted).await {
        Err(error) => println!("Write outside the policy refused: {error}"),
        Ok(()) => println!("BUG: the policy let a foreign row through"),
    }

    Ok(())
}
