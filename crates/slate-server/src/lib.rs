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
//! # What is not here
//!
//! No joins, chains, aggregates or computed columns on the wire, though the
//! kernel has all four. Each needs an ordinal space or a grouping model of its
//! own in the schema, and half of one would be worse than none. No read-only
//! transaction pinned to a replica, which [`ReplicaMode::Pinned`] would make
//! possible and which is the right way to serve a consistent multi-read export
//! — it needs a session type that is not a write transaction. No TLS or
//! connection limits: those belong to whatever fronts the server, and inventing
//! a second place to configure them makes the deployment worse.
//!
//! [`ReplicaMode::Pinned`]: https://docs.rs/slate-slatedb

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod auth;
pub mod convert;
pub mod leadership;
pub mod lease;
pub mod proto;
pub mod service;
pub mod session;
pub mod status;

pub use auth::{Authenticator, DenyEveryone, MetadataIdentity};
pub use leadership::{Cadence, Leadership, Standing, StepDown, maintain};
pub use lease::{Clock, Lease, LeaseError, ObjectStoreLease, SystemClock, Term};
pub use service::{Head, HeadConfig};
pub use session::{Limits, Sessions};
pub use status::{LEADER_KEY, code_for, from_kernel};
