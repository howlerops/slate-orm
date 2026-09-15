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
use slatedb::admin::Admin;
use slatedb::config::{GarbageCollectorDirectoryOptions, GarbageCollectorOptions};
use slatedb::object_store::local::LocalFileSystem;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Where the database lives inside the bucket. Named once because the `Admin`
/// that collects the WAL has to be pointed at the same place the `Db` was.
const SLATE_PATH: &str = "/records";

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
    let db = Arc::new(Db::open(SLATE_PATH, object_store).await?);
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
    let rows_loaded = rows.len();
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

    // What the bucket holds the instant a bulk load closes, kept so the
    // difference can be reported rather than asserted.
    let mut loaded = Vec::new();
    walk(&root, &root, &mut loaded)?;
    let after_load: u64 = loaded.iter().map(|e| e.bytes).sum();

    // Closing flushes the memtable into an SST. It does not release the WAL
    // segment that carried the same rows, so a listing here shows the trips
    // twice — 21.7 MB for 11.0 MB of data — and reads as "this layer doubles
    // your storage bill".
    //
    // Running the collector at this point does not help, which is the part
    // worth knowing. SlateDB's WAL GC keeps every segment from the manifest's
    // `replay_after_wal_id` *inclusive* onwards, and after the load that
    // boundary is the very segment holding the trips. Reopening the database
    // does not move it either — it writes a fence at the next id and leaves
    // the boundary where it was. Only a write past the boundary releases it.
    //
    // So the load is followed by one: the last trip row written over itself.
    // That is not a trick to make the number smaller, it is what any database
    // that is still being used does within a second of finishing a load, and
    // the resting size of one that never writes again is a number nobody
    // needs. It is a real row through the real record layer, so what it costs
    // — a 296-byte SST and a 196-byte WAL segment — is in the listing too.
    let object_store = Arc::new(LocalFileSystem::new_with_prefix(&root)?);
    let db = Arc::new(Db::open(SLATE_PATH, Arc::clone(&object_store) as _).await?);
    let store = RecordStore::new(
        SlateStore::from_db(Arc::clone(&db)),
        slate_wasm::taxi::catalog(),
        SecurityCatalog::new().grant(Grant::new(
            "app",
            slate_wasm::taxi::TRIPS,
            Action::EVERYTHING,
        )),
    );
    let txn = store.begin().await?;
    let Some(last) = rows.last() else {
        return Err("the trip file decoded to nothing".into());
    };
    txn.update(&root_ctx, &trips, last).await?;
    txn.commit().await?;
    drop(store);
    db.close().await?;

    // `min_age` is zero because these objects are seconds old and the default
    // threshold exists to stop a *running* system collecting a segment a slow
    // reader still needs. Nothing is open on this store — both handles are
    // closed — so there is no reader to protect. Do not copy this setting into
    // anything that serves traffic.
    let reclaimable = GarbageCollectorDirectoryOptions {
        interval: None,
        min_age: Duration::ZERO,
        dry_run: false,
    };
    Admin::builder(
        SLATE_PATH,
        Arc::new(LocalFileSystem::new_with_prefix(&root)?),
    )
    .build()
    .run_gc_once(GarbageCollectorOptions {
        manifest_options: Some(reclaimable),
        wal_options: Some(reclaimable),
        ..GarbageCollectorOptions::default()
    })
    .await?;

    let mut entries = Vec::new();
    walk(&root, &root, &mut entries)?;
    entries.sort_by(|a: &Entry, b: &Entry| a.path.cmp(&b.path));

    if json {
        // Emitted by hand rather than through serde: this crate has no serde
        // dependency and adding one so an example can print six lines of JSON
        // would be the wrong trade.
        let rows: Vec<String> = entries
            .iter()
            .map(|e| format!("    {{ \"path\": \"{}\", \"bytes\": {} }}", e.path, e.bytes))
            .collect();
        // The provenance block is what makes the committed copy checkable.
        //
        // A regenerated listing can never be compared object for object: the
        // SST names are ULIDs minted at write time and the byte counts move
        // with SlateDB's block packing. So the listing is not what gets
        // checked — *what it is a listing of* is. `bucket_provenance.rs`
        // recomputes these four numbers from the schema and the committed
        // sample in milliseconds, with no SlateDB and no object store, and
        // fails when they no longer match. That is precisely the drift the
        // caveat named: "change the schema or the row count and it silently
        // describes the old thing".
        println!("{{");
        println!("  \"provenance\": {{");
        println!("    \"trips\": {rows_loaded},");
        println!("    \"zones\": {},", slate_wasm::taxi::zone_rows().len());
        println!(
            "    \"schema\": \"{}\",",
            slate_wasm::taxi::schema_fingerprint()
        );
        println!(
            "    \"command\": \"cargo run --release -p slate-slatedb \
             --example bucket_layout -- --json\""
        );
        println!("  }},");
        println!("  \"objects\": [");
        println!("{}", rows.join(",\n"));
        println!("  ]");
        println!("}}");
    } else {
        let total: u64 = entries.iter().map(|e| e.bytes).sum();
        for entry in &entries {
            println!("{:>12}  {}", human(entry.bytes), entry.path);
        }
        println!("\n{} objects, {} total", entries.len(), human(total));
        println!(
            "({} before the WAL boundary moved and the collector ran)",
            human(after_load)
        );
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
