//! Many clients at once: closed-loop load, percentiles, and who spent the CPU.
//!
//! Every other measurement in this crate is one request at a time, which makes
//! a median and a range enough. Under concurrency neither is enough on its own,
//! for two reasons.
//!
//! **A mean hides the thing being looked for.** Queueing shows up in the tail
//! long before it shows up in the middle, so [`Percentiles`] is what a run
//! reports and p99 is the column to read.
//!
//! **The client is on the same four cores as the server.** Past some
//! concurrency the number on the page is the box rather than the head node, and
//! saying where that happens needs more than a hunch. [`cpu_now`] reads
//! `/proc/self/task/*/schedstat` — nanoseconds of CPU per thread — and groups
//! it by thread name, so a run can report *how many cores it used and which
//! side of the socket used them*. The report gives the server's runtime and the
//! load generator's runtime different thread names precisely so this attribution
//! exists.
//!
//! The load is **closed loop**: each client waits for its own reply before
//! issuing the next request. That is what a client library does, and it means
//! offered load falls as the server slows rather than queueing without bound —
//! so these numbers cannot be read as an open-loop saturation curve. Where that
//! matters it is said again.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::time::{Duration, Instant};

// --- CPU accounting --------------------------------------------------------

/// Nanoseconds of CPU time, totalled and split by thread name.
///
/// Read from `/proc/self/task/<tid>/schedstat`, whose first field is the time
/// the thread has spent *on* a CPU in nanoseconds. That is the number wanted
/// here: wall time on a contended box says nothing about whether the work was
/// done or the thread was waiting for a core, and this distinguishes them.
#[derive(Debug, Clone, Default)]
pub struct Cpu {
    /// Nanoseconds per thread-name group.
    pub by_group: BTreeMap<String, u64>,
    /// Nanoseconds across every thread.
    pub total: u64,
    /// Threads seen.
    pub threads: usize,
}

/// Read the process's CPU time now.
///
/// Returns an empty reading if `/proc` is not there, which is reported as zero
/// cores rather than as an error: a missing `/proc` should cost the harness its
/// attribution column and nothing else.
#[must_use]
pub fn cpu_now() -> Cpu {
    let mut cpu = Cpu::default();
    let Ok(entries) = fs::read_dir("/proc/self/task") else {
        return cpu;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(schedstat) = fs::read_to_string(path.join("schedstat")) else {
            continue;
        };
        let Some(nanos) = schedstat
            .split_whitespace()
            .next()
            .and_then(|field| field.parse::<u64>().ok())
        else {
            continue;
        };
        let name = fs::read_to_string(path.join("comm"))
            .map(|comm| comm.trim().to_owned())
            .unwrap_or_else(|_| "?".to_owned());
        *cpu.by_group.entry(name).or_default() += nanos;
        cpu.total += nanos;
        cpu.threads += 1;
    }
    cpu
}

/// CPU spent between two readings, in cores.
#[derive(Debug, Clone, Default)]
pub struct CpuDelta {
    /// Seconds of CPU per thread-name group.
    pub by_group: BTreeMap<String, f64>,
    /// Seconds of CPU in total.
    pub total: f64,
    /// Wall time the reading covers.
    pub wall: Duration,
}

impl CpuDelta {
    /// Cores' worth of CPU used over the window.
    #[must_use]
    pub fn cores(&self) -> f64 {
        if self.wall.is_zero() {
            return 0.0;
        }
        self.total / self.wall.as_secs_f64()
    }

    /// Cores' worth used by threads whose name starts with `prefix`.
    #[must_use]
    pub fn cores_of(&self, prefix: &str) -> f64 {
        if self.wall.is_zero() {
            return 0.0;
        }
        let seconds: f64 = self
            .by_group
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(_, seconds)| *seconds)
            .sum();
        seconds / self.wall.as_secs_f64()
    }
}

/// The CPU spent between `before` and `after`, over `wall`.
///
/// A thread that exited inside the window takes its accumulated time out of the
/// later reading, which would make a group's delta negative. Those are clamped
/// to zero rather than allowed to subtract from the total: the alternative is a
/// negative core count, which is worse than a slightly low one.
#[must_use]
pub fn cpu_between(before: &Cpu, after: &Cpu, wall: Duration) -> CpuDelta {
    let mut delta = CpuDelta {
        wall,
        ..CpuDelta::default()
    };
    for (name, later) in &after.by_group {
        let earlier = before.by_group.get(name).copied().unwrap_or(0);
        let seconds = later.saturating_sub(earlier) as f64 / 1e9;
        if seconds > 0.0 {
            delta.by_group.insert(name.clone(), seconds);
            delta.total += seconds;
        }
    }
    delta
}

// --- percentiles -----------------------------------------------------------

/// The shape of a latency distribution, in nanoseconds.
#[derive(Debug, Clone, Copy, Default)]
pub struct Percentiles {
    /// Samples behind it.
    pub count: usize,
    /// The fastest.
    pub min: f64,
    /// The median.
    pub p50: f64,
    /// The ninetieth percentile.
    pub p90: f64,
    /// The ninety-ninth.
    pub p99: f64,
    /// The slowest.
    pub max: f64,
}

impl Percentiles {
    /// Summarise `samples`, which need not be sorted.
    #[must_use]
    pub fn of(samples: &mut [u64]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        samples.sort_unstable();
        let at = |fraction: f64| -> f64 {
            #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
            let index = (((samples.len() - 1) as f64) * fraction).round() as usize;
            samples.get(index).copied().unwrap_or(0) as f64
        };
        Self {
            count: samples.len(),
            min: at(0.0),
            p50: at(0.5),
            p90: at(0.9),
            p99: at(0.99),
            max: at(1.0),
        }
    }
}

// --- one load run ----------------------------------------------------------

/// What one window of concurrent load produced.
#[derive(Debug, Clone)]
pub struct LoadRun {
    /// Operations completed.
    pub ops: u64,
    /// Of those, how many reported failure.
    pub errors: u64,
    /// How long the window actually lasted.
    pub wall: Duration,
    /// Per-operation latency.
    pub latency: Percentiles,
    /// CPU spent over the window.
    pub cpu: CpuDelta,
}

impl LoadRun {
    /// Completed operations per second.
    #[must_use]
    pub fn throughput(&self) -> f64 {
        if self.wall.is_zero() {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let ops = self.ops as f64;
        ops / self.wall.as_secs_f64()
    }
}

/// Run `op` on every client at once until `window` has passed.
///
/// One task per client, each looping on its own connection and recording its
/// own latencies. `op` takes the client by value and hands it back, which
/// avoids a borrow crossing a spawn boundary; the `u64` is a per-client
/// sequence number, so a workload can vary the key it touches rather than
/// reading the same row a million times into a cache.
///
/// `op` returns `false` for an operation that failed. Failures are counted, not
/// hidden: a throughput figure taken while a tenth of the requests were being
/// refused is not a throughput figure, and the report prints the count beside
/// every row.
pub async fn drive<C, F, Fut>(clients: Vec<C>, window: Duration, op: F) -> LoadRun
where
    C: Send + 'static,
    F: Fn(C, u64) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = (C, bool)> + Send + 'static,
{
    let before = cpu_now();
    let started = Instant::now();
    let deadline = started + window;
    let mut handles = Vec::with_capacity(clients.len());
    for (index, client) in clients.into_iter().enumerate() {
        let op = op.clone();
        handles.push(tokio::spawn(async move {
            let mut client = client;
            let mut samples: Vec<u64> = Vec::with_capacity(8192);
            let mut errors: u64 = 0;
            // Distinct starting points so N clients do not walk the keyspace in
            // lockstep and share one hot block.
            let mut sequence = (index as u64).wrapping_mul(1_000_003);
            while Instant::now() < deadline {
                let at = Instant::now();
                let (returned, ok) = op(client, sequence).await;
                let elapsed = at.elapsed();
                client = returned;
                #[allow(clippy::cast_possible_truncation)]
                samples.push(elapsed.as_nanos() as u64);
                if !ok {
                    errors += 1;
                }
                sequence = sequence.wrapping_add(1);
            }
            (samples, errors)
        }));
    }

    let mut all: Vec<u64> = Vec::new();
    let mut errors: u64 = 0;
    for handle in handles {
        if let Ok((samples, failed)) = handle.await {
            all.extend(samples);
            errors += failed;
        }
    }
    let wall = started.elapsed();
    let after = cpu_now();
    LoadRun {
        ops: all.len() as u64,
        errors,
        wall,
        latency: Percentiles::of(&mut all),
        cpu: cpu_between(&before, &after, wall),
    }
}

// --- several runs ----------------------------------------------------------

/// Several [`LoadRun`]s of the same thing, reduced by taking the median of each
/// column independently.
///
/// Median of each column rather than "the median run", because the run with the
/// median throughput is not necessarily the one with the median p99, and
/// pretending otherwise picks one number to be honest about and lets the rest
/// ride along.
#[derive(Debug, Clone)]
pub struct Repeated {
    /// The runs, in the order they were taken.
    pub runs: Vec<LoadRun>,
}

fn median(values: &mut Vec<f64>) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        values.get(middle).copied().unwrap_or(f64::NAN)
    } else {
        f64::midpoint(
            values.get(middle - 1).copied().unwrap_or(f64::NAN),
            values.get(middle).copied().unwrap_or(f64::NAN),
        )
    }
}

impl Repeated {
    /// Collect runs.
    #[must_use]
    pub fn new(runs: Vec<LoadRun>) -> Self {
        Self { runs }
    }

    fn column(&self, pick: impl Fn(&LoadRun) -> f64) -> (f64, f64, f64) {
        let mut values: Vec<f64> = self.runs.iter().map(&pick).collect();
        let low = values.iter().copied().fold(f64::INFINITY, f64::min);
        let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (median(&mut values), low, high)
    }

    /// Median throughput, and the lowest and highest run.
    #[must_use]
    pub fn throughput(&self) -> (f64, f64, f64) {
        self.column(LoadRun::throughput)
    }

    /// Median p50, with the range across runs.
    #[must_use]
    pub fn p50(&self) -> (f64, f64, f64) {
        self.column(|run| run.latency.p50)
    }

    /// Median p90.
    #[must_use]
    pub fn p90(&self) -> (f64, f64, f64) {
        self.column(|run| run.latency.p90)
    }

    /// Median p99.
    #[must_use]
    pub fn p99(&self) -> (f64, f64, f64) {
        self.column(|run| run.latency.p99)
    }

    /// Median worst sample.
    #[must_use]
    pub fn max(&self) -> (f64, f64, f64) {
        self.column(|run| run.latency.max)
    }

    /// Median cores used across the whole process.
    #[must_use]
    pub fn cores(&self) -> f64 {
        self.column(|run| run.cpu.cores()).0
    }

    /// Median cores used by threads named with `prefix`.
    #[must_use]
    pub fn cores_of(&self, prefix: &str) -> f64 {
        self.column(|run| run.cpu.cores_of(prefix)).0
    }

    /// Operations across every run.
    #[must_use]
    pub fn ops(&self) -> u64 {
        self.runs.iter().map(|run| run.ops).sum()
    }

    /// Failures across every run.
    #[must_use]
    pub fn errors(&self) -> u64 {
        self.runs.iter().map(|run| run.errors).sum()
    }
}

// --- kernel TCP counters ---------------------------------------------------

/// One named counter from `/proc/net/netstat`'s `TcpExt` line.
///
/// System-wide rather than per-socket, which is the limitation: another process
/// making loopback connections while this runs is counted too. It is used here
/// only where the traffic under measurement dominates — a handful of packets per
/// operation on an otherwise quiet machine — and where the *difference between
/// two arms* is the claim rather than the absolute number.
///
/// `DelayedACKs` is the one that matters: it counts each time the kernel's
/// delayed-acknowledgement timer expires and sends an ACK that was being held
/// back. A sender with Nagle's algorithm enabled cannot put a second small
/// segment on the wire until that ACK arrives, so a stall of exactly that shape
/// leaves a fingerprint here.
#[must_use]
pub fn tcp_ext(name: &str) -> u64 {
    let Ok(contents) = fs::read_to_string("/proc/net/netstat") else {
        return 0;
    };
    let mut lines = contents.lines();
    while let Some(header) = lines.next() {
        if !header.starts_with("TcpExt:") {
            continue;
        }
        let Some(values) = lines.next() else {
            return 0;
        };
        let index = header.split_whitespace().position(|field| field == name);
        if let Some(index) = index {
            return values
                .split_whitespace()
                .nth(index)
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
        }
    }
    0
}
