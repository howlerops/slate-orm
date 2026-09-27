# The last twenty-six open caveats, and one hazard nobody had written down

## What changed

The open list is empty. Twenty-six verdicts: twenty-five `deliberate`, one
`narrowed`. 1,023 caveats now read 0 open, 32 narrowed, 242 closed, 671
deliberate, 0 untriaged.

One of the twenty-six could not honestly be decided as it stood, and the change
that made it decidable is the only code in this commit.
`crates/slate-kernel/src/migrate.rs` gains a module-doc section, **"A rolling
deploy can race a unique index build"**, stating what
`ledger/2026-09-15-a-new-index-returns-nothing.md` recorded as "not analysed,
not claimed": a backfill building a **unique** index is not safe against a
concurrent writer, because the backfill's uniqueness check and the write path's
are separate reads, so two rows colliding on the new key can each pass their own
and both be written. A non-unique index cannot disagree, and the note says why.

**Writing it narrowed it.** The caveat says "against writers" and the section
originally said "run the migration before the writer accepts writes" — which
turned out to be advice the daemon already takes. `reconcile` in
`crates/slate-serverd/src/main.rs` runs the plan *before* the node serves, and a
node with `migrate_on_start = false` refuses to start while anything is
outstanding, so a single node cannot race itself. What is left is a rolling
deploy: the new node backfills while the old leader is still writing, through a
binary whose catalog has no such index. That is a smaller hazard than the caveat
described and a sharper one, and it is only visible from the daemon, which is
why it took reading `reconcile` rather than reading the entry.

## Why

The tracker's argument is that an undecided caveat costs a reader's trust, and
the backlog had reached the point where the remaining twenty-six were almost all
one shape: a real alternative, weighed and not taken, recorded as an absence
because "we did not do X" is easier to write than "we chose not to do X,
because". A `deliberate` verdict is the second sentence, and writing twenty-six
of them is the work of finding the *because* for each — which twice meant going
and reading the code rather than the entry.

The migrate.rs note exists because that caveat is the one place where the
missing sentence was not a judgement but an operational hazard. "Not analysed"
is an honest thing for a ledger entry to say on the day it is written and a
dangerous thing for it to be the only record of, two weeks later, in a file
somebody is reading to decide whether to run a migration against a live head
node. The ledger is where reasoning lives; a hazard belongs beside the code
that has it.

## Alternatives rejected

**Leave them open and keep working the backlog.** This is the honest-looking
choice and it had stopped converging: every entry written to close a caveat adds
caveats of its own, so a session that closes three and writes one entry is often
flat. What actually moved the number was deciding at write time, which is what
these verdicts are.

**Fix the backfill rather than document it.** Two real designs: the backfill
holds something the other writer respects — a lease — or it builds and then
validates in a second pass over the finished index. A lease lives in the storage layer, so
giving `slate-kernel::migrate` one inverts the layering the rest of the crate
keeps; the validation pass is the right answer and is a substantially larger
change than this commit. Documenting a hazard is not a fix and is not offered as
one; it is what stops the next reader meeting it by surprise.

**Mark the twenty-six `moment` instead.** A `moment` verdict says the sentence
was true when written and describes that day rather than the tree — which is
right for "five caveats went back on the backlog" and wrong for all of these.
Every one of them is still true of the tree; what changed is that the reason for
leaving it true is now written down. Misusing `moment` to clear a list is how a
verdict vocabulary stops meaning anything.

**Close the threat-model caveat outright.** Item 5 of
`ledger/2026-09-27-five-gaps-decided-rather-than-deferred.md` rests on "a caller
who can time a query already holds a valid identity for that tenant", and that
premise *is* now a check — rule 5 of `scripts/check_handlers.py` holds every
method taking a `Request<pb::..>` to deriving a `SecurityContext`, which is the
rule written because of finding 9's unauthenticated `leadership`. But rule 5
establishes that an identity was derived, not that it is the right tenant's, and
the decision is still a judgement about timing as a channel that no guard
grades. `narrowed`, with both halves as the residual.

## Evidence

`python3 scripts/caveats.py` — *1023 caveats: 0 open, 32 narrowed, 242 closed,
671 deliberate, 0 untriaged*. Before this commit: 26 open.

`python3 scripts/check_caveat_citations.py` — *every path, test and function
name a caveat verdict cites is there*. That guard is what caught two invented
ledger citations earlier in this session, and it is the reason each verdict
naming a test or a file was written by looking the name up rather than recalling
it.

`sh scripts/check.sh` — 71 passed, all of them. `cargo doc -p slate-kernel
--no-deps` exits 0 on the new module docs (the five warnings it prints are
pre-existing intra-doc links in other modules).

**No mutation was run**, and that is not an omission being glossed. This commit
changes one Rust doc comment and a JSON file of verdicts. A doc comment has no
behaviour to break; the verdict file is read by `caveats.py`,
`check_caveat_citations.py` and `check_closed_caveats.py`, all three of which
are already mutation-tested against their own suites, and mutating *data* they
read would test those guards again rather than anything written here.

## What this does not do

**A `deliberate` verdict is a reading, and twenty-six of them were written in
one sitting.** The tracker's own caveat about this —
`ledger/2026-09-27-a-hundred-and-three-caveats-decided-and-one-that-was-already-false.md`
— applies here with more force, because a batch shares a frame of mind: the
twenty-sixth reason was written by somebody who had just written twenty-five,
and the failure mode is a reason that sounds like the previous one rather than
like the caveat it is about. The protection is the same one that batch chose —
each reason names its alternative concretely enough to be argued with — and it
is a protection against vagueness, not against being wrong.

**Two of the twenty-six were decided against code I re-read; twenty-four were
decided against the entry.** The two are the backfill hazard, where the module
docs had to be written, and the threat-model premise, where `check_handlers.py`'s
rule 5 had to be checked. For the rest I took the entry's own account of what it
did and did not do as accurate. Earlier in this session two entries' accounts
turned out to rest on stale premises, so that is a known rate rather than an
assumption: the reasons here are only as good as the entries under them.

**Zero open is a property of the verdict file, not of the tree.** Nothing here
built anything, fixed anything or measured anything. Twenty-five real pieces of
work are now recorded as chosen-against rather than undone, which is a more
useful thing for a reader and exactly as much software as before. The list that
would say what is left to build is the gap table in
`docs/orm-comparison.md`, not this one.

**The backfill hazard is documented and unenforced.** Nothing stops a rolling
deploy adding a unique index while the old leader still writes, and the note
says so in its last sentence. A guard would be the runner refusing to build a
unique index unless it can establish that no other writer holds the lease —
which means it needs a way to ask, which is the lease this change declined to
reach for.

**And the narrowing is itself a reading of one function.** `reconcile` runs the
plan before the node serves *as it is written today*; nothing asserts that it
keeps doing so, and a future startup that served during a backfill would make
the single-node case live again with the note saying it cannot happen. The
ordering is load-bearing and untested as an ordering.
