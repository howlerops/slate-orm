# Every Authenticator, tested together, and a roster that must stay complete

- **Date:** 2026-09-20
- **Author:** Claude, closing the last of the three "one path of several" cases
- **Touches:** `crates/slate-serverd/src/auth.rs`, `scripts/check_handlers.py`, `scripts/test_check_handlers.py`
- **Kind:** security

## What changed

One test exercising all three `Authenticator` implementations against finding
6's hole, a `const AUTHENTICATORS` naming them, and a rule in
`check_handlers.py` that fails if an `impl Authenticator for` exists the list
does not name — or if the list names one that does not exist. No production
code: all three are already correct.

## Why

Three findings this session had fixes that covered one path of several. Two of
the three now have a check: `Catalog::insert` is fixed in code, and the handler
ordering is guarded. The third was left with an open caveat:

> **Nothing stops a fourth implementation repeating this.** The trait says
> nothing about duplicated keys, and a new `Authenticator` reaching for
> `metadata.get` would be as wrong as these two were, with no check to say so.

That is the one remaining instance of the pattern, and it is the one where the
gap is cheapest to close: the implementations are countable statically, and the
property they must share is one assertion.

So `no_authenticator_resolves_a_duplicated_identity_key` sends two values under
one key to each of them and requires a refusal. `DenyEveryone` is in the list
although it refuses everything and reads no key — because the list has to be
complete for the static rule to mean anything, and a trivially true case is
cheaper than an exemption somebody has to argue.

The static rule is the half that matters. A test over a hand-written list is
exactly the arrangement that failed for `query_from_proto_at`'s exemption
earlier today, where a list of callers was correct-looking and wrong. This one
cannot drift: the list is a Rust `const`, the check reads it from the source it
is scanning, and both directions are checked.

## Alternatives rejected

**A default method on the trait** that reads a key and refuses duplicates, so
an implementation gets the behaviour for free. The right shape if the trait
were about metadata keys, and it is not: `TokenIdentity` reads `authorization`
and strips a bearer prefix, `MetadataIdentity` reads three keys and parses
values, `DenyEveryone` reads nothing. A default that fits all three would
either be so general it asserts nothing or would force the other two into a
shape that does not suit them.

**Hard-code the roster's path in the checker.** What the first version did —
`ROOT / "crates" / "slate-serverd" / "src" / "auth.rs"` — and it was wrong for
the same reason the checker's own `SOURCES` was wrong an hour earlier: a path
that can go stale silently, in a check whose job is to notice staleness. The
list is now *found* among the files being scanned, which also lets the tests
run the rule over a tree they wrote rather than against the repository.

**Only check one direction.** Flagging an implementation missing from the list
is the obvious half. The other half — a name in the list that implements
nothing — matters as much and is easier to miss: a stale name means a test
looping over something gone, passing while covering one case fewer than its
name claims. A mutation removing that half survived until a test covered it.

**A `#[test]` per implementation instead of a loop.** Clearer failures, and it
loses the thing being built: the roster exists so that the *set* is checked,
and three independent tests have no set to compare against.

## Evidence

The rule fires. With `TokenIdentity` removed from the list:

```
`impl Authenticator for TokenIdentity` is not in AUTHENTICATORS in the
AUTHENTICATORS list.
  Add it, and give it a case in
  `no_authenticator_resolves_a_duplicated_identity_key` — finding 6 was a
  duplicated identity key resolved rather than refused, and it survived in
  the implementation nobody was testing.
```

Four new cases in `test_check_handlers.py`, over trees the test writes: an
implementation missing from the roster, a roster naming a departed one,
implementations with no roster at all, and a tree with no authenticators (which
needs no roster and must pass). 13 cases, all passing.

Five mutations, all caught:

```
ok  the roster rule is never called          -> three roster cases
ok  an unrostered implementation is not reported -> an authenticator missing from the roster fails
ok  a stale roster name is not reported      -> a roster naming an authenticator that no longer exists fails too
ok  a missing roster is reported as an empty one -> authenticators with no roster anywhere fails
ok  an empty tree is treated as a disagreement   -> a tree with no authenticators needs no roster
```

**A sixth mutation was invalid and the harness said so** rather than scoring
it: disabling the `rostered is None` branch leaves `rostered` as `None` and the
next line raises `TypeError`, so nothing ran. `NOTHING RAN — []` is the
harness distinguishing "the suite disagreed" from "the suite never reported",
which is the second of the three lies it was written for. The replacement
mutation — an absent roster read as an empty one — is a behaviour change and is
caught.

`cargo test -p slate-serverd --bins`: 175 passed. `cargo clippy -p slate-server
-p slate-serverd --all-targets`: zero diagnostics, after a first attempt where
a five-tuple drew `very_complex_type` — which `-D warnings` would have turned
red in CI, caught locally this time rather than by a failed job.

**I destroyed this work once and the check caught it.** A `git checkout` used
to undo a one-line `sed` reverted the whole uncommitted file. The next
`check_handlers.py` run said `3 impl Authenticator for and no AUTHENTICATORS
list anywhere`, which is precisely the case that had just been written. The
guard's first real finding was about its own author.

## What this does not do

**It checks the roster, not the cases.** `AUTHENTICATORS.len()` is asserted
against the number of cases in the test, so a name cannot be added without a
case — but a case could name the wrong authenticator, or pass the wrong key, and
both lists would still agree. What stops that is reading the test, which is
what stopped nothing this morning.

**One property, not a contract.** The test asks whether a duplicated key is
resolved. Finding 6 is that property; the trait has other obligations — what a
malformed value does, what an absent key does — and none of them are checked
across implementations here.

**The key each implementation reads is hand-written per case.** `PRINCIPAL_KEY`
is a literal because `slate-server` does not export the constant. If the header
were renamed, the case would send a key `MetadataIdentity` ignores and the
"more than once" assertion would fail — loudly, which is why the literal is
acceptable — but it is still a spelling maintained in two places.

**`impl Authenticator for` is matched at the start of a line.** An
implementation written inside a module with indentation, or with a `where`
clause on the next line, would be missed. Every current one is top-level; the
never-fires check catches the case where *all* of them stop matching and not
the case where one does.

**Nothing does this for the third pattern instance.** `Catalog::from_tables`
and `insert` are both correct and there is no roster of catalog constructors. A
third constructor would be the same story, and the shape of a general answer —
"what else constructs this type" — is still not something I have.
