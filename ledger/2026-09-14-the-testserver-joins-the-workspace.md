# The test server joins the workspace it should never have left

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `Cargo.toml`, `clients/python/testserver/Cargo.toml`, its lockfile
- **Kind:** repair

## What changed

`clients/python/testserver` is a member of the root workspace. Its second
`Cargo.lock` is gone, and its shared dependencies come from
`[workspace.dependencies]` rather than repeating paths.

## Why

Three separate things rotted in that crate yesterday, all invisible for the
same reason: `cargo test --workspace` never built it.

1. A `.map()` over a listener stream whose `StreamExt` import was never added —
   left behind by the TCP_NODELAY commit, which was never compiled against this
   crate.
2. Four indexes sharing id 1, from when index ids became unique per catalog
   rather than per table.
3. Every `EXPLAIN` test failing, because `EXPLAIN` became its own action and
   `Action::ALL` stopped carrying it — working exactly as designed, against a
   crate that still granted `ALL`.

Each sat broken until someone ran the Python suite. 930 passing workspace tests
said nothing about any of them.

The crate's own comment explained the detachment: the work that created it "is
not allowed to touch" the workspace manifest. That was a real constraint at the
time and it is gone.

## Alternatives rejected

**Deleting the crate and using `slate-serverd`.** Checked first, because
deleting 972 lines beats moving them. It does not work: these tests need a
replica that will *never* catch up and a lease somebody else holds forever, and
`slate-serverd` deliberately cannot produce either. A production daemon should
not ship a `--never-catch-up` flag to make a test suite easier. The fixture is
legitimate; its detachment was not.

**A CI job that builds it separately.** Closes the same hole and leaves two
lockfiles that can resolve different versions of the same dependency — which
the crate's own comment already identified as a hazard and worked around by
copying the parent's lock in a shell script. One workspace removes the hazard
and the script.

**Moving it to `crates/`.** It is the Python client's test fixture, not part of
the database, and it belongs beside the tests that use it. Workspace membership
and directory location are independent; only the first one mattered.

## Evidence

The mutation that matters: removing the `StreamExt` import again — the exact
first rot — now fails `cargo build --workspace` with 5 errors. Before this
change it failed nothing.

941 workspace tests pass, clippy clean, 138 Python tests pass. The Python
suite's `conftest` needed no change: it already built with
`CARGO_TARGET_DIR` pointed at the workspace target, so the binary lands where
it always did.

## What this does not do

`scripts/build_testserver.sh` is now unnecessary — it existed to copy the
parent lockfile in — and is left in place rather than deleted, because nothing
in this commit verified who else calls it.

The demo's three backends (`examples/explorer/backends/`) are outside any
workspace in the same way: a Go module, an npm package and a Python package
that no root-level command builds. They are examples rather than fixtures, and
the same class of rot applies to them.
