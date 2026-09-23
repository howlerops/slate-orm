//! A real head node, on a real socket, over a real backend.
//!
//! Two shortcuts were deliberately not taken.
//!
//! **The server is served over loopback TCP**, not an in-memory duplex channel.
//! An in-process channel skips the framing, the syscalls and the scheduler
//! hand-off, which between them are most of what "per-RPC overhead" means. A
//! harness that skipped them would measure protobuf encoding and call it the
//! head node.
//!
//! **The backend is SlateDB**, opened over an in-memory object store. In-memory
//! *object store*, not in-memory key-value map: SlateDB's WAL, memtable,
//! manifest and reader are all in the path, so a replica genuinely polls a
//! manifest and genuinely lags. What is missing is the network under the object
//! store, and that omission is stated wherever it changes what a number means.

use futures::TryStreamExt;
use slate_kernel::memory::MemoryStore;
use slate_kernel::{KvReadStore, KvStore, RecordStore, ReplicaPool, SecurityCatalog};
use slate_schema::Catalog;
use slate_server::leadership::Leadership;
use slate_server::lease::{Lease, LeaseError, Term};
use slate_server::proto::records_client::RecordsClient;
use slate_server::{Head, HeadConfig, Limits, MetadataIdentity, Views};
use slate_slatedb::{SlateReader, SlateStore};
use slatedb::Db;
use slatedb::config::DbReaderOptions;
use slatedb::object_store::ObjectStore;
use slatedb::object_store::memory::InMemory;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};
use tokio::task::JoinHandle;
use tonic::transport::Channel;

/// Where the fixture database lives inside the object store.
pub const DB_PATH: &str = "/records";

// --- leadership -----------------------------------------------------------

/// A lease that always grants, for the measurements that are not about leases.
///
/// The lease measurement uses a real [`slate_server::lease::ObjectStoreLease`];
/// everything else needs the node to be the leader and needs the check on the
/// write path to cost what it costs, which is a watch-channel read either way.
#[derive(Debug, Default)]
pub struct AlwaysLeader {
    generation: AtomicU64,
}

#[tonic::async_trait]
impl Lease for AlwaysLeader {
    async fn acquire(&self) -> Result<Term, LeaseError> {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Term {
            generation,
            holder: "bench".to_owned(),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        })
    }

    async fn renew(&self) -> Result<Term, LeaseError> {
        self.acquire().await
    }

    async fn release(&self) -> Result<(), LeaseError> {
        Ok(())
    }

    async fn observe(&self) -> Result<Option<Term>, LeaseError> {
        Ok(None)
    }

    fn held(&self) -> Option<Term> {
        None
    }

    fn holder(&self) -> &str {
        "bench"
    }
}

/// A `Leadership` that already holds the writer role.
pub async fn leading() -> Arc<Leadership> {
    let leadership = Leadership::new(Arc::new(AlwaysLeader::default()));
    assert!(leadership.campaign().await, "the fake lease always grants");
    leadership
}

// --- serving --------------------------------------------------------------

/// A head node and the socket it answers on.
#[derive(Debug)]
pub struct Serving {
    /// The loopback address the operating system chose.
    pub address: SocketAddr,
    server: JoinHandle<()>,
}

impl Serving {
    /// A client connected to it.
    ///
    /// One channel per client, and the caller keeps it for the whole
    /// measurement: reconnecting per operation would measure TCP setup.
    pub async fn client(&self) -> RecordsClient<Channel> {
        RecordsClient::connect(format!("http://{}", self.address))
            .await
            .expect("connect to the head node")
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Serve `head` on a loopback port the operating system chooses.
///
/// `TCP_NODELAY` is on, which is what `tonic::transport::Server` does by
/// default — and what this harness was **not** doing. See
/// [`serve_with_nagle`], which exists so the difference can be measured rather
/// than argued about.
pub async fn serve<S: KvStore + KvReadStore>(head: Head<S>) -> Serving {
    serve_with_nagle(head, false).await
}

/// The same, with Nagle's algorithm left on when `nagle` is true.
///
/// # Why this switch exists
///
/// `tonic::transport::Server` sets `tcp_nodelay` to **true** by default, but
/// its own documentation says the setting "is ignored when using this method"
/// for [`serve_with_incoming`], which is what a harness that wants to choose
/// its own port has to call. So every measurement this crate has taken ran
/// against a socket with Nagle's algorithm enabled, while every real
/// deployment runs against one without it.
///
/// That is not a detail. Nagle holds a small write back until the previous
/// one is acknowledged, and Linux delays an acknowledgement by up to 40 ms —
/// so a response written as *two* small segments (a stream's header message,
/// then its first batch of rows) can stall for tens of milliseconds while a
/// response written as one does not. A unary reply is one write and never
/// notices; a stream is two and does.
///
/// [`serve_with_incoming`]: tonic::transport::Server::serve_with_incoming
pub async fn serve_with_nagle<S: KvStore + KvReadStore>(head: Head<S>, nagle: bool) -> Serving {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let address = listener.local_addr().expect("local address");
    let incoming =
        tokio_stream::wrappers::TcpListenerStream::new(listener).map_ok(move |connection| {
            // `set_nodelay` can only fail on a socket that is already broken,
            // and a broken connection is the server's problem a moment later
            // rather than the harness's now.
            let _ = connection.set_nodelay(!nagle);
            connection
        });

    let server = tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(head.into_service())
            .serve_with_incoming(incoming)
            .await;
    });

    Serving { address, server }
}

/// A head node over `writer`, reading through `replicas`, with `limits`.
pub fn head<S: KvStore + KvReadStore>(
    catalog: Catalog,
    security: SecurityCatalog,
    writer: Arc<S>,
    replicas: Vec<Arc<dyn KvReadStore>>,
    leadership: Arc<Leadership>,
    limits: Limits,
) -> Head<S> {
    head_serving_views(
        catalog,
        security,
        writer,
        replicas,
        leadership,
        limits,
        Views::new(),
    )
}

/// [`head`], with a view registry installed.
///
/// A separate function rather than a seventh argument on `head`, because every
/// existing caller wants an empty registry and would have to say so. `Views`
/// is `BTreeMap`-shaped, so the empty case costs one failed lookup per read,
/// and whether that is measurable at all is the question the report's `views`
/// section exists to answer — which it cannot ask if the benchmark harness
/// cannot build a node that has any.
pub fn head_serving_views<S: KvStore + KvReadStore>(
    catalog: Catalog,
    security: SecurityCatalog,
    writer: Arc<S>,
    replicas: Vec<Arc<dyn KvReadStore>>,
    leadership: Arc<Leadership>,
    limits: Limits,
    views: Views,
) -> Head<S> {
    Head::new(
        HeadConfig::new(catalog, security).with_limits(limits),
        writer,
        replicas,
        leadership,
        Arc::new(MetadataIdentity::trusting_the_caller_completely()),
    )
    .serving_views(views)
}

/// The same kernel the head node holds, assembled the same way, reachable
/// without a socket.
///
/// This is the control for every "what does the head node cost" comparison, so
/// it has to be the *same* pool over the *same* stores: a control that used a
/// different catalog, a different statistics set or a different replica list
/// would be measuring something else and the difference would be attributed to
/// gRPC.
#[derive(Debug)]
pub struct InProcess<S> {
    /// The writer, as `Head` builds it.
    pub writer: RecordStore<Arc<S>>,
    /// The read pool, as `Head` builds it.
    pub pool: ReplicaPool,
}

impl<S: KvStore + KvReadStore> InProcess<S> {
    /// Build the same two objects `Head::new` builds, over the same stores.
    #[must_use]
    pub fn new(
        catalog: Catalog,
        security: SecurityCatalog,
        writer: Arc<S>,
        replicas: Vec<Arc<dyn KvReadStore>>,
    ) -> Self {
        let pool = ReplicaPool::new(replicas, catalog.clone(), security.clone())
            .with_writer(Arc::clone(&writer) as Arc<dyn KvReadStore>);
        Self {
            writer: RecordStore::new(writer, catalog, security),
            pool,
        }
    }
}

// --- backends -------------------------------------------------------------

/// A SlateDB writer and the object store under it.
pub struct Backend {
    /// The object store both the writer and the replicas read.
    pub object_store: Arc<dyn ObjectStore>,
    /// The database handle, shared by every [`SlateStore`] view of it.
    pub db: Arc<Db>,
}

impl fmt::Debug for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `Db` is not `Debug`; the path and the store's own name are what a
        // reader of a bench log would want anyway.
        f.debug_struct("Backend")
            .field("path", &DB_PATH)
            .field("object_store", &self.object_store.to_string())
            .finish_non_exhaustive()
    }
}

impl Backend {
    /// Open a fresh database over a fresh in-memory object store.
    pub async fn open() -> Self {
        let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let db = Db::builder(DB_PATH, Arc::clone(&object_store))
            .build()
            .await
            .expect("open SlateDB");
        Self {
            object_store,
            db: Arc::new(db),
        }
    }

    /// A store view of the database, waiting for object storage on commit.
    ///
    /// The default, and what read-your-writes through a replica requires: a
    /// replica reads object storage, so a write that has not been flushed is
    /// not one any replica can see.
    #[must_use]
    pub fn durable(&self) -> Arc<SlateStore> {
        Arc::new(SlateStore::from_db(Arc::clone(&self.db)))
    }

    /// A store view whose commits return as soon as the write is visible.
    ///
    /// Not a faster mode of the same thing: a write acknowledged this way can
    /// be lost. It is here because SlateDB's WAL flushes on a 100 ms timer, so
    /// a durable commit spends up to 100 ms waiting for a clock — which buries
    /// the tens of microseconds this crate is trying to see on the write path.
    #[must_use]
    pub fn visible(&self) -> Arc<SlateStore> {
        Arc::new(
            SlateStore::from_db(Arc::clone(&self.db))
                .with_durability(slate_slatedb::Durability::Visible),
        )
    }

    /// Open `count` following replicas, polling the manifest every
    /// `poll_interval`.
    pub async fn replicas(
        &self,
        count: usize,
        poll_interval: Duration,
    ) -> Vec<Arc<dyn KvReadStore>> {
        let options = DbReaderOptions {
            manifest_poll_interval: poll_interval,
            checkpoint_lifetime: Duration::from_secs(300),
            ..DbReaderOptions::default()
        };
        let mut replicas: Vec<Arc<dyn KvReadStore>> = Vec::new();
        for index in 0..count {
            let name = format!(
                "replica-{}",
                (b'a' + u8::try_from(index).unwrap_or(0)) as char
            );
            replicas.push(Arc::new(
                SlateReader::open_with(
                    name,
                    DB_PATH,
                    Arc::clone(&self.object_store),
                    slate_slatedb::ReplicaMode::Following,
                    options.clone(),
                )
                .await
                .expect("open a replica"),
            ));
        }
        replicas
    }
}

/// The kernel's own in-memory key-value store.
///
/// Not a backend anyone would deploy. It is here as the *other end* of the
/// per-RPC comparison: with the storage cost driven to nearly nothing, whatever
/// is left of the gRPC-minus-in-process difference is the head node, and if
/// that difference does not agree with the one measured over SlateDB then one
/// of the two measurements is wrong.
#[must_use]
pub fn memory() -> Arc<MemoryStore> {
    Arc::new(MemoryStore::new())
}
