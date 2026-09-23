# Nine pushes over a red branch

## What changed

One line in `crates/slate-server/tests/common/mod.rs`: the `.foreign_key(...)`
call in `mentions()`, collapsed onto the single 97-character line CI's
`rustfmt` wants. `CLAUDE.md` gained a paragraph saying CI's `rustfmt` is newer
than this container's, in the place it already says that about clippy.

## Why

Two things went wrong and only one of them is about formatting.

**The formatting.** Local `rustfmt 1.8.0-stable` accepted the hand-wrapped
builder call; CI's, from `dtolnay/rust-toolchain@stable`, rewrapped it. `cargo
fmt --all -- --check` was clean here and the `formatting` job was red there.
This is the clippy gap `CLAUDE.md` documents in detail — three commits red on
lints that do not exist locally — arriving through a different tool, and the
note did not mention `rustfmt`.

**The part that matters.** That job was red on `a51103e` at 13:40 and I pushed
**nine more commits** without noticing, because of how I was watching. Each
monitor polled for a *specific* commit's conclusion; every new push cancelled
the previous run, so the monitor's sha never reached a terminal state and it
expired silently thirty minutes later. Seven runs read `cancelled`, two read
`failure`, and I read none of them.

So: a watcher that fires only on a condition that a normal workflow prevents
from ever holding. That is this repository's own recurring defect — the
workflow triggering on a branch that never existed, the suite skipping itself,
the counter nobody scraped — committed by the thing that was supposed to be
watching for it. Locally green is necessary and never sufficient, and I spent
the day acting as though it were sufficient because the signal that would have
told me otherwise was pointed at the wrong commit.

## Alternatives rejected

**Re-run `cargo fmt --all` locally and push whatever it produces.** What I
would have done if the log had not been explicit: it produces the file as it
already is, because the local formatter thinks it is correct. The only
authority on what CI's formatter wants is CI's formatter, and its log prints
the exact diff. Applied verbatim.

**Pin `rustfmt` in `ci.yml` to the local version.** Makes the two agree, and
freezes formatting to whatever this container happens to have — the opposite of
what a `stable` toolchain is for, and it would date. The clippy note already
chose the other way for the same reason.

**Install a second toolchain here to check against.** `CLAUDE.md` says this is
usually not possible in this container, and the disk note is why; today's two
ENOSPC failures are the evidence that it still is not.

**Add a `fmt`-tolerant wrapper to `scripts/check.sh`.** There is nothing to
tolerate: the script runs the right command against the wrong binary. A local
check cannot close a gap whose cause is that the local tool is older.

## Evidence

**Every run on the branch, read rather than assumed.** Twelve most recent:
`547dd04` success (yesterday, the last verdict I actually saw), then `a51103e`
**failure**, `b7a8605` cancelled, `3d7612f` **failure**, and `7707513`,
`d47c7c8`, `8ab0667`, `f1e9268`, `573d77a` all cancelled by the next push.
`092b580` in progress.

**Both failures are the same job and the same step:** `formatting` →
`cargo fmt --all -- --check`. Fetched from the API per run rather than inferred.

**One file, one site.** `grep -oE "Diff in .*"` over the job log returns
`crates/slate-server/tests/common/mod.rs:194` and nothing else, so the whole of
today's Rust work is otherwise formatted the way CI wants.

**The diff CI asked for**, applied unchanged:

```
-        .foreign_key(
-            slate_schema::ForeignKeyDef::builder("mentions_doc", DOCS).column("doc_id"),
-        )
+        .foreign_key(slate_schema::ForeignKeyDef::builder("mentions_doc", DOCS).column("doc_id"))
```

97 characters, inside the 100-column default, which is why the newer formatter
joins it and why the older one is content either way.

**After the change:** `cargo fmt --all -- --check` clean locally,
`cargo test -p slate-server --test transaction_counts` 16 passed.

## What this does not do

- **It does not prove the rest of the branch is green.** Seven runs were
  cancelled before finishing, so the suites, the conformance runner, the
  browser checks and the deployed harness have *not* reported on any of
  today's nine commits. The formatting job failed fast and everything after it
  in those runs is unknown. The next full run is the first real verdict, and
  until it lands the honest statement is "locally verified, CI unconfirmed".
- **It does not fix the watching.** A monitor that follows the branch head
  rather than a fixed sha is the obvious answer and it is a property of how I
  work, not of this repository, so there is nothing here to commit. Named
  because the next session will have the same trap.
- **No new guard.** A check that compares two rustfmt versions would need two
  rustfmt versions. The honest mitigation is the `CLAUDE.md` paragraph and the
  habit of letting the formatter wrap rather than wrapping by hand.
- **Nothing measured.** A line break has no cost to report.
