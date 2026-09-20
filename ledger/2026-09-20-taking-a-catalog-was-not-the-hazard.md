# The guard's blind spot closed, after two attempts that measured the wrong thing

- **Date:** 2026-09-20
- **Author:** Claude, closing the caveat `check_handlers.py` was committed with
- **Touches:** `scripts/check_handlers.py`, `scripts/test_check_handlers.py`
- **Kind:** security

## What changed

A third rule in `check_handlers.py`: every call to a converter that resolves a
*request's* tables must have an authorisation above it. The converter names are
derived from their signatures rather than listed. No production code — all four
call sites are already correct, and were made so earlier today after this
guard's own exemption vouched for them wrongly.

The module docstring listed two rules when the file implemented four; it now
lists four.

## Why

The entry for this guard named its own largest gap:

> `join_from_proto` calls neither `self.table(..)` nor `fingerprint::check`
> directly. Catching it would mean following calls across functions, which is a
> parser and a call graph.

That gap is not hypothetical: it is exactly where finding 8 was open on `join`,
`explain_join`, `aggregate` and `explain_aggregate` this morning, and the guard
watched all four convert before authorising without a word. The two rules it
had cover resolutions that happen *in* a handler. These happened a function
away.

A call graph is not needed. The converters can be recognised by their
signatures in one pass, and their calls checked in a second — which is enough
for the shape that actually occurred.

## Alternatives rejected

**List the two converter names.** Two lines and correct today. Rejected for the
reason every hard-coded thing in this script has been rejected: a third
converter arrives and the list does not, and a guard that silently stops
covering a case is worse than one that never covered it, because it is cited.

**Follow calls properly — a parser and a call graph.** What the caveat said it
would take, and it would catch a converter reached two hops down. `syn` in a
lint script, or an ad-hoc Rust parser, for a codebase with two converters and
four call sites. The adjacency window is the same approximation rules 1 and 2
already make and it is honest about being one.

**Require the authorisation inside the converter instead.** Move the check to
where the resolution is, and no call-site rule is needed. This is the shape the
code cannot take: the converters are free functions with no `SecurityContext`
in scope, and threading one through them means changing both signatures and all
six of `query_from_proto_at`'s callers to carry a context they already used
before calling. The check belongs where the context is.

## Evidence

**Two attempts measured the wrong thing, and the numbers are the finding.**

The first matched a bare `catalog: &Catalog,` line and attributed it to the
nearest `fn` above, which picked up struct fields: it derived `run`, `quantile`
and `summary` as converters and reported **47** problems. That was a parsing
bug and I called it one.

Matching the whole signature instead gave **59** — worse — and I recorded at
the time that the regex was "deriving bogus names". **That was wrong and I am
withdrawing it.** Nothing was mis-parsed. Checked one function at a time:
`reconcile`, `render_plan`, `describe`, `security::catalog`, `seed::load`,
`analyze` and `store_for` all genuinely take a `&Catalog`. The rule was
correct about its own criterion and the criterion was wrong. Taking a catalog
is not the hazard — those are CLI and startup code, where there is no request,
no caller and no grant to check, and demanding an `authorized_table` above each
call would be demanding nonsense.

The hazard is narrower and can be stated exactly: **resolving a table whose
name came off the wire, with nothing to check it against.** Both halves must
show in the signature — a `&pb::` request *and* a `&Catalog` — and a
`SecurityContext` parameter disqualifies it, because such a converter can check
for itself. On the real tree that derives exactly two names:

```
CONVERTER convert.rs fn join_from_proto            wire=True  ctx=False
CONVERTER convert.rs fn aggregate_from_proto_query wire=True  ctx=False
skip      main.rs    fn reconcile                  wire=False ctx=False
skip      main.rs    fn render_plan                wire=False ctx=False
skip      main.rs    fn describe                   wire=False ctx=False
skip      security.rs fn catalog                   wire=False ctx=False
skip      seed.rs    fn load                       wire=False ctx=False
skip      seed.rs    fn analyze                    wire=False ctx=False
skip      seed.rs    fn store_for                  wire=False ctx=False
```

Six new cases in `test_check_handlers.py`, 19 in total, all passing. Five of
the six are cases that must **not** fire — a guard people switch off is worth
less than no guard, and 59 false positives is what switching it off looks
like. One of them, "a function taking a catalog but no request is not a
converter", exists only to pin that regression.

Six mutations through `scripts/mutate.py`, all caught:

```
ok  the converter rule is never called                      -> a handler converting before authorising fails
ok  a catalog alone makes a converter, without a wire request -> a function taking a catalog but no request is not a converter
ok  a converter holding a context is treated as one anyway  -> a converter holding a context can check for itself
ok  a converter delegating to a converter is not skipped    -> four cases
ok  the multi-table helpers do not count as authorisation   -> a handler that authorises its inputs first passes
ok  a tree with no converter is a pass                      -> a tree with no converter at all fails
```

The fourth is the weakest: dropping the delegation skip fails four cases at
once rather than the one written for it, because it also flags each converter's
own definition line. It demonstrates the line matters, not that the skip is
precisely right.

**A never-fires guard, because this rule needs one more than the others.**
`fingerprint::check` is one literal string; a converter is recognised by three
conditions on a signature, so renaming the generated proto module out of `pb`
would leave the rule matching nothing and printing `ok`. Zero converters is now
a failure. `scripts/check.sh`: 25/25.

`FINGERPRINT_BY_CALLER`'s reason claimed all six of `query_from_proto_at`'s
callers authorise first. Four of those six are now held to it by rule 3 rather
than by my reading, and the entry says which half is which.

## What this does not do

**It sees adjacency, not coverage.** A handler that authorises table `a` and
then converts a request naming `b` passes rule 3, exactly as it passes rule 2.
What stops that is `security_probe.rs`, where
`every_input_of_a_join_is_authorised_not_only_the_first` asserts the tables by
name — and that test exists because an earlier version of it was vacuous.

**The criterion is a signature, and signatures are not semantics.** A converter
that took its tables as `&str` and a `&Catalog`, or reached a catalog through
`&self`, would resolve request-named tables and match nothing here. The two
that exist take a `&pb::` type; a third written differently is invisible until
somebody widens `WIRE`. I did not try to enumerate the shapes it would miss.

**A `SecurityContext` parameter is treated as sufficient.** A converter holding
one is exempted on the grounds that it *can* authorise, not that it does. That
is the weakest link in the rule and there is no such converter today to test
the assumption against.

**Rule 3 runs over `slate-server` and `slate-serverd` only.** A converter in
another crate reached from a handler is outside `SOURCES`, which is the same
boundary the other rules have and the same one that was wrong once already,
when `SOURCES` was a single file.

**The 47 and the 59 were not re-measured after the fix.** They are what the two
wrong criteria reported at the time, quoted from the runs. I did not restore
either version to reproduce them.
