//! Does the latency fixture actually let concurrent reads overlap?
//!
//! ```sh
//! cargo run --release -p slate-kernel --example concurrency_probe
//! ```
//!
//! The cost model charges `n` pipelined reads as `ceil(n / depth)` round
//! trips, which is only honest if reads issued together really do land
//! together. This asks the fixture directly, without an executor in the way.
//!
//! It exists because a measurement that disagreed with the model turned out to
//! be neither the model nor the engine: the fixture and the cost model held
//! different beliefs about how many rows a block holds, and every scan
//! comparison was confounded by it. When a plan measures worse than it costs,
//! run this before changing the planner.

#![allow(clippy::expect_used, clippy::print_stdout)]

use futures::stream::{FuturesUnordered, StreamExt as _};
use slate_kernel::latency::{LatencyProfile, LatencyStore};
use slate_kernel::memory::MemoryStore;
use slate_kernel::store::{KvReadStore, KvStore};
use std::time::Instant;

#[tokio::main]
async fn main() {
    let backing = MemoryStore::new();
    {
        let txn = backing.begin().await.expect("begin");
        for i in 0..1000u64 {
            txn.put(i.to_be_bytes().to_vec(), b"value".to_vec())
                .expect("put");
        }
        txn.commit().await.expect("commit");
    }
    let slow = LatencyStore::new(backing, LatencyProfile::object_storage());
    let snapshot = slow.snapshot().await.expect("snapshot");
    let snapshot = snapshot.as_ref();

    println!("raw concurrent gets through the fixture");
    println!("{:-<70}", "");
    for n in [1usize, 4, 8, 16, 32, 64, 100] {
        let started = Instant::now();
        let mut inflight = FuturesUnordered::new();
        for i in 0..n {
            let key = (i as u64).to_be_bytes();
            inflight.push(async move { snapshot.get(&key).await });
        }
        while let Some(result) = inflight.next().await {
            result.expect("get");
        }
        let elapsed = started.elapsed();
        println!(
            "{n:4} gets at once  {elapsed:>12?}   {:.3} ms each   speedup {:.1}x",
            elapsed.as_secs_f64() * 1000.0 / n as f64,
            n as f64 / (elapsed.as_secs_f64() * 1000.0),
        );
    }
}
