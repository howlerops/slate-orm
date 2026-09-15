# The named-zone change left the browser check asserting the old refusal, and one `format!` unformatted

- **Date:** 2026-09-15
- **Author:** Claude, following CI on 576899b
- **Touches:** `site/check/workbench.py`, `crates/slate-wasm/src/lib.rs`
- **Kind:** fix

## What changed

`site/check/workbench.py` asserted that `hour(pickup_time, 'America/New_York')`
comes back as a refusal containing "timezone database". It is now answered, so
the check waited 30 seconds for a `.refusal` element that never appeared and
timed out. Replaced with four checks: the named zone is answered in the browser
and gives 24 groups, those groups match the fixed `-05:00` ones exactly
(January is standard time, so the two must agree), the spec carries `"zone":
"America/New_York"` rather than a constant offset, and a *misspelled* zone —
`america/new_york`, which is the realistic mistake, since IANA names are
case-sensitive — is refused with the list of zones that do exist.

And `cargo fmt --all -- --check` failed on one `format!` in `zone_suffix`:
CI's rustfmt wraps it, the local one does not.

## Why

Both are the same mistake in two places: a behaviour changed and the things
asserting the old behaviour were not all found. The browser check is the one
that matters, because it is the only test that runs the transition table
through a real wasm build in a real browser — everything else exercises the
kernel natively. Left red, it would have been the check nobody trusted.

The `fmt` failure is the gap `CLAUDE.md` already warns about, from the
other side: CI's toolchain is newer than this container's, so a green local
`cargo fmt -p slate-wasm` is necessary and not sufficient. This one is only a
line wrap rather than a lint that does not exist locally, but it is the same
asymmetry.

## Alternatives rejected

**Delete the region check rather than replace it.** One line, and it would
have removed the only browser-level coverage of the feature at the moment the
feature started working. The check existed to pin behaviour at that boundary;
the behaviour changed, so the check changes with it.

**Assert only that the named zone is answered.** Cheaper, and it would pass
against a lowering that quietly turned the name into a constant −5 — which is
exactly the bug a table lookup can regress into. The comparison against the
fixed offset plus the spec assertion are what make the two distinguishable:
the values agree, and the mechanism is visibly different.

**Keep `zone_suffix` as one line and add a `#[rustfmt::skip]`.** Suppressing
a formatter to win an argument with it, on a line nobody cares about.

## Evidence

`python3 site/check/workbench.py` locally, against a fresh `sh
site/build-wasm.sh`: all checks pass, including the four new ones —

```
ok    a named zone is answered, in the browser, from the transition table
ok    and agrees with the fixed offset it was in: January is standard time
ok    the zone reaches the spec as a name, not as a constant offset
ok    a zone the table does not have is refused, and the refusal lists them
```

The failure being fixed, from the CI log: `locator.innerText: Timeout 30000ms
exceeded. Call log: - waiting for locator('[data-app="grid"] .refusal')`.

`cargo fmt -p slate-wasm` reproduces and fixes the formatting diff CI reported,
which is the whole of that half.

## What this does not do

**Nothing checks that a changed refusal has no stale assertion anywhere.** Both
of these were found by CI rather than by a guard. A grep for the old message
text would have found the browser check; a guard that knew which checks assert
which messages would be a second copy of both.

**The local toolchain is still older than CI's.** Installing a second one is
not possible here for the disk reason `CLAUDE.md` gives, so the rule remains
"run the workspace-wide command and treat green as necessary rather than
sufficient".
