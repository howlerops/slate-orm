# A view costs nothing measurable, which is what step 2 predicted — and finding that out found three benchmarks that had been broken for months.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #262 (F5h)
- **Touches:** `crates/slate-headbench/{src/harness.rs,examples/*.rs,tests/examples_reach_the_server.rs}`
- **Kind:** measurement, and one fix

## What changed

`head_report` gains a sixth section, `views`, and `harness::head_serving_views`
so a benchmark can stand up a node that has any. Three benchmarks that could
not run any more can. Two tests in a new `tests/` directory keep them running.

## Why

The step-2 entry recorded, in as many words, that a view's cost was *a
prediction from reading the code, not a measurement*: one `BTreeMap` lookup per
read and one `Expr::and`, with `and` folding `Expr::True` away, so a view's read
should cost what the same predicate costs from the caller and a node that
merely declares views should serve an ordinary read unchanged. This repository
has been wrong about that kind of claim before — `docs/performance.md` is full
of withdrawn ones — so it is measured now.

The benchmark fix was not planned and is the more valuable half. `leadership`
used to take `_request` and answer anybody who could reach the port, including
under `mode = "deny-all"`; the fix gave it the same `self.context(&request)`
every other handler has. Every benchmark that warmed its channel with a bare
`LeadershipRequest` has panicked on `UNAUTHENTICATED` at its first call ever
since — three of the five examples in this crate — and nobody noticed, because
`ci.yml` builds this crate and runs none of it. `cargo clippy --all-targets`
kept reporting that the examples compiled, which was true and not the question.

That is `CLAUDE.md`'s "a check that never fires is a check nobody has debugged",
pointed at a benchmark. An unrun benchmark is not slow to notice a regression;
it is unable to.

## Alternatives rejected

**Report the first numbers.** The first run said an ordinary read cost 71 µs
more on a node that declares one view — 1.02× — and 71 µs is a preposterous
price for a failed lookup in a `BTreeMap` of one. Reporting it would have been
a finding manufactured out of a measurement artefact. The two halves run
against *different nodes on different channels*, so anything drifting lands
entirely on the second; section 1 of this report already learned that and
re-measures its floor at the end. The same A-B-A here put the difference inside
the noise and showed why.

**Average the two control runs.** Would hide exactly what the second one is
for. They are printed side by side and the drift between them is printed under
the comparison.

**Run a benchmark in CI so this cannot recur.** The obvious fix and too
expensive: the `views` section alone is a release build of the whole dependency
tree, and in debug the warm-up is four thousand RPCs. What broke was never a
number — it was the call — so the two tests below are the smallest thing that
would have caught it, and they run under `cargo test --workspace` with
everything else.

**Name the view's read by renaming the table on the wire message.** How the
section was first written, and the server refused it: `query_to_proto` attaches
a schema claim, and `fingerprint::check_named` verifies it under the name the
request used, so a claim hashed as `events` is refused for a read of `recent`.
That is this morning's own fix catching this afternoon's shortcut. The section
now builds the view's request from a `TableDef` named `recent` carrying
`events`' columns — which is exactly what `scripts/codegen.py` emits for a
view, so the benchmark asks the question the way a real client would.

## Evidence

Seven runs per measurement, median with the range, on a four-core container;
2,000 rows, a predicate admitting 80 of them, asserted equal on both sides
before anything is timed.

**A view's read against the same predicate from the caller.** Two independent
runs of the section:

| | caller sends it | the view carries it | difference |
|---|---|---|---|
| run 1 | 3.17 ms [3.13 – 3.21] | 3.16 ms [3.16 – 3.18] | 9.39 µs — noise |
| run 2 | 3.17 ms [3.13 – 3.25] | 3.15 ms [3.12 – 3.18] | 20.21 µs — noise |

**An ordinary read, on a node with a view against one without**, measured
A-B-A:

| | no view | one view | no view again | difference | control drift |
|---|---|---|---|---|---|
| run 1 | 3.15 ms | 3.16 ms | 3.16 ms | 7.02 µs — noise | 6.51 µs |
| run 2 | 3.16 ms | 3.18 ms | 3.15 ms | 21.78 µs — noise | 11.10 µs |

Both halves of the prediction hold, and the report says "inside run-to-run
noise, not a finding" itself rather than my saying it: `Measure::separated_from`
decides, on the quantiles, and it declined to separate any of these four pairs.
**A null result, stated as one.**

`cargo test -p slate-headbench`: 2 passed, both new.
`the_harness_client_can_reach_the_harness_server` is a single authenticated
round trip against a node the harness stood up — deliberately not a
measurement, because what broke was the call.
`no_example_sends_a_request_with_no_identity` reads the five example sources
and refuses `tonic::Request::new(pb::`, with a never-fires half that requires
it to have read at least five files, since a loop that read nothing would
report success over a tree it never looked at.

Mutations via `scripts/mutate.py`, three, all caught by a named test: an
example put back to a bare request (`no_example_sends_a_request_with_no_
identity`), the source check narrowed to read no files at all (same test, by
the never-fires half), and the round trip's identity dropped
(`the_harness_client_can_reach_the_harness_server`).

The floor's label in section 1 said "no storage, no auth" and now says
"authenticated", because it is — that measurement moved when `leadership` did
and the label did not follow it.

## What this does not do

**It does not measure a view at scale.** 2,000 rows and a predicate admitting
80. The composition is one `Expr::and` whichever way, so nothing about the
shape suggests the answer changes with row count — but that is the same kind of
reasoning this section exists to check, and it is untested above 2,000.

**It does not measure a registry of more than one view.** The lookup is a
`BTreeMap::get` and the second half above measures a map of one against a map
of none, which is the step from zero. A hundred views is `log n` on a tiny map
and would be measured the same way; nobody has.

**Nothing runs the examples themselves, still.** The source check sees the
shape that broke three files at once; it cannot see an example that
authenticates *wrongly*, or one that breaks for any other reason. The five
benchmarks remain run by hand, and `head_concurrency` and `stream_step` were
fixed here by inspection — their leadership calls now carry an identity, and
neither was executed end to end afterwards, because each takes minutes.
`head_report`'s `views` section is the only one this session actually ran.

**The 71 µs is not explained, only shown to be an artefact.** It reproduced as
"the second node measured is slower" and vanished under A-B-A. Whether that is
the channel settling, the allocator, or something about standing up a second
`Serving` in the same process was not chased.
