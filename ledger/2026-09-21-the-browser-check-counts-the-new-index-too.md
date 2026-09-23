# The browser check counted two indexed columns, and there are three now

- **Date:** 2026-09-21
- **Author:** Claude Opus 5
- **Touches:** `site/check/workbench.py`, `crates/slate-wasm/tests/sql.rs`
- **Kind:** fix

## What changed

Two counts in the workbench's browser check, and one more line wrapped the way
CI's rustfmt wants.

`site/check/workbench.py` asserted the schema tree marks **two** indexed
columns and that the storage view lists **six** leaves. Adding `by_title_text`
to the wasm fixture made those three and seven.

## Why

`1c91bca` turned two CI jobs red. Both were mine and both were avoidable.

**The browser check.** The wasm fixture gained a text index on `books.title`,
so the schema tree marks a third column and the keyspace holds a third index
prefix. I updated the equivalent Rust assertion —
`the_schema_comes_from_the_catalog` — because `cargo test -p slate-wasm`
failed and told me. I did not update this one, because I ran `site/check/docs.py`
and `site/check/quickstarts.py` and not `site/check/workbench.py`. `CLAUDE.md`
lists all three and says to run the ones your change touches; a change to the
fixture the workbench renders touches the workbench.

**The formatter.** An 82-character `vec![...]` of operator names, which
`rustfmt 1.8.0` here leaves alone and CI's `1.98.1` splits across three lines.
The second time today. Diff taken from the job log.

## Alternatives rejected

**Loosen the assertions to `>= 2` and `>= 6`.** That is how a count-based check
stops being a check. The counts are exact because an index appearing or
vanishing from the tree is the thing worth noticing, and the numbers now carry
a comment naming which three and which seven.

**Add the browser check to `scripts/check.sh`.** It needs a browser and a wasm
build, which is precisely the boundary that script draws and states — and
`scripts/test_check_sh.py` would then require it to be listed as one the script
*can* run, which would be false. The right fix is the one applied: run it.

## Evidence

`python3 site/check/workbench.py`: "the workbench runs the kernel in a browser",
every case ok. `cargo fmt --all -- --check`: clean. `sh scripts/check.sh`: 34
passed.

Running it locally first needed `sh site/build-wasm.sh` — the check refuses a
`site/slate_wasm_bg.wasm` older than its sources, and mine was eleven files
behind. That artifact is gitignored and CI builds it fresh, so the staleness
was local only; worth recording because the refusal reads like a failure of
the change under test and is not.

## What this does not do

Nothing stops the next fixture change from missing this file again. The three
site checks are separate scripts run by hand, and only CI runs all of them —
so "I ran the checks" and "I ran the checks this change touches" stay different
sentences. A single entry point that runs the browser checks too would need a
browser in `scripts/check.sh`, which that script exists not to require.
