//! What the gRPC head node costs.
//!
//! `slate-server` is the one component in this project with no performance
//! number attached to it. Its two tuning constants — the stream batch size and
//! the lease term — are argued for in comments and were never measured. This
//! crate measures them, and measures the thing every one of those arguments
//! rests on: what a request costs when it arrives over a socket instead of as a
//! function call.
//!
//! ```sh
//! cargo run --release -p slate-headbench --example head_report
//! cargo run --release -p slate-headbench --example head_report -- stream lease
//! ```
//!
//! # The shape
//!
//! Every measurement is the same three pieces:
//!
//! - A **real head node** on a loopback socket, over a **real SlateDB** on an
//!   in-memory object store. See [`harness`] for what that does and does not
//!   include.
//! - An **in-process control** built from the same catalog, the same rules and
//!   the same stores ([`harness::InProcess`]), so the difference between the
//!   two is the head node and nothing else.
//! - **Repeated runs**, reported as a median and a range ([`stats`]). A
//!   difference whose ranges overlap is printed as noise rather than as a
//!   number.
//!
//! # Why a separate crate
//!
//! It could be an example inside `slate-server`, next to the tests. It is not,
//! because the head node's dependency on a storage backend is a dev-dependency
//! there and this harness needs `slate-slatedb`, `slatedb` and an object store
//! as ordinary ones — and because a benchmark that can be built without
//! building the server's test tree is one that gets run.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
// A benchmark harness is allowed to be direct about failure: there is no caller
// to hand an error to, and a fixture that cannot be built has nothing to
// measure.
#![allow(clippy::expect_used)]

pub mod counting;
pub mod fixture;
pub mod harness;
pub mod load;
pub mod stats;
