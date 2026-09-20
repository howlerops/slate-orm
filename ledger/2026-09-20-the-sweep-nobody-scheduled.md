# The sweep nobody scheduled

## What changed

`examples/retention/` — a retention job that calls `purge_deleted` on a
schedule, and a harness that runs it against a real head node and fails if the
retired row survives. Four files: `purge.py` (the example), `head.toml` (a
one-table catalog with a soft delete), `seed.py` (the before and the after),
`run.sh` (the harness). Wired into CI's `demo` job.

## Why

Soft delete shipped with `purge_deleted` and nothing that runs it.
`ledger/2026-09-19-a-purge-something-can-call.md` named it: *"No scheduler.
Nothing runs a purge periodically. A deployment wanting one needs an external
cron calling the RPC, and there is no example of that here."*

Which means the convention, as shipped, made a soft-deleting table grow
without bound in every deployment that used it. The RPC existing is not the
same as anybody being able to run it on a Tuesday.

## Alternatives rejected

**A scheduler inside `slate-serverd`,** configured by a `[retention]` section.
The stronger-sounding answer, and I did not take it, because it needs three
decisions that belong to a deployment rather than to a database:

- *Whose identity does it run as?* A purge is privileged. An internal sweep
  needs a principal, and inventing one means the server holds a credential no
  operator granted.
- *Does it run on a follower?* Only the leaseholder may write, so either the
  sweep is leader-only — new coupling between retention and leadership — or it
  fails noisily on every replica.
- *What happens when it overruns its interval?* Skip, queue, or overlap: three
  answers, all defensible, none the database's to pick.

An external caller answers all three by existing: it holds its own credentials,
it points at whichever node it is told to, and if it is slow the next run is
simply late. That is an argument, not a proof, and it is written here so the
next person can disagree with it on the merits.

**A shell one-liner in the docs.** Would have "closed" the gap in ten minutes
and taught nothing about the two things that actually bite — the privileges a
sweep needs, and the ceiling.

**Run it against the explorer's schema.** The only other soft-deleting table in
the repository. Rejected: the demo's catalog changes for the demo's reasons, and
a harness that fails when somebody adds a column to `shipments` is a harness
people learn to ignore. `examples/retention/head.toml` is one table and shares
nothing.

## Evidence

**The harness fails when the sweep does nothing.** Run with `--older-than 1h`
against a row retired seconds ago, so the sweep erases zero:

```
notes: erased 0 row(s) retired before 1789910982
after the sweep the table holds [1, 2], expected [2] — the retired row survived
$ echo $?
1
```

and with `--older-than 0s`, exit 0 and `verified: 1 is gone, 2 is untouched`.
There is a live row beside the retired one throughout: without it "the table is
empty" would pass for a sweep that erased everything.

**That check was broken, and running it the wrong way is what found it.** The
first version of `verify` set `query.include_deleted = True`. `include_deleted`
is a *method*, so the assignment replaced the bound method with a boolean, the
query asked an ordinary question, and the retired row was invisible — meaning
the check reported success whether or not the sweep had run. It passed the
positive case for entirely the wrong reason, and only the negative run exposed
it. A harness that cannot fail is the thing this directory exists to avoid, and
it very nearly shipped as one.

**The cleanup decided the exit code.** With that fixed, the *passing* run
started exiting 1: `rm -rf` in the `EXIT` trap raced the dying node, failed with
"Directory not empty", and its status became the script's. The trap now waits
for the process, ignores the removal's status and re-exits with the body's.
Two ways for the harness to lie, both found by running it rather than reading
it.

**The privileges a sweep needs were established by removal, not by guessing.**
Starting from `actions = ["all"]` the purge was refused for `read_deleted`;
adding that, it was refused for `read`. The retention role holds `read`,
`delete` and `read_deleted` — and the application's role deliberately holds
none of `read_deleted`, so the example demonstrates least privilege rather than
running the job as a superuser. That `all` does not include `read_deleted` is
the kind of thing an operator finds out at 3am, and the config says so in a
comment now.

**The generated declaration is what a retention job imports.** `purge_deleted`
takes a `Table`, not a name, because a purge carries the same schema check as
any other request — so `--schema` points at the module `codegen.py` wrote and
resolves names through its `BY_NAME`. Taking a bare string would have meant this
script keeping a second copy of the catalog.

**Checks:** `scripts/test_check_sh.py` — 62 steps and 5 blocks accounted for.
`sh scripts/check.sh` — 20 of 20. `ruff check .` and `ty check` clean. The
harness passes both with `SLATE_SERVERD` set and unset.

## What this does not do

- **No offline tool.** A stopped node's keyspace still cannot be purged; this
  needs a running server to talk to. Unchanged from the entry it closes.
- **The ceiling is still a count**, and the script reports when it hits one
  rather than looping until the backlog clears. Deliberate: a sweep that keeps
  going until it finishes is a sweep that can run for an hour holding a writer.
  An operator who sees "hit the ceiling" raises `--at-most` or shortens the
  interval, both of which are decisions.
- **Nothing measures a sweep's cost.** How long a purge of a million rows takes,
  and what it does to concurrent writes, is unknown and unasked.
- **No `systemd` unit, `cron` line or `CronJob` manifest.** The script's
  docstring says which of `--once` and `--every` suits which, and stops there —
  a worked Kubernetes manifest would date faster than the argument.
- **One language.** The example is Python because the retention job is a script
  and the Python client is the one with a scripting ergonomic. Go and TypeScript
  can call `purge_deleted` and the conformance corpus covers that they agree;
  there is no Go or Node version of this example and I do not think there should
  be.
