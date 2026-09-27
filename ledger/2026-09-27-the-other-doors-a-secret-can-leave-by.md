# A secret-holding type now has to say so, and may not derive `Debug` or reach `Display` or `Serialize`. The script an earlier entry argued against, written because the enumeration had to be redone by hand every time.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/check_secret_types.py`, `scripts/test_check_secret_types.py`, `scripts/check.sh`, `.github/workflows/ci.yml`, `docs/caveat-status.json`
- **Kind:** security

## What changed

`scripts/check_secret_types.py`: a struct with a field named for a secret must
be in `HOLDS_SECRET`, must not derive `Debug`, and must not implement or derive
`Display` or `Serialize`. Both directions, over every workspace crate, in
`check.sh` and CI.

## Why

`ledger/2026-09-20-the-secret-that-debug-prints-as-numbers.md` closed two
credential redactions and left two caveats that are the same gap from opposite
ends:

> **It covers `Debug`, and a secret can leave by other doors.** Neither test
> says anything about `Display`, about `serde`, about a secret reaching an
> error message, or about one being written to a trace span's fields.

> **Two types, asserted to be the whole surface by enumeration.** [...] A third
> secret-holding type added tomorrow gets no test and nothing will say so. I
> argued above against building a script for it; that argument is a judgement
> about cost, not a claim that the gap is closed.

That judgement was reasonable and is overturned here for a reason the entry
could not see: the enumeration has to be redone **by hand every time anyone
wonders**, and this session wondered. Three of the four doors are checkable
from the text; the cost of checking them is a hundred lines and it is paid
once.

## Alternatives rejected

**A `Secret<T>` newtype that cannot be printed.** The structural fix, and the
right one for a codebase starting today: make the type system carry it instead
of a roster. Rejected because it is a refactor of two types plus every
construction site, in a workspace where the two current impls are already
correct — it would buy the same guarantee for the same two types and a
migration for the third. A roster costs a line when the third arrives, and the
line is where a reader learns what keeps it quiet.

**Match `token` as a secret-shaped name.** It is a read token in the reader
pool, a lexer token in `slate-sql`, and a bearer secret in `auth.rs`, and only
the third is one. A roster keyed on a word with three meanings is a roster of
false positives, which is the state in which a guard stops being read.

**Check for a secret formatted into an error message.** The fourth door the
caveat names, and the one this does not close. It needs to know that
`format!("bad token {t}")` interpolates a secret-holding value, which is
type information a regular expression does not have. Recorded below rather
than approximated.

## Evidence

**The surface is two types, and the script agrees with the enumeration that was
done by hand.** `Credentials` in `slate-slatedb/src/s3.rs` and `Bearer` in
`slate-serverd/src/auth.rs`. Neither derives `Debug`; neither implements
`Display` or `Serialize`. `Credentials` has a hand-written `Debug` that redacts;
`Bearer` has none at all, and the hand-written one is on `TokenIdentity`, which
holds a `Vec<Bearer>` and prints only the names.

**The suffix rule is load-bearing and was measured.** `slate-serverd`'s config
carries `secret_env`, `secret_file` and `secret_access_key_env` — the name of
an environment variable, a path, and another name. Without the `_env`/`_file`
exclusion the daemon's whole configuration type joins the roster and the roster
stops meaning anything. A mutation deleting that exclusion is caught by both a
fixture and the real tree.

**Mutations.** Two runs, nine cases:
`ledger/mutations/20260927T002755-scripts-check-secret-types-py.json` (eight
cases, seven caught, one survivor) and
`ledger/mutations/20260927T002812-scripts-check-secret-types-py.json` (the
survivor re-run after its test was written: caught).

| mutation | outcome |
| --- | --- |
| the unrostered-type rule never reports | caught |
| the stale-roster rule never reports | caught |
| a derived door is not looked for | caught, 2 named cases |
| an implemented door is not looked for | caught |
| a field naming *where* a secret lives counts as one | caught, incl. the real tree |
| the never-fires half, deleted | caught |
| `Debug` is no longer one of the doors | caught |
| a derive carries across an unrelated item | **survived**, then caught |

The survivor is a parsing detail worth the note: a `struct` line consumes the
derives above it, so the case with an unrelated *struct* between a derive and
the rostered one never reached the reset arm. Only a non-struct item — an
`enum`, a type alias, a function — does, and that is the case that was missing.

**Suites.** `scripts/test_check_secret_types.py` 11 passed 0 failed.
`sh scripts/check.sh` 69 of 69 (two new steps). `scripts/test_check_sh.py`: 104
CI steps, all accounted for.

## What this does not do

**A secret formatted into an error or a trace field is still invisible.** The
fourth door, named in the caveat this closes and not closed here. What would
find it is knowing that the value interpolated into a `format!` is one of these
two types, which is type information; a regular expression looking for
`{secret}` would miss `{t}` and flag every unrelated field of that name. The
two constructors that could do it were read when the original entry was
written, and that reading is still all there is.

**It reads `crates/` only.** The three clients hold secrets too — a bearer
token in each — and none of them is Rust, so none is read. The Python client
printing a bearer token was finding 10 of the security review and was fixed;
nothing here would catch a second one. A per-language version of this guard is
three more parsers, and the clients' secrets are passed through rather than
stored, which is a weaker reason than it sounds and is why this is stated
rather than argued.

**A struct nested inside another is reported against the outer one.** The
owner of a field is the last `struct` opened above it, which is right for every
flat block in this workspace and wrong for a nested definition. The name a
reader is given is then the enclosing type, which is findable; the rule still
fires, it just points one level out.
