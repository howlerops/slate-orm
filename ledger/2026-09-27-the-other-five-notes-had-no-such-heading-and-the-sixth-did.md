# Six design notes checked for an answered question under an open heading; one had one, and its answer was a refactor that had already shipped

- **Date:** 2026-09-27
- **Author:** Claude Code, closing the residual of
  `ledger/2026-09-27-a-heading-that-said-open-over-two-answers.md`
- **Touches:** `docs/views.md`
- **Kind:** docs

## What changed

`docs/views.md`'s **Open, and deliberately not decided here** becomes **Left
open here; one answered, one still open**, and the two questions under it are
labelled:

- *Where a view is declared* — already marked "answered" inline, and its
  closing paragraph was written in the future tense about a refactor that has
  since happened. Struck through, with what the tree now holds.
- *What `EXPLAIN` shows* — labelled as the one question in the section that is
  still open. It is the same thing
  `ledger/2026-09-20-a-view-cannot-be-a-privilege-boundary.md` carries as an
  open caveat, so this is a pointer rather than a new claim.

The stale paragraph said extracting a `slate-sql` crate "is a refactor of some
size" and described the arrangement it would produce. `crates/slate-sql`
exists — `sql.rs`, `lower.rs`, `lib.rs` — `slate-serverd` and `slate-wasm` both
take it as a workspace dependency, and `slate-wasm/src/lib.rs` is down to five
mentions of `wasm_bindgen`, which is the number that paragraph predicted as the
end state.

## Why

The residual asked whether the other design notes have the same defect as
`docs/arrays.md`: content updated, frame not. The check is cheap and exact —
grep every note for a section heading that claims something is open — and the
answer is a clean one:

| note | an "open/undecided/deferred" heading |
|---|---|
| `views.md` | **yes** — and one of its two questions was answered |
| `ctes.md` | no |
| `full-text.md` | no |
| `validation.md` | no |
| `paging-a-join.md` | no |
| `persisting-the-schema.md` | no |
| `undo-window.md`, `topology.md`, `clickbench.md` | no |

Nine notes, one heading, one stale question under it. That is the residual
answered rather than estimated, which is the point of doing the grep rather
than saying "the others probably have it too".

## Alternatives rejected

**Widen the grep to every heading, not just open-flavoured ones.** A heading
can go stale without using the word "open" — "What this note does not do" over
something it now does, for instance. Rejected as a different and much larger
question: the residual is specifically about the shape found in `arrays.md`,
and answering a wider question here would leave the narrow one answered by
implication rather than by reading.

**Build the guard.** The sibling caveat already argues why not, and this
result is the evidence for that argument rather than against it: one occurrence
across nine files is not a pattern worth a parser, and the thing a parser would
need to know — what the heading claims about the paragraph — is the part it
cannot see.

**Leave `views.md` for whoever closes the `EXPLAIN` caveat.** They would find
the heading and probably fix it, and until then every other reader of that file
meets the same trap. The two questions have nothing to do with each other.

## Evidence

- `grep -n "^## .*\([Oo]pen\|[Nn]ot decided\|[Uu]ndecided\|[Nn]ot done\|
  [Ll]ater\|[Dd]eferred\)"` over all nine notes in `docs/` — one hit, in
  `views.md`.
- `crates/slate-sql/src/` holds `lib.rs`, `lower.rs`, `sql.rs`;
  `slate-sql.workspace = true` appears in `crates/slate-serverd/Cargo.toml`
  and `crates/slate-wasm/Cargo.toml`;
  `grep -c wasm_bindgen crates/slate-wasm/src/lib.rs` answers 5, the number the
  stale paragraph predicted.
- `python3 scripts/check_cited_docs.py` — 666 citations, all openable.
  `sh scripts/check.sh` — 71 of 71.
- No mutation run: prose only.

## What this does not do

**It checks one heading shape.** The grep looks for six words in a `##` line.
A note whose stale frame is phrased some other way — "Still to decide", "Next"
— is not found, and neither is a stale sentence anywhere that is not a
heading. What is claimed is exactly what was run.

**`EXPLAIN` over an expanded view is still unexamined.** Labelling it as the
section's one open question is not answering it; the plan mentions the base
table, `Action::Explain` is excluded from `Action::ALL`, and nobody has looked
at what an expanded plan prints. That caveat stays open where it already is.

**The future-tense paragraph is kept, struck through.** Same call as every
other correction in this file's neighbourhood: the argument that got the
refactor built is worth more than the fact that it is built, and deleting it
would leave `crates/slate-sql` looking like it had always been there.
