//! The command line.
//!
//! `clap` rather than a hand-rolled loop. `clients/python/testserver` hand-rolls
//! one and it panics on an unrecognised flag with a message that names no
//! alternative, which is the state of affairs this binary exists to end. What
//! is being bought is not argument parsing — that is twenty lines — but
//! `--help`, `--version`, a suggestion on a near-miss, and the guarantee that
//! `--config` without a value is an error rather than a `None` somewhere.

use clap::Parser;
use std::path::PathBuf;

/// A head node over the slate-orm record layer.
#[derive(Debug, Parser)]
#[command(
    name = "slate-serverd",
    version,
    about = "A head node over the slate-orm record layer: gRPC, writer leadership, row-level security.",
    long_about = LONG_ABOUT,
    after_help = AFTER_HELP,
)]
pub(crate) struct Cli {
    /// The configuration file.
    #[arg(short, long, value_name = "FILE")]
    pub(crate) config: PathBuf,

    /// Bind this address instead of the one in the file.
    ///
    /// The authentication rules are applied to whichever address is actually
    /// bound, so overriding a loopback address with a public one can turn a
    /// valid configuration into a refusal. That is the intended behaviour.
    #[arg(long, value_name = "ADDR")]
    pub(crate) listen: Option<String>,

    /// Insert fixture rows from FILE before serving, as a superuser.
    ///
    /// For development and for a client's test harness. Not a configuration
    /// setting, so a file copied between environments cannot carry it.
    #[arg(long, value_name = "FILE")]
    pub(crate) seed: Option<PathBuf>,

    /// Check the configuration and exit, without opening storage or binding.
    #[arg(long)]
    pub(crate) check: bool,

    /// Print the resolved schema as JSON and exit.
    ///
    /// Column names, ordinals and types, as the server would serve them —
    /// which is what a client in another language has to restate by hand.
    #[arg(long)]
    pub(crate) print_schema: bool,
}

const LONG_ABOUT: &str = "\
A head node over the slate-orm record layer: gRPC, writer leadership over an
object-store lease, and row-level security.

Everything the node serves comes from one TOML file: the tables, the indexes,
the constraints, the roles and policies, the storage backend, the replicas, and
— with no default and no way to omit it — how callers are authenticated.

The node prints `LISTENING <address>` on standard output once its socket is
bound and before it accepts anything, so a harness can wait for that line
rather than poll the port. `address = \"127.0.0.1:0\"` binds a free port and
the banner reports which.";

const AFTER_HELP: &str = "\
A configuration that runs:

  [listen]
  address = \"127.0.0.1:50051\"

  [auth]
  mode = \"trusted-header\"          # deny-all | trusted-header | token

  [storage]
  backend = \"memory\"               # memory | local | s3

  [[tables]]
  name = \"docs\"
  id = 1
  columns = [
    { name = \"id\",   type = \"u64\" },
    { name = \"kind\", type = \"str\" },
    { name = \"size\", type = \"i64\" },
  ]
  primary_key = [\"id\"]

  [[tables.indexes]]
  name = \"by_kind\"
  id = 1
  columns = [\"kind\"]
  where = \"size > 0\"               # a partial index

  [[security.grants]]
  role = \"app\"
  tables = [\"docs\"]
  actions = [\"all\"]

  [[security.policies]]
  name = \"big_only\"
  table = \"docs\"
  actions = [\"read\"]
  using = \"size >= 10\"             # `:principal` and `:tenant` are available

Every refusal names the setting it is about. `--check` validates the file
without opening storage or binding a socket.";
