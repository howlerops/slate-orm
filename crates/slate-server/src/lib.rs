//! The head node: a gRPC server over the record layer, and the leadership the
//! record layer deliberately does not provide.
//!
//! `slate-kernel` is a library. It has a writer and it has replicas, and it is
//! careful to say that deciding *which process* is the writer is not its job:
//! SlateDB fences but does not elect, so two processes that both want to write
//! will take turns fencing each other forever. This crate is the process that
//! decides, and the wire surface that goes with it.
//!
//! ```text
//!                    ┌──────────────────────────────┐
//!    gRPC ──────────►│            Head              │
//!                    │                              │
//!                    │  writes ─► leader? ─► writer │──► object storage
//!                    │  reads  ─► pool ─► replica   │◄── manifest poll
//!                    └──────────────┬───────────────┘
//!                                   │ compare-and-set
//!                             ┌─────▼─────┐
//!                             │   lease   │  one object, same bucket
//!                             └───────────┘
//! ```
//!
//! # The four decisions
//!
//! **Leadership is a lease over object storage, and it is not a safety
//! mechanism.** [`lease`] takes it with a conditional write, so exactly one
//! process wins a contested acquisition; but a paused process can still believe
//! in an expired lease, and no lease fixes that. Safety comes from SlateDB's
//! fence underneath. The lease is what stops the fencing happening over and
//! over. See [`lease`] for the argument in full.
//!
//! **Being fenced is terminal, and the node keeps serving reads anyway.**
//! [`leadership`] turns `WriterFenced` into a permanent step-down: writes are
//! refused locally, without a round trip to a store that will never answer
//! again. Reads carry on through the replica pool, because they were never
//! going to the writer in the first place. That is `handover.rs`'s finding
//! turned into behaviour: a takeover is an interruption, not a drain, and only
//! the replica path survives it.
//!
//! **A transaction is a task, not an entry in a map.** [`session`] keeps each
//! open transaction inside its own task, because `RecordTransaction` borrows
//! the store and the alternatives were a self-referential struct or leaking a
//! store per handover.
//!
//! **Identity comes from the transport, never from the request.** [`auth`]
//! builds the [`SecurityContext`](slate_kernel::SecurityContext); the `.proto`
//! has no principal field anywhere, and nothing in this crate can produce a
//! superuser.
//!
//! # Getting one running
//!
//! ```no_run
//! use slate_server::{Head, HeadConfig, auth::MetadataIdentity, lease::ObjectStoreLease,
//!                    leadership::{Cadence, Leadership, maintain}};
//! use slate_kernel::{KvReadStore, memory::MemoryStore};
//! use slate_schema::Catalog;
//! use slate_kernel::SecurityCatalog;
//! use std::sync::Arc;
//!
//! # async fn example(
//! #     catalog: Catalog,
//! #     security: SecurityCatalog,
//! #     object_store: Arc<dyn object_store::ObjectStore>,
//! #     writer: Arc<MemoryStore>,
//! #     replicas: Vec<Arc<dyn KvReadStore>>,
//! # ) -> Result<(), Box<dyn std::error::Error>> {
//! let lease = Arc::new(ObjectStoreLease::new(object_store, "leases/writer", "head"));
//! let term = lease.term_length();
//! let leadership = Leadership::new(lease);
//! tokio::spawn(maintain(Arc::clone(&leadership), Cadence::for_term(term)));
//!
//! let head = Head::new(
//!     HeadConfig::new(catalog, security),
//!     writer,
//!     replicas,
//!     leadership,
//!     // Correct only behind a proxy that sets the identity headers itself.
//!     Arc::new(MetadataIdentity::trusting_the_caller_completely()),
//! );
//!
//! tonic::transport::Server::builder()
//!     .add_service(head.into_service())
//!     .serve("0.0.0.0:50051".parse()?)
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! # One way to name a column, for every shape of row
//!
//! Joins, chains, aggregates and computed columns are all on the wire, and
//! between them they need three ordinal spaces: a joined row's columns come
//! from several tables, a computed value sits in a slot no table declares, and
//! a grouped row is its keys followed by its aggregates. The kernel packs all
//! three flat, by table width.
//!
//! The wire does not. A [`ColumnRef`](proto::ColumnRef) names a *producer* and
//! an index inside it — input 2's column 3, the first computed value, the
//! second aggregate — and [`convert::Space`] turns that into the kernel's flat
//! ordinal. The client never adds a width to anything, which matters for two
//! reasons beyond tidiness: this protocol deliberately does not publish table
//! widths, and an ordinal computed from one is silently re-pointed the day a
//! column is added to an earlier table.
//!
//! It also makes two refusals decidable that a flat ordinal makes
//! undecidable. A `HAVING` naming a column that is not grouped is a *kind*
//! mismatch rather than an ordinal that happens to be in range; so is a
//! computed value named from across a join, which the kernel's joined space
//! has no slot for. Both are refused with a message that says which.
//!
//! A join is one routing decision and one snapshot, however many tables it
//! reads, so every response still carries exactly one `served_by` and the
//! freshness a client asked for applies to the whole result. Two views would
//! be a join across two points in time.
//!
//! On the way back, a row's computed values are a list of their own rather than
//! a tail of its columns — the same split `JoinedRow` and `Group` already had,
//! and for the same reason: concatenated, reading the nth computed value means
//! `table_width + n`, which is the arithmetic this whole model exists to
//! remove.
//!
//! # Checking a client's schema without publishing one
//!
//! [`ColumnRef`](proto::ColumnRef) removed the arithmetic across tables and
//! left the ordinal within one, and a client outside Rust knows that ordinal
//! only because somebody wrote it down. [`fingerprint`] closes that: a request
//! may carry what the client believes the table's columns are, and a
//! declaration that is not this catalog's is refused before anything is read.
//!
//! It is an assertion, never a description — nothing here tells a client what
//! the schema is — and it is built so that every migration the schema layer
//! supports leaves an older client working, because a check that broke on a
//! migration would be turned off. See [`fingerprint`] for the whole argument.
//!
//! # A status code is lossy, so a reason travels with it
//!
//! `UNAVAILABLE` is four kernel errors wanting four different responses and
//! `ALREADY_EXISTS` is two, so [`status`] attaches a `google.rpc.ErrorInfo` —
//! a stable `reason` token and the error's own payload — to every status. A
//! client can then tell "that email address is taken" from "that id is taken"
//! without matching on prose that nothing tests.
//!
//! # What is not here
//!
//! No grouped join: the kernel groups over a single-table cursor, and
//! aggregating a join here would mean a second implementation of grouping in
//! the head node — over rows it had already streamed, losing the projection
//! narrowing that lets `COUNT(*)` read no columns at all. No `ORDER BY` over
//! groups either, for the same reason: the kernel has no ordering over groups
//! to be an oracle against, and a comparator written here would be a second
//! statement of the sort rules. Groups come back in the kernel's own order,
//! ascending by encoded key.
//!
//! No read-only transaction pinned to a replica, which [`ReplicaMode::Pinned`]
//! would make possible and which is the right way to serve a consistent
//! multi-read export — it needs a session type that is not a write
//! transaction. No TLS or connection limits: those belong to whatever fronts
//! the server, and inventing a second place to configure them makes the
//! deployment worse.
//!
//! [`ReplicaMode::Pinned`]: https://docs.rs/slate-slatedb

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod auth;
pub mod convert;
pub mod fingerprint;
pub mod leadership;
pub mod lease;
pub mod proto;
pub mod service;
pub mod session;
pub mod status;

pub use auth::{Authenticator, DenyEveryone, MetadataIdentity};
pub use fingerprint::of_table as schema_fingerprint;
pub use leadership::{Cadence, Leadership, Standing, StepDown, maintain};
pub use lease::{Clock, Lease, LeaseError, ObjectStoreLease, SystemClock, Term};
pub use service::{Head, HeadConfig};
pub use session::{Limits, Sessions};
pub use status::{DOMAIN, LEADER_KEY, code_for, from_kernel, reason_for};
