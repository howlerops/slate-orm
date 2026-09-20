# The lease parser, attacked: no defect, two tests that were missing

- **Date:** 2026-09-20
- **Author:** Claude, on the caveat that struck the lease row for the RPC only
- **Touches:** `crates/slate-server/src/lease.rs`, `docs/security-review.md`
- **Kind:** security

## What changed

`lease::untrusted`: eight tests putting the lease decoder against bytes it did
not write. No production change — the decoder is total and correct, and was
before.

## Why

The leadership entry struck the review's "lease and leadership protocol" row
for the RPC surface only, and said so:

> The lease *protocol* itself is still not attacked.

The protocol is a large question. Its **parser** is not, and it is the part an
adversary reaches. The lease is an object in the same bucket as the data —
`lease.rs` describes it as "deliberately plain text, three lines and a header",
so an operator can `cat` it — which means anything that can corrupt a block can
corrupt it, and anyone with bucket write can author it outright.

What makes that worth a suite rather than a read is where `decode` sits:
`current()` is called on every renewal and every campaign attempt, by every
node. A panic in it is not one failed request; it is a crash loop across the
cluster, on a timer, from one bad object. That is the same argument
`slate-tuple/tests/untrusted.rs` opens with, and the same weak contract is the
right one: **for any input, `decode` returns `Ok` or `Err`.**

## Alternatives rejected

**Attack the protocol instead** — two nodes racing, clock skew, a rolled-back
lease object. The bigger and more interesting question, and it needs a model of
what an attacker with bucket write can do to *SlateDB*, not just to the lease;
`lease.rs` is explicit that safety comes from the fence underneath, so an
attack on the lease alone would be attacking the component that already says it
promises no mutual exclusion. Worth doing, not doable well in the time this
had, and left recorded rather than half-done.

**Sign or checksum the lease object.** Would detect a forged one — and against
an attacker who can write the bucket it buys nothing, because they can write
the data too, and the key would have to live somewhere that is not the bucket,
which is the lock service this design deliberately does not have. Against
*corruption* the magic line and the field parse already refuse, which is what
the tests below establish.

**Put the tests in `tests/lease.rs` beside the existing ones.** `decode` is
private, and making it `pub(crate)` to test it would widen the surface for the
convenience of the test. A `#[cfg(test)] mod` in the file is where it belongs,
and it matches `untrusted.rs`'s shape.

**Bound the object's size before parsing.** `decode` reads whatever
`object_store` hands back, and the `Malformed` error formats the whole text
into its message. A huge object would make a huge error string. Left alone:
the read itself has already allocated the object, so the error is a second copy
of something already in memory rather than a new exposure, and a size limit
here would be a guess at a number with no measurement behind it.

## Evidence

**Eight tests, and the decoder passed all of them from the start.** Hostile
text biased towards what means something to this parser — the magic line, the
three field names, a bare colon, `u64::MAX`, `-1` — plus arbitrary bytes, plus
every truncation of a valid lease, 4,000 cases each.

**One thing I expected to find and did not**, recorded because a withdrawn
hypothesis is worth more than a silent one: `expires_ms` is a `u64` parsed
straight into `Duration::from_millis` and added to `UNIX_EPOCH`, and
`SystemTime + Duration` **panics** on overflow rather than saturating. That is
the classic shape. It does not fire: `u64::MAX` milliseconds is about 5.8×10⁸
years, and a Linux `SystemTime` holds an `i64` of seconds, so it fits with
eleven orders of magnitude to spare. `an_expiry_at_the_top_of_the_field_does_not_panic`
asserts it rather than leaving it to that arithmetic being redone correctly by
the next reader — and a mutation reading the field as seconds scaled by a
billion does overflow, and is caught.

Six mutations, all caught, and **two of the tests exist only because a mutation
survived without them**:

```
ok  an unparsable expiry panics instead of being refused        -> two property tests
ok  the expiry is read as seconds, which overflows              -> three tests
ok  a duplicated field takes the first value rather than the last -> a_duplicated_field_takes_the_last_value
ok  a non-UTF-8 lease is decoded lossily instead of refused     -> a_lease_that_is_not_utf8_is_refused_rather_than_repaired  *
ok  the magic line is not checked                               -> an_object_that_is_not_a_lease_is_refused_on_the_first_line  *
ok  the refusal does not say what it expected                   -> the same
```

The two marked `*` are the findings. Neither is a defect in the decoder; both
are properties the decoder has and nothing asserted:

- **Lossy UTF-8 decoding is not a cosmetic difference.** It substitutes U+FFFD
  for every invalid sequence, so a corrupted object whose bytes happen to land
  around the field names would parse as a *valid* term rather than as
  malformed, and a node would act on a generation and a holder nobody wrote.
- **The magic line is the only thing that says "this is not a lease".** Without
  it, any object at that key reaches the field loop, where a stray
  `generation:` anywhere in the bytes is enough to be read as one — a block, a
  manifest, a file somebody put there by hand.

`cargo test -p slate-server --lib`: 22 passed. `cargo clippy --workspace
--all-targets`: zero diagnostics, after the new module needed the same
`clippy::expect_used` allowances the integration suites carry — caught locally
rather than by a red job. `scripts/check.sh`: 29/29.

## What this does not do

**The protocol is still not attacked**, and that is now the whole of the
review's lease row that remains. Fencing, generation monotonicity and what two
nodes that both believe they lead can do to each other are covered by 1,390
lines of correctness testing and by an unusually explicit argument in
`lease.rs`'s module docs — an argument I read and did not test.

**Nothing asserts the generation never repeats.** `lease.rs` promises "each
acquisition raises a generation number that never repeats", which is the
property the whole ordering rests on. It is presumably covered by
`tests/lease.rs`; I did not check which test, and this suite does not.

**It tests `decode`, not `current()`.** The path from a bucket object to a
`Term` also includes the `object_store` read, the not-found case and the
backend-error case. Those are covered by the existing integration suite against
an in-memory store; what is new here is only the parsing.

**The `holder` field is unbounded attacker-controlled text** when an attacker
can write the bucket, and it reaches `LeadershipStatus.holder` on the wire.
That is now behind authentication (finding 9), and it is a string going to a
client that asked for it, so I judged it acceptable rather than bounding it.
A judgement, not a measurement of what a client does with a megabyte of
`holder`.
