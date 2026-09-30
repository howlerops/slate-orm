# Fifty-one of ninety-nine, read at random, one wrong

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `docs/caveat-status.json`
- **Kind:** docs

## What changed

The random sample extended from 12 to **51 distinct open caveats** — 52% of the
99 that predate today. One moved: "Nothing in the demo or the conformance
corpus catches a check violation and renders it" is **narrowed**, because
`examples/explorer/conformance/conformance.py` runs `"a write the schema's
CHECK refuses"` against a live server and compares all three clients on the
decoded `violations`, with a batch variant beside it. Its own comment records
that it replaced the captured-blob testing the caveat names. The residual is
the demo's *browser* UI, which still renders no violation.

Fifty hold.

## Why

`ledger/2026-09-28-a-random-twelve-found-nothing.md` said twelve bounds the
failure rate below about 22% and no lower, and that forty would bring it to
about 7%. Fifty-one with one failure gives roughly **2% observed, with a 95%
upper bound near 9%** — which is the first statement about the open backlog
that is worth anything.

Set against the biased sample's 4-in-25, the two now say different things
clearly: **where the tree has recently moved, roughly a sixth of the open
caveats are stale; across the backlog as a whole, roughly one in fifty.** The
five found by accident yesterday were not a sign the list was rotten. They were
a sign that the parts of the list somebody is actively working near go stale
fast, which is exactly where a session looking for work will land.

A methodological correction, because the previous entry got it wrong. It said
the recorded seed makes twelve extensible to forty. It does not:
`random.sample(pop, 12)` and `random.sample(pop, 40)` share no construction, so
the second is a fresh draw rather than a superset. What *is* extensible is a
shuffle — `random.shuffle(order)` once, then prefixes of `order` nest by
construction. This tranche used the shuffle, and by luck it overlapped the
first twelve in exactly one caveat, so the union is 51 rather than 52. Future
increments should take longer prefixes of the shuffle and ignore the `sample`
call entirely.

## Alternatives rejected

**Re-draw all 51 from the shuffle and discard the first twelve.** Cleaner
provenance — one method, one seed — and it would mean re-reading twelve
caveats to change nothing. Rejected as ceremony: the twelve were read against
the same tree by the same method of *reading*, and only the selection differed.
The union is what was read and is stated as a union.

**Report "2%".** One failure in fifty-one is 2% and the interval matters more
than the point: the 95% upper bound is near 9%, so "one in fifty" is the
estimate and "worse than one in eleven is unlikely" is the claim. The previous
entry made this mistake's mirror image by nearly quoting a zero, and got
corrected in its own text; quoting a 2% here would be the same error with a
different number.

**Push the sample to all 99.** Forty-eight more reads at the rate this took is
most of another session, and the marginal information is small: the interval is
already tight enough to act on, and the action it implies — attention on
recently-moved code rather than on the backlog's length — does not change
between 9% and 5%. Worth doing when somebody is already reading in that area,
not as its own task.

## Evidence

- Draw A, recorded in the previous entry: `random.seed(20260928)` then
  `random.sample(pop, 12)` over the 99 pre-today `open` verdicts. Twelve read,
  zero moved.
- Draw B, this entry: same seed, `random.shuffle(order)` over the same 99, then
  `order[:40]`. One of the forty was already in draw A, leaving 39 new. Read:
  38 hold, one narrowed.
- Of the 39: **fifteen settled by conclusive absence** — a grep over every
  place the feature could live returning nothing (no service worker in `site/`,
  no `remap-path-prefix` in the wasm build, no `withFreshness` in the Go or
  TypeScript clients, no `includeDeleted` or `ifUnchanged` control in the demo's
  web source, no `returning_projection` in the proto, no `DeleteOutcome`, no
  purge or erase test in `slate-slatedb`, and eight more).
- **Eight had hits and were read.** Seven held anyway, which is the same lesson
  as the previous two entries: `crates/slate-kernel/src/migrate.rs` matched
  "diff" in a heading that argues *against* a diff — "Why a fingerprint and not
  a diff"; the Python adapter matched "every table" in a comment reading "which
  is *not* every table in the catalog"; `scripts/test_codegen.py` matched
  "importable" about the web module rather than about Go or Python packaging;
  and `crates/slate-server/tests/status.rs` matched nothing for "lag" once the
  neighbouring file's prose was excluded. The eighth was the real one.
- `python3 scripts/caveats.py`: 1582 caveats, 120 open, 65 narrowed, 373
  closed, 891 deliberate, 0 untriaged. 118 open before this entry's own three
  were added.

## What this does not do

**It leaves forty-eight of the ninety-nine unread**, and the shuffle order says
which they are. The interval quoted above assumes the 51 are representative,
which the shuffle makes true of the *draw* and not of the reading: a caveat
settled by a grep returning nothing is settled less thoroughly than one settled
by reading a function, and fifteen of the 39 were the former.

**It still says nothing about the 890 `deliberate` verdicts.** They outnumber
the open ones seven to one, a `deliberate` that has stopped being defensible is
invisible, and no sample has been drawn from them. That is now the largest
unexamined population in the tracker by a wide margin, and this entry is the
third to say so without doing it.

**The rate is about caveats, not about the system.** A caveat being true says
the gap it names is still there, which is the tracker working; it says nothing
about whether the gap matters. Fifty confirmations that the demo UI lacks a
control and the corpus lacks a case are fifty true statements about a system
whose shape nobody re-examined here.
