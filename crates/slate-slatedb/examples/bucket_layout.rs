//! What a record layer actually puts in a bucket.
//!
//! The workbench shows the *keyspace* — rows and index entries as keys in one
//! ordered map. That is the layer this repository owns. It is not what an
//! operator sees when they open the bucket: SlateDB batches those keys into
//! SSTs, a WAL and a manifest, and the object store holds files.
//!
//! This seeds the site's own 100,000-trip taxi sample into a real SlateDB
//! store over a real object store on local disk, then walks the result. The
//! listing it prints is a real listing, and `--json` emits it for the site so
//! the page can show a bucket nobody had to imagine.
//!
//! ```sh
//! cargo run --release -p slate-slatedb --example bucket_layout
//! cargo run --release -p slate-slatedb --example bucket_layout -- --json > site/data/bucket.json
//! ```
//!
//! `LocalFileSystem` rather than MinIO because the two are the same to
//! SlateDB — it writes objects through `object_store` either way — and one of
//! them needs no container. The paths and sizes below are what an S3 bucket
//! would hold; the only difference is the scheme in front of them.

use slate_kernel::RecordStore;
use slate_kernel::security::{Action, Grant, SecurityCatalog, SecurityContext};
use slate_slatedb::SlateStore;
use slatedb::Db;
use slatedb::object_store::local::LocalFileSystem;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Entry {
    path: String,
    bytes: u64,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let json = std::env::args().any(|a| a == "--json");
    let root = std::env::temp_dir().join(format!("slate-bucket-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;

    // The `Db` is opened here rather than through `SlateStore::open` so this
    // example can close it at the end. Closing is what turns a memtable into
    // objects; a listing taken before it is a true listing of a state nobody
    // ships, and a misleading picture of what the data costs at rest.
    let object_store = Arc::new(LocalFileSystem::new_with_prefix(&root)?);
    let db = Arc::new(Db::open("/records", object_store).await?);
    let kv = SlateStore::from_db(Arc::clone(&db));

    let trips = slate_wasm::taxi::trips();
    let zones = slate_wasm::taxi::zones();
    let security = SecurityCatalog::new()
        .grant(Grant::new(
            "app",
            slate_wasm::taxi::TRIPS,
            Action::EVERYTHING,
        ))
        .grant(Grant::new(
            "app",
            slate_wasm::taxi::ZONES,
            Action::EVERYTHING,
        ));
    let store = RecordStore::new(kv, slate_wasm::taxi::catalog(), security);
    let root_ctx = SecurityContext::superuser();

    let rows = slate_wasm::taxi::decode(&trip_bytes()?)?;
    if !json {
        eprintln!("seeding {} trips and {} zones…", rows.len(), 265);
    }
    let txn = store.begin().await?;
    txn.insert_many(&root_ctx, &zones, &slate_wasm::taxi::zone_rows())
        .await?;
    txn.insert_many(&root_ctx, &trips, &rows).await?;
    txn.commit().await?;

    drop(store);
    db.close().await?;

    let mut entries = Vec::new();
    walk(&root, &root, &mut entries)?;
    entries.sort_by(|a: &Entry, b: &Entry| a.path.cmp(&b.path));

    if json {
        // Emitted by hand rather than through serde: this crate has no serde
        // dependency and adding one so an example can print six lines of JSON
        // would be the wrong trade.
        let rows: Vec<String> = entries
            .iter()
            .map(|e| format!("  {{ \"path\": \"{}\", \"bytes\": {} }}", e.path, e.bytes))
            .collect();
        println!("[\n{}\n]", rows.join(",\n"));
    } else {
        let total: u64 = entries.iter().map(|e| e.bytes).sum();
        for entry in &entries {
            println!("{:>12}  {}", human(entry.bytes), entry.path);
        }
        println!("\n{} objects, {} total", entries.len(), human(total));
    }

    std::fs::remove_dir_all(&root)?;
    Ok(())
}

fn trip_bytes() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "site",
        "data",
        "trips.bin.gz",
    ]
    .iter()
    .collect();
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(path)?).read_to_end(&mut out)?;
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<Entry>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out)?;
        } else {
            out.push(Entry {
                path: path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned(),
                bytes: entry.metadata()?.len(),
            });
        }
    }
    Ok(())
}

fn human(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let n = bytes as f64;
    if bytes >= 1 << 20 {
        format!("{:.1} MB", n / (1u64 << 20) as f64)
    } else if bytes >= 1 << 10 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
