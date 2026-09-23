# Two redaction tests that a leak could have walked past, and a premise I got wrong

- **Date:** 2026-09-20
- **Author:** Claude, starting on the four areas the review says it did not clear
- **Touches:** `crates/slate-serverd/src/auth.rs`, `crates/slate-slatedb/src/s3.rs`
- **Kind:** security

## What changed

Both credential-redaction tests strengthened, and one new case. No production
code: both redactions are correct and were correct before.

- `debug_never_prints_a_secret` now also checks the **byte** rendering of the
  secret, the startup banner, and `Chosen` as a whole rather than the
  authenticator alone.
- `credentials_are_not_printed_by_debug` now sets a **session token**, which it
  did not, and counts the redactions so a `Debug` that dropped the credentials
  entirely cannot pass.
- `a_config_with_no_credentials_redacts_nothing` is new: the ordinary case,
  where credentials come from the environment and there is nothing to redact.

## Why

`docs/security-review.md` names four areas it did not examine, of which one is
*"the S3 backend and its credential handling"*. Two types hold a resolved
secret in memory — `slate_slatedb::s3::Credentials` and
`slate_serverd::auth::Bearer` — and each is wrapped by a hand-written `Debug`
that redacts. The daemon's config file holds only environment-variable *names*
and file paths, never a secret, so those two are the whole surface.

**I began by claiming neither redaction was tested. That was wrong and I am
withdrawing it.** I grepped the workspace for `redact` in tests, got nothing,
and concluded nothing tested them. Both were tested — by
`debug_never_prints_a_secret` and `credentials_are_not_printed_by_debug`, which
simply do not use that word. The duplicates I had written were deleted and what
they added was folded into the tests that already existed. This is the same
mistake as asserting finding 7's limits were untested earlier today: a grep for
one spelling is not a search for a property.

What the sweep did find is a real weakness in one of them, and it is only
visible through a mutation:

**`Bearer::secret` is a `Vec<u8>`.** A `Debug` that printed it renders
`[48, 49, 50, ..]`, and `!rendered.contains(GOOD)` passes while every byte of
the accepted bearer token sits in the log. The mutation that swaps `t.name` for
`t.secret` — the exact accident the hand-written impl exists to prevent — was
caught by the old test, but *by luck*: `GOOD` is hex digits whose byte values
do not spell it back. A secret chosen differently, or a field added holding a
`Vec<u8>`, and the test says nothing.

Two other gaps, smaller and both real:

- The S3 test never set a **session token**, so the `map(|_| "<redacted>")`
  arm was unexercised. An STS token is shorter-lived than a key pair and every
  bit as usable while it lasts.
- Neither test would have noticed a `Debug` that printed *nothing*, which is
  useless and indistinguishable from one that redacts.

## Alternatives rejected

**A `Secret` newtype wrapping every credential, with `Debug` on the wrapper.**
The structurally right answer: the redaction stops being per-struct and becomes
a property of the type, and a new field holding a secret gets it by
construction. Rejected for now because it is a change to two crates' internals
to protect two fields, and because `Bearer::secret` is compared with
`ConstantTimeEq` and `Credentials` is handed to `AmazonS3Builder` — both want
the raw bytes, so the wrapper would be unwrapped at every use and the only
thing it buys is the `Debug`. Worth revisiting if a third secret-holding type
appears; two is not a pattern.

**A script that finds secret-carrying structs and requires a redacting
`Debug`.** This is the shape of three guards written today, and it does not
work here: there is no syntactic signal for *holds a resolved secret*.
`secret_env` and `session_token_env` are variable **names**, `access_key_id` is
deliberately printed, and `MINIMUM_SECRET` is a number. A name-based criterion
would be a proxy, and two proxies for the converter rule this morning cost 47
and 59 false positives before the right criterion fit. The test is the guard
here: a derive replacing either hand-written impl fails it immediately.

**Assert the exact `Debug` output.** Catches everything, including the "prints
nothing" case, with no thought about what to look for. Rejected because it
fails on every unrelated field added to `S3Config` or `Chosen`, and a test that
cries wolf on ordinary changes is a test that gets its expected string pasted
over without being read.

**Leave the byte check out because `Vec<u8>` "obviously" does not print the
string.** That is the reasoning that made the old test pass for the wrong
reason. The point of the assertion is that the bytes are in the log, and they
are in the log whichever way `Debug` renders them.

## Evidence

Five mutations, all caught, and the two marked `*` are the ones the tests did
not catch before this change:

```
ok  the authenticator's Debug prints the secret bytes in place of the name
ok  the startup banner names the secret it accepts                        *
ok  the secret access key is printed instead of redacted
ok  the session token is printed instead of redacted                      *
ok  the Debug impl drops the credentials entirely                         *
ok  a config with no credentials invents an empty set                     *
```

The first is caught either way; it is listed because it is the mutation whose
*manner* of being caught was the finding — the old test caught it through the
string check, which works only for this particular secret.

**A seventh mutation was refused by the harness rather than scored**, and the
refusal was right: my anchor for the startup banner did not occur in the file
at all, so the suite would have run against unmutated code and reported a
survivor. `mutate.py` printed `the text to replace occurs 0 times ... do not
run the suite, because it would pass against unmutated code`. That is failure
mode 1 in its own docstring, met while using it.

**An eighth was invalid and I wrote it**: inserting a second struct to break
`Chosen`'s `Debug` derive stopped the crate compiling, and the harness reported
`NOTHING RAN` with the three `E0277`s rather than calling it a survivor.

`cargo test -p slate-slatedb --lib`: 6 passed. `cargo test -p slate-serverd
--bins`: 175 passed. `cargo clippy --workspace --all-targets`: zero
diagnostics. `cargo fmt --all -- --check`: clean. `scripts/check.sh`: 27/27.

## What this does not do

**It covers `Debug`, and a secret can leave by other doors.** Neither test says
anything about `Display`, about `serde`, about a secret reaching an error
message, or about one being written to a trace span's fields. I checked that
neither type implements `Display` or `Serialize` and that no error constructor
in either module formats a secret — but that is a reading, and a reading is
what this entry is about being wrong.

**Two types, asserted to be the whole surface by enumeration.** The daemon's
config carries names rather than secrets, which I verified by reading
`config.rs`'s `Token` and `S3` structs. A third secret-holding type added
tomorrow gets no test and nothing will say so. I argued above against building
a script for it; that argument is a judgement about cost, not a claim that the
gap is closed.

~~**Three of the four unexamined areas are still unexamined.** The review names
the lease and leadership protocol, the Python client, and the tuple codec on
adversarial input beyond `untrusted.rs`.~~ This entry covers the S3 backend's
credential handling and nothing else, and even there it covers the credentials
rather than the backend.

> **All four are worked, later the same day**, and the other three turned up
> more than this one did: the leadership RPC answering unauthenticated
> (finding 9), the Python client printing a bearer token (finding 10), and a
> type the tuple codec's fuzzer had never generated. The S3 row is the only
> one that found no defect.

**The `access_key_id` is printed deliberately and I did not re-derive that.**
AWS treats a key id as an identifier rather than a secret, the existing test
asserts it is present with the comment "key id is useful in logs", and I kept
that. It is a defensible position I inherited rather than one I checked against
a threat model.
