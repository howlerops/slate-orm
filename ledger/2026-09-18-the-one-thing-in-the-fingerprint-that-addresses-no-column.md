# A decimal's scale goes into the schema fingerprint, breaking that hash's own rule on purpose, because the failure it prevents is worse than the failure the rule is about

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `crates/slate-server/src/fingerprint.rs` and `tests/review_fingerprint.rs`; the same hash in all three clients and their suites; `clients/python/tests/fixture.py`; `README.md`, `docs/orm-comparison.md`, `site/docs/features.html`
- **Kind:** fix

## What changed

`fingerprint::accepted` hashes `column.scale()` after the type, for a decimal
column and only for one. The three clients' ports do the same. Two constants
are pinned in all four suites.

A table with no decimal column hashes exactly as it did, so no client of one
needs rebuilding — the existing `0x97c3c1256af4cfdb` for `DOCS`, written down
independently in the Go and TypeScript suites, is unchanged and still green.

## Why

This was the last open item on the decimal feature and three documents called
it the sharpest edge:

> Nothing checks a client's declared scale against the server's; the
> fingerprint deliberately does not hash it, because a scale addresses no
> column, so a client with it wrong reaches the right column and renders every
> value off by a power of ten, for ever, with no error anywhere.

Every clause of that is true and the conclusion was the wrong way round. The
fingerprint's rule — hash only what a client must restate in order to
*address* a column — is a rule about which failures the hash is for, and a
scale does fail it. But the rule exists to keep the hash from breaking a fleet
on an unrelated schema change, and it is worth asking, per property, whether
that danger is real. For a scale it is not: **changing a scale is already a
refused migration**, so there is no unrelated change that could invalidate the
hash. The cost the rule guards against cannot be paid here.

Meanwhile the failure it was leaving open is the worst kind this project
recognises. A wrong ordinal reads the wrong column and usually reads nonsense.
A wrong scale reads the *right* column, and every value is off by a power of
ten, consistently, for ever — the wire carries a count of units and never the
scale, so no layer downstream has anything to compare against. The fingerprint
is the only place in the system where the two statements meet.

## Alternatives rejected

**Put the scale on the wire beside each value.** Then nothing needs checking,
because nothing is restated. Rejected for the reason the whole type exists: the
scale on the column is what makes a decimal encode as an integer, so the
ordering is the integer ordering and `SUM` is integer addition. A scale on the
value is a second place for it to live and a second place for it to disagree.

**Publish the schema and have clients fetch it.** Removes the class. Rejected
as the protocol decision it would reverse — `fingerprint.rs`'s own module doc
explains why the claim travels *to* the server rather than the schema
travelling out, and this change is that design working rather than an argument
against it.

**Hash the scale on every column as a zero for non-decimals.** Simpler: no
conditional, one shape. Rejected because it changes every table's fingerprint,
including every table with no decimal in it — which would make every client in
the fleet mismatch on a change that means nothing to them. The conditional is
what keeps the blast radius to clients that actually use a decimal, and those
are exactly the clients this is for.

**Bump the canonical form's version prefix to `slate.v1.schema/2`.** The honest
signal that the hash changed. Rejected for the same reason: it would change
every fingerprint, and the version prefix earns its keep precisely by *not*
being bumped for a change that only affects the tables it applies to.

**Leave it, and document the hole better.** It had been documented well for two
commits and was still a hole. A documented silent wrong answer is a silent
wrong answer.

## Evidence

**Five mutations across four independent ports, all killed:**

| mutation | outcome |
| --- | --- |
| the server stops hashing the scale | killed |
| Python stops hashing the scale | killed |
| Go stops hashing the scale | killed |
| TypeScript stops hashing the scale | killed |
| TypeScript defaults an absent scale to 2 rather than 0 | killed |

The last one is the one worth having: `scale` is optional in the TypeScript
`ColumnDef`, so `?? 0` is a real choice and a wrong default would make a
scale-2 declaration and a scale-omitted one hash alike — which is exactly the
confusion the whole change is about.

**Two constants, four ports.** `0xdab8856481bc4a6d` for `prices(id, label,
amount decimal(2))` and `0xdaba08fbb666133f` for the same table at scale 4,
written down independently in `review_fingerprint.rs`, `test_fixture.py`,
`schema_test.go` and `schema.test.ts`. Four implementations agreeing on a
constant is the evidence; one agreeing with itself is not, which is the rule
the existing `DOCS` constant already follows here.

**End to end, on one client.** The Python client's fixture declares `prices`'
`amount` at scale 2 and its testserver declares the same, so all 282 of its
tests now pass only because the two agree — change either 2 and the file is
refused. That comment in `fixture.py` used to say the opposite and is
corrected.

**A claim checked before it was made.** I was about to write that the
three-SDK conformance run proves all three clients agree with the server on a
scale, because the explorer's `books.price` is a decimal at scale 2 and the
corpus passes. It does not: the Go and TypeScript adapters hold no catalog and
send no `SchemaCheck` at all — `query.go` says so in a comment. Only the Python
adapter carries a declaration. So conformance exercises this for one of three,
and the cross-port agreement rests on the pinned constants instead.

`cargo test -p slate-server --no-fail-fast`, the Go, TypeScript and Python
client suites, `./run.sh --conformance` (92), `site/check/docs.py`: green.

## What this does not do

- **Two of the three clients still send no schema check at all.** The Go and
  TypeScript adapters in the explorer hold no catalog; the *libraries* support
  `SchemaCheck` and their own suites exercise it, but the demo does not. So the
  conformance corpus cannot catch a Go or TypeScript client with a wrong scale,
  and nothing says it should — the corpus compares answers, and a refused
  request is an answer only if the request was sent.
- **A request without a schema check is served as before.** The check is
  optional on the wire and this does not change that. A client that sends none
  can still have every scale wrong.
- **Nothing checks the scale of a *computed* decimal**, because there is no
  column to declare one against. The kernel refuses a computed decimal whose
  scale is not one a column already states, which is a different guarantee
  reached a different way.
- **The three SDKs still write their scale by hand**, read off the schema by
  eye. This catches the mistake rather than preventing it, which is the most a
  protocol that publishes no schema can do.
