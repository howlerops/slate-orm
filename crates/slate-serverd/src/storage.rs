//! Opening the store, the replicas and the object store the lease lives in.
//!
//! # Why the lease and the database share one object store
//!
//! `slate-server`'s lease is "one object, same bucket", and the argument for
//! it is that a deployment should need no second stateful system to elect a
//! writer. That only holds if the file cannot accidentally point them at two
//! different places, so it cannot: the lease's `path` is a path *within* the
//! store `[storage]` already describes, and there is no way to configure a
//! different endpoint or bucket for it.
//!
//! # Why this is two steps
//!
//! Opening a SlateDB database as a writer *fences* whatever writer was there
//! before. That is the safety mechanism and it is supposed to be rare: the
//! lease exists, in `topology.md`'s words, "to stop the fencing happening over
//! and over".
//!
//! A process that opened the database and then campaigned would fence the
//! healthy leader on its way to discovering that it is not the leader. So
//! [`prepare`] builds the object store and settles the settings, the lease is
//! taken against that object store, and only then does [`open`] open the
//! database. A node that loses the campaign never touches it.
//!
//! # The in-memory backend has a real lease
//!
//! `backend = "memory"` needs a [`Leadership`](slate_server::Leadership), and
//! the obvious way to get one is a fake [`Lease`](slate_server::Lease) that
//! always grants — which is what `clients/python/testserver` writes, and what
//! the head node's own tests write again.
//!
//! This crate instead runs the real [`ObjectStoreLease`] against an in-memory
//! object store. The compare-and-set, the generation counter, the renewal and
//! the expiry are all the genuine article; the store simply is not visible to
//! any other process, which is exactly as true of the data. A single-process
//! database has a single-process lease, and no code here has to be trusted to
//! behave like the real one.
//!
//! It costs a lease acquisition against a `BTreeMap` at startup. What it buys
//! is that the memory backend exercises the same leadership path as the
//! others, so a change that breaks leadership cannot pass the fast tests and
//! fail only against object storage.
//!
//! [`ObjectStoreLease`]: slate_server::lease::ObjectStoreLease

use crate::config;
use crate::error::{Fault, Started};
use core::time::Duration;
use object_store::ObjectStore;
use object_store::memory::InMemory;
use object_store::path::Path;
use slate_kernel::KvReadStore;
use slate_kernel::memory::MemoryStore;
use slate_server::lease::ObjectStoreLease;
use slate_slatedb::{Durability, ReplicaMode, SlateReader, SlateStore};
use slatedb::IsolationLevel;
use std::sync::Arc;

/// The writer, whichever kind it is.
///
/// An enum rather than `Arc<dyn KvStore>` because
/// [`Head`](slate_server::Head) is generic over its writer and needs the
/// concrete type: the writer also joins the replica pool, and a trait object
/// there would lose `SlateStore::close`, which is what makes a clean shutdown
/// clean.
#[derive(Debug)]
pub(crate) enum Writer {
    /// Everything in this process, and gone when it exits.
    Memory(Arc<MemoryStore>),
    /// A SlateDB database over an object store.
    Slate(Arc<SlateStore>),
}

/// Which backend, once the strings have been checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A map in this process.
    Memory,
    /// A SlateDB database over a local directory.
    ///
    /// Separate from [`Kind::Object`] for one reason, and it is a leadership
    /// reason rather than a storage one: `object_store`'s `LocalFileSystem`
    /// returns `NotImplemented` for `PutMode::Update`, and that is the
    /// conditional write the lease's renew and release are made of. A local
    /// node can therefore *take* the lease (which uses `PutMode::Create`) and
    /// can never renew or release it.
    Local,
    /// A SlateDB database over an object store with conditional writes.
    Object,
}

/// Everything the object store is needed for, resolved before the lease is
/// campaigned for.
pub(crate) struct Prepared {
    /// The object store the database uses.
    pub(crate) objects: Arc<dyn ObjectStore>,
    kind: Kind,
    /// The filesystem directory, for `local` only. The file lease needs a real
    /// path, and an `ObjectStore` deliberately does not expose one.
    directory: Option<std::path::PathBuf>,
    path: Path,
    durability: Durability,
    isolation: IsolationLevel,
    /// One line for the banner.
    pub(crate) description: String,
}

impl core::fmt::Debug for Prepared {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Prepared")
            .field("kind", &self.kind)
            .field("path", &self.path)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

impl Prepared {
    /// Whether another process could be writing this database.
    ///
    /// The in-memory backend is invisible outside this process, so there is
    /// nothing to contend with and nothing to fence. Every other backend can
    /// be opened by a second `slate-serverd`, and the whole point of the lease
    /// is that only one of them should.
    pub(crate) const fn is_shared(&self) -> bool {
        matches!(self.kind, Kind::Local | Kind::Object)
    }

    /// The lease this backend's storage can actually support.
    ///
    /// Two implementations, chosen by what the object store can do rather than
    /// by taste. `Kind::Local` gets an advisory file lock, because
    /// `LocalFileSystem` cannot do the conditional write the object-store
    /// lease is built out of; everything else gets the real
    /// [`ObjectStoreLease`]. See [`crate::filelease`] for the argument.
    pub(crate) fn lease(
        &self,
        settings: &config::LeaseSettings,
        term: Duration,
    ) -> Arc<dyn slate_server::Lease> {
        let holder = settings.holder.clone();
        match (self.kind, &self.directory) {
            (Kind::Local, Some(directory)) => {
                // The lease path is a path inside the store, and for a local
                // store that is a path inside the directory — so the lock file
                // sits beside the data, which is what makes "one node per
                // database" a thing the filesystem can enforce.
                let path = directory.join(settings.path.trim_start_matches('/'));
                Arc::new(crate::filelease::FileLease::new(
                    path,
                    holder.unwrap_or_else(|| format!("slate-serverd-{}", uuid::Uuid::new_v4())),
                    term,
                ))
            }
            _ => Arc::new(
                match holder {
                    Some(holder) => ObjectStoreLease::with_holder(
                        Arc::clone(&self.objects),
                        settings.path.as_str(),
                        holder,
                    ),
                    // No holder configured: the lease appends a fresh uuid to
                    // the label, which is what keeps two processes on one host
                    // from believing they hold each other's lease.
                    None => ObjectStoreLease::new(
                        Arc::clone(&self.objects),
                        settings.path.as_str(),
                        "slate-serverd",
                    ),
                }
                .with_term_length(term),
            ),
        }
    }
}

/// Resolve the backend and build the object store, without opening the
/// database.
pub(crate) fn prepare(storage: &config::Storage) -> Started<Prepared> {
    let durability = match storage.durability.as_str() {
        "durable" => Durability::Durable,
        // Spelled out in the message because this one loses acknowledged
        // writes, and an operator should meet that sentence at least once.
        "visible" => Durability::Visible,
        other => {
            return Err(Fault::new(format!(
                "`[storage] durability = \"{other}\"` is not a setting; there are `durable` (wait for object storage) and `visible` (return as soon as the write is visible to readers, which loses acknowledged writes if the writer dies before its next flush)"
            )));
        }
    };
    let isolation = match storage.isolation.as_str() {
        "serializable" => IsolationLevel::SerializableSnapshot,
        "snapshot" => IsolationLevel::Snapshot,
        other => {
            return Err(Fault::new(format!(
                "`[storage] isolation = \"{other}\"` is not a level; there are `serializable` and `snapshot`. The record layer's own guarantees hold under either, because they rest on write-write conflicts; an application invariant that reads one row and writes another needs `serializable`"
            )));
        }
    };

    // `Path` refuses a leading slash, and `/records` is how the rest of this
    // project writes a database path. Trimmed rather than refused, so the
    // documentation and the file can agree.
    let path = Path::from(storage.path.trim_start_matches('/'));

    let mut directory_path = None;
    let (kind, objects, description) = match storage.backend.as_str() {
        "memory" => {
            reject(storage, &["directory", "s3"])?;
            let objects: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
            (
                Kind::Memory,
                objects,
                "memory (nothing is persisted)".to_owned(),
            )
        }

        "local" => {
            reject(storage, &["s3"])?;
            let directory = storage.directory.as_ref().ok_or_else(|| {
                Fault::new("`backend = \"local\"` needs `directory`, the folder the object store is rooted at")
            })?;
            // Created rather than required to exist: a first start on a fresh
            // volume is the ordinary case, and `LocalFileSystem` refuses a
            // missing prefix with an error about a path rather than about a
            // missing directory.
            std::fs::create_dir_all(directory)
                .map_err(|why| Fault::new(format!("cannot create `{directory}`: {why}")))?;
            let objects: Arc<dyn ObjectStore> = Arc::new(
                object_store::local::LocalFileSystem::new_with_prefix(directory)
                    .map_err(|why| Fault::new(format!("cannot use `{directory}`: {why}")))?,
            );
            directory_path = Some(std::path::PathBuf::from(directory));
            (
                Kind::Local,
                objects,
                format!("local ({directory}, database at {path})"),
            )
        }

        "s3" => {
            reject(storage, &["directory"])?;
            let s3 = storage
                .s3
                .as_ref()
                .ok_or_else(|| Fault::new("`backend = \"s3\"` needs a `[storage.s3]` table"))?;
            let s3_config = s3_config(s3)?;
            let bucket = s3_config.bucket().to_owned();
            let objects = s3_config
                .build()
                .map_err(|why| Fault::new(format!("cannot reach the bucket `{bucket}`: {why}")))?;
            (
                Kind::Object,
                objects,
                format!("s3 (bucket {bucket}, database at {path})"),
            )
        }

        other => {
            return Err(Fault::new(format!(
                "`[storage] backend = \"{other}\"` is not a backend; there are `memory`, `local` and `s3`"
            )));
        }
    };

    Ok(Prepared {
        objects,
        kind,
        directory: directory_path,
        path,
        durability,
        isolation,
        description,
    })
}

/// Open the writer and its replicas. Only a node holding the lease may.
pub(crate) async fn open(
    prepared: &Prepared,
    replicas: &[config::Replica],
) -> Started<(Writer, Vec<Arc<dyn KvReadStore>>)> {
    match prepared.kind {
        Kind::Memory => {
            if !replicas.is_empty() {
                return Err(Fault::new(
                    "`backend = \"memory\"` has no replicas: a replica reads the database out of object storage, and this one is a map in this process. Use `local` or `s3` to run with replicas",
                ));
            }
            Ok((Writer::Memory(Arc::new(MemoryStore::new())), Vec::new()))
        }
        Kind::Local | Kind::Object => {
            let store = SlateStore::open(prepared.path.clone(), Arc::clone(&prepared.objects))
                .await
                .map_err(|why| {
                    Fault::new(format!(
                        "cannot open the database at `{}`: {why}",
                        prepared.path
                    ))
                })?;
            let writer = Arc::new(
                store
                    .with_durability(prepared.durability)
                    .with_isolation(prepared.isolation),
            );
            let opened = open_replicas(replicas, &prepared.path, &prepared.objects).await?;
            Ok((Writer::Slate(writer), opened))
        }
    }
}

async fn open_replicas(
    replicas: &[config::Replica],
    path: &Path,
    objects: &Arc<dyn ObjectStore>,
) -> Started<Vec<Arc<dyn KvReadStore>>> {
    let mut opened: Vec<Arc<dyn KvReadStore>> = Vec::with_capacity(replicas.len());
    let mut names: Vec<&String> = Vec::with_capacity(replicas.len());
    for replica in replicas {
        // The name is what `served_by` reports and what tenant affinity
        // hashes, so two replicas sharing one would make routing report a
        // replica that served nothing — the confusion `topology.md` says
        // `snapshot_from` exists to avoid.
        if names.contains(&&replica.name) {
            return Err(Fault::new(format!(
                "two replicas are called `{}`; the name is what `served_by` reports and what tenant affinity hashes on",
                replica.name
            )));
        }
        names.push(&replica.name);

        let mode = replica_mode(replica)?;
        let reader = SlateReader::open(&replica.name, path.clone(), Arc::clone(objects), mode)
            .await
            .map_err(|why| Fault::new(format!("cannot open replica `{}`: {why}", replica.name)))?;
        opened.push(Arc::new(reader));
    }
    Ok(opened)
}

fn replica_mode(replica: &config::Replica) -> Started<ReplicaMode> {
    match (replica.mode.as_str(), &replica.checkpoint) {
        ("following", None) => Ok(ReplicaMode::Following),
        ("latest", None) => Ok(ReplicaMode::Latest),
        ("pinned", Some(id)) => uuid::Uuid::parse_str(id)
            .map(ReplicaMode::Pinned)
            .map_err(|_| {
                Fault::new(format!(
                    "replica `{}`: `checkpoint = \"{id}\"` is not a uuid",
                    replica.name
                ))
            }),
        ("pinned", None) => Err(Fault::new(format!(
            "replica `{}` is `pinned` and names no `checkpoint`; a pinned replica is fixed on one checkpoint id",
            replica.name
        ))),
        (mode @ ("following" | "latest"), Some(_)) => Err(Fault::new(format!(
            "replica `{}` is `{mode}` and names a `checkpoint`, which only `pinned` uses",
            replica.name
        ))),
        (other, _) => Err(Fault::new(format!(
            "replica `{}`: `mode = \"{other}\"` is not a mode; there are `following`, `latest` and `pinned`",
            replica.name
        ))),
    }
}

/// Build an [`S3Config`](slate_slatedb::S3Config) from the file or the
/// environment.
fn s3_config(s3: &config::S3) -> Started<slate_slatedb::S3Config> {
    if s3.from_env {
        // Everything else in the table would be silently ignored, and a
        // half-overridden S3 configuration is how a deployment ends up writing
        // to the wrong bucket.
        if s3.bucket.is_some()
            || s3.endpoint.is_some()
            || s3.region.is_some()
            || s3.access_key_id_env.is_some()
            || s3.secret_access_key_env.is_some()
            || s3.session_token_env.is_some()
            || s3.allow_http
            || s3.virtual_hosted_style
        {
            return Err(Fault::new(
                "`[storage.s3] from_env = true` takes the whole configuration from the `SLATE_S3_*` variables; remove the other keys rather than have them quietly ignored",
            ));
        }
        return slate_slatedb::S3Config::from_env().ok_or_else(|| {
            Fault::new("`[storage.s3] from_env = true` but `SLATE_S3_BUCKET` is not set")
        });
    }

    let bucket = s3
        .bucket
        .as_ref()
        .ok_or_else(|| Fault::new("`[storage.s3]` needs a `bucket`, or `from_env = true`"))?;
    let mut config = slate_slatedb::S3Config::new(bucket);
    if let Some(endpoint) = &s3.endpoint {
        config = config.with_endpoint(endpoint);
    }
    if let Some(region) = &s3.region {
        config = config.with_region(region);
    }
    match (&s3.access_key_id_env, &s3.secret_access_key_env) {
        (Some(key), Some(secret)) => {
            config = config.with_credentials(
                environment(key, "access_key_id_env")?,
                environment(secret, "secret_access_key_env")?,
            );
        }
        // Left to the environment: `AmazonS3Builder::from_env` picks up the
        // AWS variables and the instance metadata service, which is how a
        // deployment on AWS is meant to get credentials.
        (None, None) => {}
        _ => {
            return Err(Fault::new(
                "`[storage.s3]` names one of `access_key_id_env` and `secret_access_key_env`; it takes both or neither",
            ));
        }
    }
    if let Some(token) = &s3.session_token_env {
        config = config.with_session_token(environment(token, "session_token_env")?);
    }
    Ok(config
        .allow_http(s3.allow_http)
        .virtual_hosted_style(s3.virtual_hosted_style))
}

fn environment(variable: &str, field: &str) -> Started<String> {
    std::env::var(variable).map_err(|_| {
        Fault::new(format!(
            "`[storage.s3] {field} = \"{variable}\"`, and that environment variable is not set"
        ))
    })
}

/// Refuse a field that belongs to another backend, for the reason
/// [`crate::auth::choose`] refuses one that belongs to another auth mode: it
/// would otherwise be ignored, and the backend is probably not the one meant.
fn reject(storage: &config::Storage, fields: &[&str]) -> Started<()> {
    let present: Vec<&str> = fields
        .iter()
        .copied()
        .filter(|field| match *field {
            "directory" => storage.directory.is_some(),
            "s3" => storage.s3.is_some(),
            _ => false,
        })
        .collect();
    if present.is_empty() {
        return Ok(());
    }
    Err(Fault::new(format!(
        "`[storage] backend = \"{}\"` does not use {}",
        storage.backend,
        present
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(" or ")
    )))
}

/// How long a lease term lasts, and how often it is renewed.
pub(crate) fn term(lease: &config::LeaseSettings) -> Started<Duration> {
    Ok(
        config::optional_duration(lease.term.as_ref(), "lease.term")?
            .unwrap_or(slate_server::lease::DEFAULT_TERM),
    )
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

    fn storage(text: &str) -> config::Storage {
        toml::from_str(text).unwrap_or_else(|e| panic!("{e}"))
    }

    #[tokio::test]
    async fn the_memory_backend_opens_and_refuses_replicas() {
        let prepared = prepare(&storage("backend = \"memory\"")).unwrap();
        assert!(!prepared.is_shared());
        let (writer, replicas) = open(&prepared, &[]).await.unwrap();
        assert!(matches!(writer, Writer::Memory(_)));
        assert!(replicas.is_empty());

        let replica = toml::from_str::<config::Replica>("name = \"a\"").unwrap();
        let error = match open(&prepared, &[replica]).await {
            Err(fault) => fault.to_string(),
            Ok(_) => panic!("the memory backend accepted a replica"),
        };
        assert!(error.contains("has no replicas"), "{error}");
    }

    #[test]
    fn a_field_from_another_backend_is_refused() {
        let error = prepare(&storage("backend = \"memory\"\ndirectory = \"/tmp/x\""))
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not use `directory`"), "{error}");
    }

    #[test]
    fn an_unknown_backend_lists_the_backends() {
        let error = prepare(&storage("backend = \"postgres\""))
            .unwrap_err()
            .to_string();
        assert!(error.contains("`memory`, `local` and `s3`"), "{error}");
    }

    #[test]
    fn an_unknown_durability_explains_what_visible_costs() {
        let error = prepare(&storage("backend = \"memory\"\ndurability = \"fast\""))
            .unwrap_err()
            .to_string();
        assert!(error.contains("loses acknowledged writes"), "{error}");
    }

    #[test]
    fn an_unknown_isolation_level_lists_both() {
        let error = prepare(&storage(
            "backend = \"memory\"\nisolation = \"read-committed\"",
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("`serializable` and `snapshot`"), "{error}");
    }

    #[tokio::test]
    async fn a_local_backend_opens_a_real_database_and_is_shared() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let prepared = prepare(&storage(&format!(
            "backend = \"local\"\ndirectory = \"{}\"",
            dir.path().display()
        )))
        .unwrap();
        assert!(prepared.is_shared(), "another process could open this one");
        let (writer, _) = open(&prepared, &[]).await.unwrap();
        assert!(matches!(writer, Writer::Slate(_)));
    }

    #[test]
    fn a_pinned_replica_without_a_checkpoint_is_refused() {
        let replica = toml::from_str::<config::Replica>("name = \"a\"\nmode = \"pinned\"").unwrap();
        let error = replica_mode(&replica).unwrap_err().to_string();
        assert!(error.contains("names no `checkpoint`"), "{error}");
    }

    #[test]
    fn an_unknown_replica_mode_lists_the_modes() {
        let replica = toml::from_str::<config::Replica>("name = \"a\"\nmode = \"warm\"").unwrap();
        let error = replica_mode(&replica).unwrap_err().to_string();
        assert!(
            error.contains("`following`, `latest` and `pinned`"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn two_replicas_with_one_name_are_refused() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let prepared = prepare(&storage(&format!(
            "backend = \"local\"\ndirectory = \"{}\"",
            dir.path().display()
        )))
        .unwrap();
        let one = toml::from_str::<config::Replica>("name = \"a\"").unwrap();
        let two = toml::from_str::<config::Replica>("name = \"a\"").unwrap();
        let error = match open(&prepared, &[one, two]).await {
            Err(fault) => fault.to_string(),
            Ok(_) => panic!("two replicas with one name were accepted"),
        };
        assert!(error.contains("two replicas are called"), "{error}");
    }

    #[test]
    fn from_env_refuses_to_be_half_overridden() {
        let s3: config::S3 = toml::from_str("from_env = true\nbucket = \"other\"").unwrap();
        let error = s3_config(&s3).unwrap_err().to_string();
        assert!(error.contains("quietly ignored"), "{error}");
    }

    #[test]
    fn one_credential_without_the_other_is_refused() {
        let s3: config::S3 = toml::from_str("bucket = \"b\"\naccess_key_id_env = \"A\"").unwrap();
        let error = s3_config(&s3).unwrap_err().to_string();
        assert!(error.contains("both or neither"), "{error}");
    }
}
