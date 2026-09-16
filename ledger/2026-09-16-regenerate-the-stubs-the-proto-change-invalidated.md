# Regenerate the stubs and the bundled proto that the predicate-write change invalidated

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `clients/python/src/slate/_proto/`, `clients/typescript/proto/slate/v1/records.proto`
- **Kind:** fix

## What changed

Regenerated the committed Python protobuf stubs and copied the server's
`records.proto` over the TypeScript package's bundled copy, both stale since
`DeleteWhere`, `UpdateWhere`, `Assignment` and `WriteResponse.rows` were added
to the proto.

No hand-written code. Every line is generated or copied.

## Why

CI run 104 went red on exactly two jobs, Python and TypeScript, and both were
this repository's own guards firing correctly:

- `test_the_committed_stubs_match_the_proto` regenerates the stubs and compares
  them byte for byte. 187 passed, this one failed.
- `the bundled proto matches the server's` compares the shipped copy with
  `crates/slate-server/proto`. Its docstring says "copy the file across; do not
  edit the copy", which is what was done.

Both exist because a client built from a stale protocol definition does not
fail to build — it fails at runtime, decoding fields into the wrong shape,
which is the worst way for it to fail. The guards turn that into a red job.

~~The Go client passed, because it generates from the proto at build time and
has nothing committed to go stale. That asymmetry is the reason the other two
need a guard at all.~~

**Withdrawn the same day, and wrong when written.** Go commits its stubs like
Python does, and it has a guard — `TestStubsAreFresh` — that regenerates and
compares byte for byte. It passed because it *skipped*: the test needs `protoc`
and no CI job installed one, while its own comment claimed CI did. The Go stubs
were stale in that very commit, missing `Assignment`, `DeleteWhereRequest` and
`UpdateWhereRequest`, and the job was green. I did not check before writing the
explanation; the next commit installs `protoc` and makes the skip fatal under
`SLATE_REQUIRE_PROTOC`.

## Alternatives rejected

**Regenerate with whatever `pip` resolves.** The failure message warns against
exactly this: the generator versions are part of the output, and regenerating
with a different one is a change to the stubs rather than a fix. The installed
versions here — `grpcio-tools` 1.83.1, `mypy-protobuf` 5.1.0, `protobuf`
7.36.1 — were checked against the versions CI reported in its failure before
running the generator, and they match.

**Generate the Python stubs at install time instead of committing them.** It
would delete this failure mode. It also makes installing the client require a
protobuf toolchain, which is the reason they are committed. The guard is the
cheaper half of that trade.

**Make the TypeScript package read the server's proto directly.** Only works in
a checkout of this repository; the package ships to people who do not have the
Rust tree. Hence a copy, hence a copy that can drift, hence the check.

## Evidence

Python 192 passed. TypeScript 91 passed. Go passed too, and that turned out to
mean less than it appeared — see the withdrawal above. Before the fix, TypeScript failed 78
of 91 — not from the proto drift but from a *second* guard underneath it: the
prebuilt `slate-serverd` predated the source, and the harness refused to test
"a server this tree did not produce" rather than testing a stale binary
quietly. Rebuilding cleared it. Two guards, two correct refusals, one after the
other.

## What this does not do

Nothing stops the next proto change from doing this again. The guards catch it
after the fact, in CI, rather than at the moment the proto is edited — a
pre-commit check that regenerated and compared would catch it three minutes
earlier, and would need the Python toolchain present at commit time for every
agent working here, which it is not. The trade is deliberate and the cost is a
red CI run.

The binary-staleness guard names whatever source file changed most recently,
which here was a *test* file that cannot affect the server binary. It is
conservative rather than precise: it cannot know which files the binary depends
on, and refusing too often is the right failure for this check. Left alone.
