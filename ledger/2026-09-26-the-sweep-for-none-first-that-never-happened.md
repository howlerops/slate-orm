# The `None`-first sweep the CI-ruff entry said it had not done: one occurrence, in a place `RUF036` does not look, and a guard that is stricter than the lint because the lint is not available here

- **Date:** 2026-09-26
- **Author:** Claude, working from the standing instruction to address every caveat
- **Kind:** fix
- **Touches:** `scripts/{check_none_last.py,test_check_none_last.py,check.sh}`, `.github/workflows/ci.yml`, `clients/python/src/slate/values.py`

## What changed

`scripts/check_none_last.py` walks every `.py` in this repository and reports a
type union writing `None` anywhere but last. It is in `scripts/check.sh` (now
65 steps) and in CI, with 17 cases beside it.

The sweep found **one** occurrence, in `clients/python/src/slate/values.py`:

```python
-PyValue = None | Null | bool | int | float | str | bytes | _uuid.UUID | Vector | Array
+PyValue = Null | bool | int | float | str | bytes | _uuid.UUID | Vector | Array | None
```

## Why

`ledger/2026-09-20-cis-ruff-is-newer-than-mine-too.md` fixed two occurrences of
`RUF036` — *"`None` not at the end of the type union"* — after CI's `ruff`
went red on a file this container's called clean, and recorded what it had not
done:

> **Two occurrences, one rule.** I did not sweep the tree for other annotations
> that put `None` first; `ruff --fix` with a newer version would, and running
> one is the thing that is not possible here.

Six days later that is still exactly true, and it is the shape `CLAUDE.md`
warns about twice — for clippy and for ruff — under one heading: *a green local
lint is necessary and not sufficient, and there is no local command that finds
the next one.* An unswept tree plus a newer CI is a build that goes red on a
file nobody touched.

**The one occurrence is not a `RUF036` violation**, and that is the interesting
part. Run with preview enabled, `ruff --preview --select RUF036` passes on it,
because `PyValue = None | ...` is a type *alias assignment* and the rule reads
annotations. Nothing was wrong: `None | X` and `X | None` are the same type to
every checker. But the rule this repository adopted after two red builds is
"write `None` last", and a rule that holds in annotations and not in the alias
on the next line is one nobody can apply from memory — and a future `ruff` that
extends `RUF036` to aliases turns it red with no warning, which is the original
failure again.

## Alternatives rejected

**Enable `ruff`'s preview and select `RUF036`.** One line, and it is the
obvious move. `--select RUF036` here answers *"has no effect because preview is
not enabled"*, so getting the one rule means turning preview on repository-wide
— every other unstable rule with it, on a codebase that has to stay green
against two `ruff` versions at once. That is how a lint configuration becomes
the thing somebody switches off. Cost of the walk instead: 150 lines that need
no lint at all, so they cannot be newer or older than anything.

**Run `ruff --fix` with a newer `ruff`.** What the caveat itself proposed, and
it is not available: installing a second toolchain is the disk problem
`CLAUDE.md` documents, and `pip install --upgrade ruff` would change the
version `scripts/check.sh` runs for everything else. It would also have found
nothing, since the one occurrence is outside the rule.

**Fix the alias and write no guard.** Twenty seconds, and it leaves the caveat
exactly where it was: a sweep done once, by a person, with nothing to say
whether it is still true tomorrow. That is the *class* the caveat is in, not
just the instance.

**Match the lint exactly — annotations only.** Then the guard would pass on the
only thing in this tree it has to say anything about, which is a check that
demonstrably does nothing. Being stricter means this can go red on a file
`ruff` calls clean; the docstring says so outright, so nobody reads a failure
as a disagreement with `ruff`.

## Evidence

**The sweep, as a measurement.** 121 Python files read, one union with `None`
not last, in an alias. Confirmed against the lint from both directions:
`ruff check .` and `ruff check --preview --select RUF036 .` both pass on the
tree before *and* after the fix, at the root and in `clients/python`, which is
what establishes the guard is doing something `ruff` is not.

**The first draft reported the same union nine times.** `ast` parses
`A | B | C | D` as `(((A | B) | C) | D)`, so a walk over every `BinOp` sees one
union once per link and calls `A` "not last" of an inner node where it is fine
in the whole. `operands()` flattens first and `_nested()` skips the inner arms;
*a chain reports once, not once per link* is the case that holds it.

**Mutation run**,
`ledger/mutations/20260926T023230-scripts-check-none-last-py.json` — **eight
cases, eight caught**, each by a named test:

| mutation | the test that failed |
|---|---|
| the offence condition → `if False:` | *None first is reported* (and three more) |
| `parts[:-1]` → `parts` | *None last is correct* (and three more) |
| stop flattening the chain | *a chain reports once, not once per link* |
| drop the `_nested` skip | *a chain reports once, not once per link* |
| a `SyntaxError` returns a finding | *a file that will not parse is skipped rather than reported* |
| `SKIP` is ignored | *a vendored tree is not read*, *generated protobuf stubs are not read* |
| `read > 0` → `True` | *a tree with no Python at all is reported, not passed* |
| `is_none` drops the `is None` test | *a bitwise or between values is not a union* |

The last is the one worth naming: without it, `mask = 0b01 | 0b10` is a union
with a constant not last, and ordinary arithmetic becomes a style finding.

`sh scripts/check.sh`: **65 passed, all of them.** `ruff` and `ty` clean on
both new files.

## What this does not do

**It reads the `|` operator only.** `Optional[X]`, `Union[None, X]` and a
union inside a string annotation are all invisible. None occurs in this tree —
checked when this was written, which is an observation about today rather than
a property — and each would need its own arm.

**It does not close the gap the entry it came from is about.** CI's `ruff` is
still newer than this container's, and this covers one rule of the hundreds
that differ. The next `RUF` number to land stable will go red here the same
way, and the honest mitigation is unchanged: read the diff out of the job's
log.

**The one occurrence was not a bug.** `None | X` and `X | None` are the same
type, `ruff` passed on it, and no behaviour changed. What changed is that the
house rule now holds uniformly and something says so — which is worth less than
a bug fix and is what a sweep that comes back almost empty buys.

**The mutation record's filename was invented and a guard caught it.** The
first draft of the table above cited `…T023731…`; the record is `…T023230…`.
That is the **fifth** invented citation in two days —
`ledger/2026-09-26-the-citation-nobody-could-follow.md` counted four — and the
fourth to be caught by a guard rather than by a person. The class is not
carelessness about facts, it is a plausible filename recalled instead of looked
up, and reading it back does not catch it. What catches it is that every tree
where a filename can be written now has something that opens it.

**Nothing checks the guard against a newer `ruff`.** If `RUF036` ever becomes
stricter than this walk — a shape it catches and this misses — the two would
disagree silently and only CI would know. The direction that matters today is
the other one.
