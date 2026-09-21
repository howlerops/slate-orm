# The citation guard was green because I ran it before writing the paragraph it would have caught

- **Date:** 2026-09-21
- **Author:** Claude Code, on `claude/rust-orm-record-layer-gswxlu`
- **Kind:** fix
- **Touches:** `docs/persisting-the-schema.md`

## What changed

One paragraph in §5 quoted a test by its *old* name as history and named no
live one. `scripts/check_cited_tests.py` refuses that, and the paragraph now
names both: the test is
`adding_a_nullable_column_is_not_a_migration_but_it_is_a_new_layout` and used
to be `adding_a_nullable_column_is_not_a_migration`.

## Why

CI went red on `876f645` in the `scripts` job. Not a toolchain gap and not a
newer linter — the guard exists here, `scripts/check.sh` runs it, and it would
have caught this locally. **I ran `check.sh` and then edited the docs.** The
green I reported was green for the tree as it stood four edits earlier.

The guard itself is right and its message says exactly what to do: "if the dead
name is deliberate history — name the live one in the same paragraph, which is
what a reader needs anyway." The old name is worth keeping, because the whole
point of the paragraph is that the assertion was inverted.

## Alternatives rejected

**Drop the old name.** One word shorter and it loses the paragraph's argument:
"this test used to assert the opposite" is the evidence that the limitation was
recorded rather than discovered late, and a reader who cannot find the old name
in the history cannot check that.

**Add the old name to an exemption list.** The guard has no such list and
should not grow one for this: the fix it asks for is strictly better for the
reader than an exemption, which is the difference between a guard worth having
and one people route around.

## Evidence

`python3 scripts/check_cited_tests.py` failed with

```
docs/persisting-the-schema.md: `adding_a_nullable_column_is_not_a_migration`
names no test, and nothing beside it does either
```

and now reports `13 documents in docs/, every cited test resolves`. Full
`sh scripts/check.sh` re-run after the edit, this time in the right order.

## What this does not do

**It does not stop the next person doing the same thing.** The failure is
sequencing — run the checks, then keep editing — and no check can catch an edit
made after it ran. The only defence is the pre-push habit, and `check.sh` is
cheap enough (no build, no browser, no network) that re-running it after the
last edit costs nothing but remembering to.
