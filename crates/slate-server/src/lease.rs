//! Electing a writer.
//!
//! SlateDB fences but does not elect. Two head nodes that both decide to be the
//! writer will take turns fencing each other and neither will make progress —
//! each one's first write kills the other, and each one restarts and does it
//! again. Nothing in the storage layer breaks that cycle, because from its side
//! both processes are behaving correctly. Something outside it has to decide,
//! once, which of them is the writer. That is this module.
//!
//! # What a lease over object storage can and cannot promise
//!
//! It is worth being blunt about this, because a lease is the classic thing to
//! believe more of than it says.
//!
//! **It cannot promise mutual exclusion.** The holder checks an expiry against
//! its own clock, and between that check and the write it may be descheduled,
//! garbage-collected, or paused by its hypervisor for longer than the whole
//! lease. Its clock may also simply disagree with the next node's. So there are
//! moments when two processes both believe they hold the lease, and no amount
//! of care here removes them — this is Kleppmann's objection to lease-based
//! locking, and it applies in full.
//!
//! **What it does promise is liveness, and an ordering.** Exactly one process
//! wins each contested acquisition, because the write that takes the lease is a
//! compare-and-set against the object store, not a read followed by a write.
//! And each acquisition raises a generation number that never repeats, so the
//! two processes that briefly overlap can always be ordered.
//!
//! Safety comes from underneath: SlateDB fences the old writer, atomically, at
//! the storage layer, and a fenced writer cannot commit — or read — again. The
//! lease is what stops the fencing from happening over and over. That division
//! is the whole design: **the lease decides who tries, the fence decides who
//! wins.** [`Leadership`](crate::leadership::Leadership) is wired to believe the
//! fence over the lease, and steps down on
//! [`KernelError::WriterFenced`](slate_kernel::KernelError::WriterFenced) even
//! while its own lease still looks valid.
//!
//! # Why object storage rather than a lock service
//!
//! etcd or ZooKeeper would give a better lease — a real session, revocation,
//! and a fencing token the storage layer could check. They would also be a
//! second stateful system to run, and the deployment this project targets is a
//! bucket. Object storage already offers the one primitive a lease needs, a
//! conditional write, on S3, R2, Tigris, GCS and MinIO alike. So the lease goes
//! where the database already is.
//!
//! The cost of that choice is the paragraph above: no revocation, so a stale
//! holder is bounded only by its expiry, and the fence has to be the backstop.
//! With a lock service it would still have to be, because SlateDB will not
//! check anyone else's token.
//!
//! # The stored object
//!
//! Deliberately plain text, three lines and a header. An operator debugging a
//! stuck cluster at three in the morning should be able to `cat` the lease and
//! know who holds it, not reach for a decoder.

use async_trait::async_trait;
use bytes::Bytes;
use object_store::{
    Attributes, ObjectStore, ObjectStoreExt as _, PutMode, PutOptions, TagSet, UpdateVersion,
    path::Path,
};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

/// A term of leadership: who holds the lease, under which generation, until
/// when.
///
/// The expiry is the holder's own claim, written when the term was last
/// extended. A reader compares it against its own clock, which is why the
/// module docs are careful about what that comparison proves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// Raised by one on every acquisition, never reused, never lowered.
    ///
    /// A renewal keeps the generation: a term that is extended is the same
    /// term. That is what lets two overlapping writers be ordered — the
    /// generation names the term, not the heartbeat.
    pub generation: u64,
    /// The holder's identity, as it wrote it.
    pub holder: String,
    /// When the holder said the term runs out.
    pub expires_at: SystemTime,
}

impl Term {
    /// Whether the term has not yet run out, by `now`.
    #[must_use]
    pub fn is_live_at(&self, now: SystemTime) -> bool {
        self.expires_at > now
    }
}

/// What went wrong taking, keeping or giving up a lease.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LeaseError {
    /// Someone else holds a live lease.
    ///
    /// Not a failure: this is the answer a follower expects, and the holder is
    /// named so the follower can tell a client where to send its writes.
    #[error("the lease is held by `{holder}` until {expires_at:?}")]
    Held {
        /// Who holds it.
        holder: String,
        /// When their term runs out, by their clock.
        expires_at: SystemTime,
    },

    /// This client held the lease and does not any more.
    ///
    /// Terminal for the term it names. A client that saw this must not write
    /// again on the strength of that term, whatever its own clock says.
    #[error("lost the lease: generation {generation} is now held by `{holder}`")]
    Lost {
        /// Who holds it now.
        holder: String,
        /// The generation this client thought it held.
        generation: u64,
    },

    /// Renew or release was called by a client that never acquired.
    #[error("this client does not hold the lease")]
    NotHeld,

    /// The lease object exists but is not a lease.
    ///
    /// Reported rather than overwritten. The object is at a path an operator
    /// configured, and quietly stamping over an unrecognised file there is how
    /// a lease path that was misconfigured onto something important becomes a
    /// data-loss incident.
    #[error("the lease object is malformed: {detail}")]
    Malformed {
        /// What did not parse.
        detail: String,
    },

    /// The store backing the lease failed.
    #[error("lease storage: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Where the current time comes from.
///
/// Injectable because every interesting property of a lease is about expiry,
/// and a test that establishes them by sleeping is a test that is slow when it
/// passes and flaky when the machine is busy. With a clock the test can hold
/// the lease at the exact instant it expires.
pub trait Clock: fmt::Debug + Send + Sync + 'static {
    /// The current time.
    fn now(&self) -> SystemTime;
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Taking, keeping and giving up the right to be the writer.
///
/// A `Lease` is one client's handle, not a shared registry: it remembers what
/// *this* process holds, so [`Lease::renew`] and [`Lease::release`] need no
/// arguments and cannot be handed the wrong term.
///
/// Implementations must make [`Lease::acquire`] a compare-and-set. A read
/// followed by a write looks identical in every single-threaded test and loses
/// the only property the trait exists to provide; `tests/lease.rs` runs both
/// against the same harness to show the difference.
#[async_trait]
pub trait Lease: fmt::Debug + Send + Sync + 'static {
    /// Try to become the holder.
    ///
    /// Succeeds when the lease is free or expired, and returns the term this
    /// client now holds. Returns [`LeaseError::Held`] when someone else's term
    /// is still live — that is an ordinary answer, not a fault.
    async fn acquire(&self) -> Result<Term, LeaseError>;

    /// Extend the term this client holds, keeping its generation.
    ///
    /// Returns [`LeaseError::Lost`] if the lease moved on, which is the only
    /// way a well-behaved holder finds out it was replaced while it was idle.
    async fn renew(&self) -> Result<Term, LeaseError>;

    /// Give the lease up so a successor does not have to wait out the expiry.
    ///
    /// Best effort by nature: a process that crashes cannot call it, which is
    /// why the expiry exists at all.
    async fn release(&self) -> Result<(), LeaseError>;

    /// What the store says, regardless of what this client believes.
    ///
    /// `None` when no lease object exists yet.
    async fn observe(&self) -> Result<Option<Term>, LeaseError>;

    /// The term this client believes it holds.
    ///
    /// Believes: it is not re-read, so it can be out of date in exactly the
    /// way the module docs describe.
    fn held(&self) -> Option<Term>;

    /// This client's identity, as it writes into the lease.
    fn holder(&self) -> &str;
}

// --- the object-store implementation --------------------------------------

/// Marks the object as ours, and versions the format.
const MAGIC: &str = "slate-lease v1";

/// How long a term lasts if the caller does not say.
///
/// Long enough that a renewal can fail twice before the term ends, short
/// enough that a crashed head node does not block its successor for a
/// noticeable time. The renewal cadence in [`crate::leadership`] is derived
/// from it rather than chosen separately, so the two cannot drift apart.
///
/// What it costs is now measured (`slate-headbench`, see
/// `docs/performance.md`), and it is not the renewal: a renewal is one
/// conditional PUT with no GET, against a five-second budget. What the term
/// buys is the failover window — takeover after a crash takes *exactly* the
/// term (measured at 300, 600 and 1200 ms terms, landing within 0.3%), so 15 s
/// means up to 15 s of refused writes plus up to 5 s before the successor
/// campaigns. A graceful release costs 3 µs instead. Still not changed:
/// trading that window down needs conditional-PUT tail latency against a real
/// bucket, and an in-memory object store cannot tell you that.
pub const DEFAULT_TERM: Duration = Duration::from_secs(15);

/// A lease held as a single object in the same storage the database uses.
///
/// Acquisition and renewal are conditional writes: `PutMode::Create` when the
/// object does not exist, `PutMode::Update` against the version last read
/// otherwise. Both are supported by every object store this project targets.
pub struct ObjectStoreLease {
    store: Arc<dyn ObjectStore>,
    path: Path,
    holder: String,
    term_length: Duration,
    clock: Arc<dyn Clock>,
    /// What this client holds, and the object version that proves it.
    ///
    /// A `tokio` mutex rather than a `std` one because it is held across the
    /// conditional write: two concurrent `acquire` calls on one client must not
    /// interleave their read and write halves, which is the same race the
    /// compare-and-set defends against between processes.
    held: Mutex<Option<Held>>,
}

/// A term, plus the object version that will let us write over it.
#[derive(Debug, Clone)]
struct Held {
    term: Term,
    version: UpdateVersion,
}

impl fmt::Debug for ObjectStoreLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObjectStoreLease")
            .field("path", &self.path)
            .field("holder", &self.holder)
            .field("term_length", &self.term_length)
            .finish_non_exhaustive()
    }
}

impl ObjectStoreLease {
    /// A lease at `path` in `store`, held under a fresh identity.
    ///
    /// The holder id is a v4 UUID with `label` prefixed, so logs and the
    /// `Leadership` RPC name something a person recognises while two processes
    /// on one host still get distinct identities. Two live processes sharing an
    /// identity would each treat the other's lease as its own and never
    /// contend, which is the one way to get the split brain this exists to
    /// prevent.
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>, path: impl Into<Path>, label: &str) -> Self {
        let sanitised: String = label
            .chars()
            .filter(|c| !c.is_whitespace() && *c != ':')
            .take(64)
            .collect();
        let holder = format!("{sanitised}-{}", uuid::Uuid::new_v4());
        Self::with_holder(store, path, holder)
    }

    /// A lease held under an identity the caller chooses.
    ///
    /// For tests, and for a deployment where the process identity comes from
    /// the scheduler. The identity must be unique among live processes; see
    /// [`ObjectStoreLease::new`].
    #[must_use]
    pub fn with_holder(store: Arc<dyn ObjectStore>, path: impl Into<Path>, holder: String) -> Self {
        // The stored format is line-oriented, so a holder containing a newline
        // could forge the fields after it. Replacing rather than refusing keeps
        // the constructor infallible; the identity is diagnostic, not
        // load-bearing.
        let holder = holder.replace(['\n', '\r'], "_");
        Self {
            store,
            path: path.into(),
            holder,
            term_length: DEFAULT_TERM,
            clock: Arc::new(SystemClock),
            held: Mutex::new(None),
        }
    }

    /// How long each term lasts.
    #[must_use]
    pub const fn with_term_length(mut self, term_length: Duration) -> Self {
        self.term_length = term_length;
        self
    }

    /// Take the time from somewhere other than the system clock.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// How long each term lasts.
    #[must_use]
    pub const fn term_length(&self) -> Duration {
        self.term_length
    }

    /// Read the lease object and the version needed to write over it.
    async fn read(&self) -> Result<Option<(Term, UpdateVersion)>, LeaseError> {
        match self.store.get(&self.path).await {
            Ok(result) => {
                let version = UpdateVersion {
                    e_tag: result.meta.e_tag.clone(),
                    version: result.meta.version.clone(),
                };
                let bytes = result.bytes().await.map_err(backend)?;
                Ok(Some((decode(&bytes)?, version)))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(error) => Err(backend(error)),
        }
    }

    /// Write `term` under `mode`, and remember it if it lands.
    async fn write(&self, term: Term, mode: PutMode) -> Result<Held, object_store::Error> {
        let options = PutOptions {
            mode,
            tags: TagSet::default(),
            attributes: Attributes::default(),
            extensions: object_store::Extensions::default(),
        };
        let result = self
            .store
            .put_opts(&self.path, encode(&term).into(), options)
            .await?;
        Ok(Held {
            term,
            version: UpdateVersion {
                e_tag: result.e_tag,
                version: result.version,
            },
        })
    }

    /// Who holds it now, for an error message. Best effort: naming the holder
    /// is a courtesy, and failing to read it must not turn a clean "someone
    /// else has it" into a storage error.
    async fn whoever_holds_it(&self) -> (String, SystemTime) {
        match self.read().await {
            Ok(Some((term, _))) => (term.holder, term.expires_at),
            _ => ("unknown".to_owned(), UNIX_EPOCH),
        }
    }
}

#[async_trait]
impl Lease for ObjectStoreLease {
    async fn acquire(&self) -> Result<Term, LeaseError> {
        let mut held = self.held.lock().await;
        let now = self.clock.now();
        let expires_at = now + self.term_length;

        let Some((current, version)) = self.read().await? else {
            // No lease object at all: the first process to create one wins,
            // and `Create` is what makes "first" mean something.
            let term = Term {
                generation: 1,
                holder: self.holder.clone(),
                expires_at,
            };
            return match self.write(term, PutMode::Create).await {
                Ok(new) => {
                    let term = new.term.clone();
                    *held = Some(new);
                    Ok(term)
                }
                Err(object_store::Error::AlreadyExists { .. }) => {
                    let (holder, expires_at) = self.whoever_holds_it().await;
                    Err(LeaseError::Held { holder, expires_at })
                }
                Err(error) => Err(backend(error)),
            };
        };

        // Someone else's term, still running. Nothing to do but wait it out —
        // there is no revocation, and taking it early would be exactly the
        // split brain the lease exists to avoid.
        if current.holder != self.holder && current.is_live_at(now) {
            *held = None;
            return Err(LeaseError::Held {
                holder: current.holder,
                expires_at: current.expires_at,
            });
        }

        let term = Term {
            // Every acquisition raises the generation, including one that
            // takes over an expired term of our own. The number orders terms;
            // it is not a count of distinct processes.
            generation: current.generation.saturating_add(1),
            holder: self.holder.clone(),
            expires_at,
        };
        match self.write(term, PutMode::Update(version)).await {
            Ok(new) => {
                let term = new.term.clone();
                *held = Some(new);
                Ok(term)
            }
            // The compare-and-set fired: between our read and our write,
            // someone else took it. This is the case a read-then-write lease
            // gets wrong, and it is the only reason this is not one.
            Err(object_store::Error::Precondition { .. }) => {
                *held = None;
                let (holder, expires_at) = self.whoever_holds_it().await;
                Err(LeaseError::Held { holder, expires_at })
            }
            Err(error) => Err(backend(error)),
        }
    }

    async fn renew(&self) -> Result<Term, LeaseError> {
        let mut held = self.held.lock().await;
        let Some(current) = held.clone() else {
            return Err(LeaseError::NotHeld);
        };

        let term = Term {
            generation: current.term.generation,
            holder: self.holder.clone(),
            expires_at: self.clock.now() + self.term_length,
        };
        match self
            .write(term, PutMode::Update(current.version.clone()))
            .await
        {
            Ok(new) => {
                let term = new.term.clone();
                *held = Some(new);
                Ok(term)
            }
            Err(object_store::Error::Precondition { .. }) => {
                // Pinning the write to the version we last wrote is what makes
                // this detectable at all: without it a renewal would happily
                // stamp our name back over the successor's lease and the two
                // processes would trade it forever.
                *held = None;
                let (holder, _) = self.whoever_holds_it().await;
                Err(LeaseError::Lost {
                    holder,
                    generation: current.term.generation,
                })
            }
            Err(error) => Err(backend(error)),
        }
    }

    async fn release(&self) -> Result<(), LeaseError> {
        let mut held = self.held.lock().await;
        let Some(current) = held.take() else {
            return Err(LeaseError::NotHeld);
        };

        // Expire it in place rather than delete it. `object_store` has no
        // conditional delete, so an unconditional one would remove whatever is
        // there — including a successor's lease, if we were slow enough to be
        // replaced before getting here. Writing an already-expired term is
        // conditional, so it cannot touch anyone else's.
        let term = Term {
            generation: current.term.generation,
            holder: self.holder.clone(),
            expires_at: self.clock.now(),
        };
        match self.write(term, PutMode::Update(current.version)).await {
            Ok(_) => Ok(()),
            // Already replaced. The successor has it; there is nothing to give
            // up and nothing to report.
            Err(object_store::Error::Precondition { .. }) => Ok(()),
            Err(error) => Err(backend(error)),
        }
    }

    async fn observe(&self) -> Result<Option<Term>, LeaseError> {
        Ok(self.read().await?.map(|(term, _)| term))
    }

    fn held(&self) -> Option<Term> {
        // `try_lock` rather than blocking: this is called from a status
        // handler, and reporting "busy acquiring" as "not held" is better than
        // making a diagnostic RPC wait on a round trip to object storage.
        self.held
            .try_lock()
            .ok()
            .and_then(|held| held.as_ref().map(|h| h.term.clone()))
    }

    fn holder(&self) -> &str {
        &self.holder
    }
}

fn backend<E: std::error::Error + Send + Sync + 'static>(error: E) -> LeaseError {
    LeaseError::Backend(Box::new(error))
}

/// The stored form: a magic line, then one field per line.
fn encode(term: &Term) -> Bytes {
    let expires_ms = term
        .expires_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Bytes::from(format!(
        "{MAGIC}\ngeneration: {}\nholder: {}\nexpires_ms: {expires_ms}\n",
        term.generation, term.holder
    ))
}

fn decode(bytes: &[u8]) -> Result<Term, LeaseError> {
    let text = core::str::from_utf8(bytes).map_err(|_| LeaseError::Malformed {
        detail: "not UTF-8".to_owned(),
    })?;
    let mut lines = text.lines();
    match lines.next() {
        Some(MAGIC) => {}
        other => {
            return Err(LeaseError::Malformed {
                detail: format!("expected `{MAGIC}` on the first line, found {other:?}"),
            });
        }
    }

    let mut generation = None;
    let mut holder = None;
    let mut expires_ms = None;
    for line in lines {
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        match field {
            "generation" => generation = value.trim().parse::<u64>().ok(),
            "holder" => holder = Some(value.trim().to_owned()),
            "expires_ms" => expires_ms = value.trim().parse::<u64>().ok(),
            _ => {}
        }
    }

    match (generation, holder, expires_ms) {
        (Some(generation), Some(holder), Some(expires_ms)) => Ok(Term {
            generation,
            holder,
            expires_at: UNIX_EPOCH + Duration::from_millis(expires_ms),
        }),
        _ => Err(LeaseError::Malformed {
            detail: format!("missing or unparsable fields in {text:?}"),
        }),
    }
}
