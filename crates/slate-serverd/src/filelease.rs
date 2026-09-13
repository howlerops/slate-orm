//! A writer lease for a database on a local filesystem.
//!
//! # Why the object-store lease is not usable here
//!
//! [`ObjectStoreLease`](slate_server::lease::ObjectStoreLease) takes and
//! extends the lease with conditional writes: `PutMode::Create` to take one
//! that does not exist, `PutMode::Update` against the version last read to
//! renew, release, or take over an expired one.
//! `object_store::local::LocalFileSystem` implements the first and returns
//! `NotImplemented` for the second.
//!
//! The consequences were not subtle, and were found by running the thing:
//!
//! - the first node takes the lease and can never renew it, so the term
//!   silently lapses under a perfectly healthy leader;
//! - it cannot release it either, so a restart waits out the term;
//! - and then the restart *fails anyway*, because taking over an expired lease
//!   is also a conditional update. A `local` database was a one-start database.
//!
//! None of that is silent any more:
//! [`ObjectStoreLease::acquire`](slate_server::lease::ObjectStoreLease) now
//! probes for the conditional update and refuses with `LeaseError::Unsupported`
//! rather than taking a lease it cannot keep. So the object-store lease over a
//! local filesystem is a refusal to start, not a one-start database — and this
//! type is what a `local` deployment uses instead, rather than a patch over a
//! failure nobody could see.
//!
//! # What replaces it
//!
//! An advisory lock (`flock`) on one file, through `std::fs::File::try_lock`.
//! For the case `backend = "local"` describes — one machine, one directory —
//! this is a better primitive than the object lease, not a weaker stand-in:
//!
//! - mutual exclusion is enforced by the kernel rather than inferred from a
//!   timestamp, so there is no window in which two processes both believe they
//!   hold it;
//! - it is released when the process exits, however it exits, so a crashed
//!   node does not block its successor for a term;
//! - and it needs no clock, which is the part of a time-based lease that
//!   cannot be made sound.
//!
//! # The alternatives, and why not
//!
//! **A fake lease that always grants** — which is what
//! `clients/python/testserver` and the head node's own tests use. It is what
//! this crate is trying not to be: a binary whose leadership is real on one
//! backend and pretend on another is one where the fast tests prove nothing
//! about the slow path.
//!
//! **A create-only lease in `slate-server`, alongside the object-store one.**
//! This is the interesting one, because it would work. `PutMode::Create` *is*
//! implemented by `LocalFileSystem`, and a compare-and-set can be built out of
//! nothing else by putting the generation in the object's *name*: taking the
//! lease is creating `writer/0000000008`, which exactly one contender can do,
//! and reading it is listing the prefix and taking the highest. That is not a
//! trick — it is precisely how SlateDB writes its own manifest, which is why
//! SlateDB can fence a writer over a local directory at all.
//!
//! It was still rejected, for three reasons in descending order of weight:
//!
//! - **It is worse where it is not needed.** Against S3 it costs a `PUT` plus
//!   a `DELETE` per renewal and a `LIST` per observation, where the conditional
//!   update S3 *does* support costs one `PUT` and no `GET`. So it could not
//!   replace `ObjectStoreLease`; it would have to sit beside it, chosen by
//!   capability — a third lease implementation, and a rule for picking between
//!   two of them, to serve one backend.
//! - **It is weaker than this one on the deployment `local` describes.** One
//!   machine, one directory. A `flock` is mutual exclusion the kernel enforces,
//!   released when the process exits however it exits; a generation lease is
//!   two clocks and a term, so a crashed node blocks its successor for up to
//!   fifteen seconds and a paused one can believe in a term it has lost. Trading
//!   a real lock for a timed one is a downgrade.
//! - **The one place it would be better is untestable here.** Its appeal is
//!   NFS: `LocalFileSystem`'s `PutMode::Create` publishes through a hard link,
//!   which is the classic atomic primitive that *does* cross an NFS mount,
//!   where `flock` does not. That is a real argument and it is also a safety
//!   claim about a filesystem this repository has no way to run a test against.
//!   Shipping an untested claim of NFS safety is worse than the documented
//!   limitation below, because the limitation makes people use `s3` and the
//!   claim would make them stop.
//!
//! If a shared-mount deployment ever becomes a thing this project supports, the
//! generation lease is the design to build, and the paragraph above is the
//! specification. It is not that today.
//!
//! # What it does not do
//!
//! `flock` is advisory and host-local. Two machines mounting one directory
//! over NFS are not separated by it, and neither is a process that never asks.
//! That is the same scope `backend = "local"` already has — a local directory
//! is not a shared database — but it is worth saying, because the failure
//! would be exactly the split brain the lease exists to prevent. A deployment
//! with more than one node uses `s3`, where the conditional write is real and
//! [`ObjectStoreLease`](slate_server::lease::ObjectStoreLease) is used
//! unchanged.
//!
//! Two nodes on *one* host over one `local` directory are separated, and that
//! is now a supported shape rather than a refusal: the second one starts as a
//! reader. See `Head::read_only` and `storage::open_read_only`.
//!
//! The term is still reported and still extended on renewal, because
//! [`Term`] has one and the leadership RPC shows it. It is bookkeeping here:
//! the lock does not expire, so a term that lapsed would not release anything.

use async_trait::async_trait;
use core::time::Duration;
use slate_server::lease::{Lease, LeaseError, Term};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use tokio::sync::Mutex;

/// Marks the file as ours and versions the format.
const MAGIC: &str = "slate-file-lease v1";

/// A lease held as an advisory lock on one file.
pub(crate) struct FileLease {
    path: PathBuf,
    holder: String,
    term_length: Duration,
    /// The locked handle, and what it is a term for. Dropping the handle
    /// releases the lock, which is why it is kept rather than the lock being
    /// "taken" and forgotten.
    ///
    /// A `tokio` mutex because it is held across the lock attempt and the
    /// write that records the holder, exactly as `ObjectStoreLease` holds one
    /// across its read and conditional write.
    held: Mutex<Option<Holding>>,
    /// The generation last written, so a renewal does not re-read the file.
    generation: AtomicU64,
}

struct Holding {
    /// Kept alive: dropping it unlocks.
    file: File,
    term: Term,
}

impl core::fmt::Debug for FileLease {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FileLease")
            .field("path", &self.path)
            .field("holder", &self.holder)
            .field("term_length", &self.term_length)
            .finish_non_exhaustive()
    }
}

impl FileLease {
    /// A lease over the file at `path`, held under `holder`.
    pub(crate) fn new(path: impl Into<PathBuf>, holder: String, term_length: Duration) -> Self {
        Self {
            path: path.into(),
            // The file is line-oriented, so a holder with a newline in it could
            // forge the line after it. Replaced rather than refused: the
            // identity is diagnostic, and this matches what `ObjectStoreLease`
            // does with the same problem.
            holder: holder.replace(['\n', '\r'], "_"),
            term_length,
            held: Mutex::new(None),
            generation: AtomicU64::new(0),
        }
    }

    /// Open the lock file, creating it and its directory if needed.
    fn open(path: &Path) -> Result<File, LeaseError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(backend)?;
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(backend)
    }

    /// Read `generation` and `holder` out of an already-open file.
    fn read(file: &mut File) -> (u64, String) {
        let mut text = String::new();
        if file.seek(SeekFrom::Start(0)).is_err() || file.read_to_string(&mut text).is_err() {
            return (0, "unknown".to_owned());
        }
        let mut generation = 0;
        let mut holder = "unknown".to_owned();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("generation: ") {
                generation = rest.trim().parse().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("holder: ") {
                holder = rest.trim().to_owned();
            }
        }
        (generation, holder)
    }

    fn write(file: &mut File, generation: u64, holder: &str) -> Result<(), LeaseError> {
        file.seek(SeekFrom::Start(0)).map_err(backend)?;
        file.set_len(0).map_err(backend)?;
        // Plain text so an operator can `cat` it, the same reason
        // `ObjectStoreLease` gives for its format.
        write!(
            file,
            "{MAGIC}\ngeneration: {generation}\nholder: {holder}\n"
        )
        .map_err(backend)?;
        file.flush().map_err(backend)
    }
}

#[async_trait]
impl Lease for FileLease {
    async fn acquire(&self) -> Result<Term, LeaseError> {
        let mut held = self.held.lock().await;
        if let Some(holding) = held.as_ref() {
            // Already ours. Re-acquiring is not an error and does not raise the
            // generation: nothing changed hands.
            return Ok(holding.term.clone());
        }

        let mut file = Self::open(&self.path)?;
        if file.try_lock().is_err() {
            // Someone holds it. Their name is in the file, which we can read
            // without the lock — the lock is advisory and reading is harmless.
            let (_, holder) = Self::read(&mut file);
            return Err(LeaseError::Held {
                holder,
                // There is no expiry: the lock is held until the holder exits.
                // Reporting one term's worth into the future is the honest
                // approximation, since `Term` has nowhere to say "indefinite"
                // and a successor should come back and try again rather than
                // treat it as free.
                expires_at: SystemTime::now() + self.term_length,
            });
        }

        // The generation continues the file's rather than starting again, so
        // two terms of two different processes are still ordered.
        let (previous, _) = Self::read(&mut file);
        let generation = previous.saturating_add(1);
        Self::write(&mut file, generation, &self.holder)?;
        self.generation.store(generation, Ordering::SeqCst);

        let term = Term {
            generation,
            holder: self.holder.clone(),
            expires_at: SystemTime::now() + self.term_length,
        };
        *held = Some(Holding {
            file,
            term: term.clone(),
        });
        Ok(term)
    }

    async fn renew(&self) -> Result<Term, LeaseError> {
        let mut held = self.held.lock().await;
        let Some(holding) = held.as_mut() else {
            return Err(LeaseError::NotHeld);
        };
        // Nothing to write: the lock is the lease, and it has not moved. Only
        // the reported expiry advances.
        holding.term.expires_at = SystemTime::now() + self.term_length;
        Ok(holding.term.clone())
    }

    async fn release(&self) -> Result<(), LeaseError> {
        let mut held = self.held.lock().await;
        let Some(mut holding) = held.take() else {
            return Err(LeaseError::NotHeld);
        };
        // Blanked before unlocking so a successor never reads a stale holder,
        // and the unlock is the `drop` — kept explicit so the order is legible.
        let _ = Self::write(&mut holding.file, holding.term.generation, "(released)");
        let _ = holding.file.unlock();
        drop(holding.file);
        Ok(())
    }

    async fn observe(&self) -> Result<Option<Term>, LeaseError> {
        if let Some(holding) = self.held.lock().await.as_ref() {
            return Ok(Some(holding.term.clone()));
        }
        let mut file = match Self::open(&self.path) {
            Ok(file) => file,
            // Nothing there is not an error; it is the answer.
            Err(_) => return Ok(None),
        };
        let (generation, holder) = Self::read(&mut file);
        if generation == 0 || holder == "(released)" {
            return Ok(None);
        }
        Ok(Some(Term {
            generation,
            holder,
            expires_at: SystemTime::now() + self.term_length,
        }))
    }

    fn held(&self) -> Option<Term> {
        // `try_lock` rather than blocking, for the reason `ObjectStoreLease`
        // gives: this is called from a status handler, and reporting "busy"
        // as "not held" beats making a diagnostic RPC wait.
        self.held
            .try_lock()
            .ok()
            .and_then(|held| held.as_ref().map(|h| h.term.clone()))
    }

    fn holder(&self) -> &str {
        &self.holder
    }
}

fn backend(error: std::io::Error) -> LeaseError {
    LeaseError::Backend(Box::new(error))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn lease(path: &Path, holder: &str) -> FileLease {
        FileLease::new(path, holder.to_owned(), Duration::from_secs(15))
    }

    #[tokio::test]
    async fn one_holder_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer");
        let first = lease(&path, "first");
        let second = lease(&path, "second");

        let term = first.acquire().await.expect("the first one takes it");
        assert_eq!(term.generation, 1);

        match second.acquire().await {
            Err(LeaseError::Held { holder, .. }) => assert_eq!(holder, "first"),
            other => panic!("the second should be refused, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn releasing_lets_the_next_one_in_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer");
        let first = lease(&path, "first");
        let second = lease(&path, "second");

        first.acquire().await.expect("first");
        first.release().await.expect("release");

        // No waiting for a term: this is the property the object-store lease
        // cannot give on a local filesystem, and the reason this type exists.
        let term = second.acquire().await.expect("second");
        assert_eq!(term.holder, "second");
        assert_eq!(term.generation, 2, "generations continue across processes");
    }

    #[tokio::test]
    async fn a_dropped_holder_releases_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer");
        {
            let first = lease(&path, "first");
            first.acquire().await.expect("first");
        }
        // The `File` went with it, and with the file the lock.
        let second = lease(&path, "second");
        assert!(second.acquire().await.is_ok());
    }

    #[tokio::test]
    async fn renewing_extends_the_term_without_changing_hands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer");
        let held = lease(&path, "held");
        let first = held.acquire().await.expect("acquire");
        let renewed = held.renew().await.expect("renew");
        assert_eq!(renewed.generation, first.generation);
        assert!(renewed.expires_at >= first.expires_at);
    }

    #[tokio::test]
    async fn renewing_something_not_held_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let idle = lease(&dir.path().join("writer"), "idle");
        assert!(matches!(idle.renew().await, Err(LeaseError::NotHeld)));
    }

    #[tokio::test]
    async fn observe_reports_the_holder_and_then_nobody() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer");
        let watcher = lease(&path, "watcher");
        assert!(watcher.observe().await.expect("observe").is_none());

        let holder = lease(&path, "holder");
        holder.acquire().await.expect("acquire");
        let seen = watcher.observe().await.expect("observe").expect("a term");
        assert_eq!(seen.holder, "holder");

        holder.release().await.expect("release");
        assert!(watcher.observe().await.expect("observe").is_none());
    }

    #[tokio::test]
    async fn the_file_is_readable_by_a_person() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer");
        let held = lease(&path, "node-a");
        held.acquire().await.expect("acquire");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains(MAGIC), "{text}");
        assert!(text.contains("holder: node-a"), "{text}");
    }

    #[tokio::test]
    async fn a_newline_in_a_holder_cannot_forge_a_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer");
        let sneaky = lease(&path, "a\ngeneration: 99");
        let term = sneaky.acquire().await.expect("acquire");
        assert_eq!(term.generation, 1);
        // The newline became an underscore, so the injected text is inside the
        // holder's own value rather than starting a line of its own — and it
        // is the line start that the reader keys on.
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(!text.lines().any(|line| line == "generation: 99"), "{text}");
        sneaky.release().await.expect("release");

        // Read back through the parser, which is what actually matters: the
        // next holder's generation continues from 1, not from 99.
        let next = lease(&path, "next");
        assert_eq!(next.acquire().await.expect("acquire").generation, 2);
    }
}
