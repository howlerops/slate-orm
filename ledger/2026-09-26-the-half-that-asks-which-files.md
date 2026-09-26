# Eight rules thorough about the files they are given, and nothing asking whether those are the right files. `check_handlers.py` now asks.

- **Date:** 2026-09-26
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/check_handlers.py`, `scripts/test_check_handlers.py`, `docs/caveat-status.json`
- **Kind:** security

## What changed

A ninth rule in `check_handlers.py`: no workspace crate outside `SOURCES`
carries what the other eight guard. Members are read from `Cargo.toml`, each
member's `src/` that is not already scanned is walked, and a file containing
`fingerprint::check`, `impl Authenticator for` or a `Request<pb::` method is
reported with the `SOURCES` line to add. `main` takes a `root`, so the rule can
be run over a workspace a test wrote.

## Why

Two caveats, one week, the same hole:

> **Rule 3 runs over `slate-server` and `slate-serverd` only.** A converter in
> another crate reached from a handler is outside `SOURCES`, which is the same
> boundary the other rules have and the same one that was wrong once already,
> when `SOURCES` was a single file.

> **Two crates, named explicitly.** A third server crate is outside this until
> somebody adds it, and the never-fires check will not notice — it fires on
> "nothing found anywhere", not on "a tree nobody listed". That is a weaker
> guarantee than it sounds and I have not closed it.

That second sentence is the precise shape. Every never-fires half in the file
asks whether the *listed* tree still contains what its rule is about; none can
ask whether an unlisted tree does. `SOURCES` was one file once, and a
`fingerprint::check` in `convert.rs` — a file away, reachable only through
callers — was invisible to it. The fix then was to name a second directory,
which is the same fix one size up and has the same ceiling.

## Alternatives rejected

**Scan every crate and roster the exemptions.** The thorough answer: walk all
thirteen members and exempt the ones that legitimately carry a marker. It
inverts the maintenance — a crate that *should* be checked is checked by
default — and it was rejected because the exemption list would immediately be
larger than `SOURCES` and would not distinguish "checked here" from "checked by
something else". `SOURCES` names where the rules apply; this rule names where
they should.

**Match a gRPC service impl rather than markers.** `impl pb::…Server for` is
the narrowest possible definition of "a crate that serves the wire" — and it
misses exactly the case the first caveat names, a *converter* in another crate
reached from a handler, which implements no service. The markers are the
subjects of the rules themselves, so the roster cannot drift from what is
guarded without a rule losing its subject.

**Glob `crates/*` instead of reading the manifest.** A directory under
`crates/` that is not a workspace member is compiled by nothing, and demanding
coverage of it is demanding coverage of dead code.
`scripts/check_examples_roster.py` reads the manifest for the same reason.

**Leave `main`'s `root` defaulting to the repository.** One less parameter, and
it makes the rule untestable in the way this repository has now been bitten by
three times: the rule would read *this* workspace while every other rule read a
fixture. It did, on the first run — see below.

## Evidence

**Three reports on the real tree, all three false, all from one marker.**
`self.table(` is rule 1's subject and the obvious fourth entry. With it, rule 9
reported:

| crate | why it is not a finding |
| --- | --- |
| `slate-sql/src/sql.rs` | `self.table()`, no argument — a parser method on a builder |
| `slate-schema/src/catalog.rs` | the `Catalog::table` the rule is *about*: the primitive the authorised one is built from |
| `slate-wasm/src/lib.rs` | the browser binding, which has no tenant to cross |

`self.table` names a method on whatever `self` happens to be, and rule 1 is
about a `self` that is a gRPC service. The three markers that stayed name the
same thing everywhere they appear. With `self.table(` dropped, the tree is
clean: 13 workspace crates, none outside `SOURCES` carrying any of the three.

**The untestable-parameter failure, met again in the same hour it was written
about.** The first run of the suite failed nine cases, every one reporting
`crates/slate-server/src/auth.rs` — a file in *this* tree — against a `SOURCES`
the fixture had replaced. `root` had a real default and the fixtures could not
override it. That is the third time
(`scripts/check_cost_prose.py`'s `main` records `docs` and `readmes`), and the
lesson each time is the same: whatever a check reads, its tests must be able to
say "read this instead".

**Mutations.** Two runs, nine cases:
`ledger/mutations/20260926T234729-scripts-check-handlers-py.json` (eight cases,
six caught, two survivors) and
`ledger/mutations/20260926T234809-scripts-check-handlers-py.json` (the survivor
that was a missing test, re-run: caught).

| mutation | outcome |
| --- | --- |
| rule 9 reports nothing | caught, 4 named cases |
| each of the three markers, deleted | caught, one case each |
| the no-members never-fires half, deleted | caught |
| `members()` reads `member` rather than `members` | caught, 4 named cases |
| a crate already covered is reported anyway | **survived**, then caught |
| a crate with no `src/` is scanned anyway | **survived** |

The first survivor was a missing test: no fixture's source directory was also a
listed member, so the skip-what-is-already-covered branch was never reached.
`run` grew an `inside_crate` argument and a case that puts the service inside a
listed crate; re-run, caught.

The second was redundant code. `rglob` on a path that is not there yields
nothing and raises nothing, so a member with no `src/` was already skipped by
finding no files. The `is_dir()` guard was deleted and the finding written
where it was.

**Suites.** `scripts/test_check_handlers.py` 43 passed 0 failed (was 35).
`sh scripts/check.sh` 67 of 67.

## What this does not do

**It is a roster of spellings, like every other rule here.** A converter that
fingerprinted through a re-export, or a service built by a macro, carries none
of the three markers and is invisible. That is the same limit rule 3's own
caveat records about signatures — *"a converter that took its tables as `&str`
and a `&Catalog` would resolve request-named tables and match nothing here"* —
and it is why the markers are the rules' own subjects rather than a guess at
what a hazard looks like: when a rule's subject is respelled, its never-fires
half fires and this roster is edited in the same change.

**`self.table(` is out, and something real is out with it.** A fourth crate
that grew a gRPC service and resolved tables with a bare `self.table(name)`
would be caught by `Request<pb::` — but one that only *converted*, reached from
a handler, and resolved that way would not. The alternative was three false
positives on every run, which is the state in which a guard stops being read.
Narrowing it properly means matching the receiver's type, which needs a Rust
parser and not a substring.

**The other eight rules still read two directories.** Rule 9 says the list is
complete; it does not widen the list. A crate that legitimately joins
`SOURCES` is still a deliberate edit — which is the point, and is what makes
the edit happen at the moment the crate appears rather than at the next audit.
