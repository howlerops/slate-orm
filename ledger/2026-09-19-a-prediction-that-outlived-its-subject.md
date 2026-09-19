# A prediction that outlived its subject

## What changed

The closing paragraph of `docs/orm-comparison.md`'s **What neither plan does**
section. It named generated migrations and client codegen as the two rows most
worth a plan next, and predicted that codegen "would turn `SchemaCheck`'s
run-time drift refusal into a compile-time one in three languages". Codegen has
since been built. The paragraph now says migrations alone, and withdraws the
prediction in place with what actually happened, which is neither half of it.

## Why

`ledger/2026-09-19-a-default-cannot-hold-now.md` fixed the paragraph two screens
above this one and closed by admitting it had not re-read this one: *"half of
codegen has since been built and that sentence has not been re-read against
it."* That was an accurate note-to-self and it sat there. This is the follow-up.

The stale tense is the small half. The prediction was **wrong about the
mechanism**, and wrong in the direction that would have caused harm if anyone
had acted on it:

- *Not compile-time.* `scripts/codegen.py --check` regenerates the declarations
  and diffs them against the committed files, in CI. No type system is involved
  in any of the three languages.
- *Not a replacement.* The run-time refusal exists for a deployed client older
  than the catalog the running server holds. Nothing generated at build time can
  rule that state out, so the check has to stay. Codegen added an earlier check
  for the case where client and server are built together; it did not move the
  later one.

A reader who took "turn X into Y" at face value would have read the surviving
run-time check as leftover scaffolding. The document's own stated purpose is
that "the argument is the thing to attack if you disagree", which makes a
disproven argument left standing worse than a stale date — the same reasoning
the earlier correction gave, applied to the paragraph it skipped.

## Alternatives rejected

**Change "are also out of both" to name only migrations and stop.** The minimal
staleness fix, and it deletes the wrong prediction rather than withdrawing it.
This repository's documents record what was predicted and what it turned out to
be — `docs/correctness.md` and `docs/performance.md` both carry withdrawals —
because a document that quietly deletes its misses reads as a document that
never missed. Cost of doing it properly: one block quote.

**Delete the whole paragraph.** Generated migrations *are* still out, the gap
table row for them is accurate, and "what is the next thing worth a plan" is
information a reader wants. Deleting it to avoid restating one clause loses
more than it saves.

**Fold the withdrawal into the codegen gap-table row at line 111.** That row is
already correct and already says what is built. Putting the withdrawal there
would separate it from the claim it withdraws, which is the failure mode the
earlier entry diagnosed: *"the update stopped there and did not follow the
subject to its second mention."* Doing it the other way round is the same
mistake mirrored.

**Go through the rest of the document for the same class of problem.** Tempting
and out of scope for a correction whose whole content is one paragraph. It is
named below as not done, rather than claimed.

## Evidence

**Codegen's drift check is a CI diff, not a compile.** `.github/workflows/ci.yml`
line 441 runs `python3 scripts/codegen.py --check` with the three generated
paths, in the job that has a built `slate-serverd`, with a comment saying it
runs *before* the conformance suite so a stale file gives one line instead of a
hundred schema-check failures.

**No generated file carries a fingerprint in a type.** `grep -i fingerprint`
over the three generated demo schema modules returns 3, 3 and 1 hits, and
reading them shows every one is inside a comment. The value is computed from the
declaration at run time by `fingerprint_of(table)` in
`clients/python/src/slate/schema.py:211` and its two counterparts, exactly as it
was before codegen existed.

**The run-time refusal is unchanged and still reachable.** Nothing in this
session's work touched the `SchemaCheck` path, and the conformance corpus still
carries live cases that trip it — a client declaring the first 3 of a table's 8
columns wrongly gets the full refusal text naming the prefix, which is how a
mis-built relation surfaced earlier today.

**Generated migrations really are still out**, so that half of the paragraph
needed no change: the row at line 110 says `slate-kernel/src/migrate.rs` plans
and applies a diff and nothing writes the target catalog for you, which is
current.

`python3 site/check/docs.py` passes — 36 checks, including that every relative
link resolves. `sh scripts/check.sh` — 19 of 19.

## What this does not do

- **No code changes.** This is a documentation correction and nothing in the
  behaviour it describes moved.
- **The rest of `docs/orm-comparison.md` was not re-read against the code.** I
  followed the one thread the previous entry left dangling. The document is
  1,200 lines of claims about what is and is not built, written across several
  sessions, and the two corrections it has needed today were both found by
  someone who happened to be editing nearby — which is not a process.
- **Nothing checks a document's claims against the repository.** Said in
  `2026-09-19-a-stops-at-that-stopped-being-true.md` about design notes and true
  again here. A checker would need the claims marked up, and this is the third
  instance rather than a class anyone has attacked.
- **No prediction elsewhere was re-examined.** `docs/roadmap.html` and the
  gap table both contain forward-looking statements of the same kind, and
  whether any of them has been overtaken is unknown.
