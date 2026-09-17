# A token that never reached a caller

- **Date:** 2026-09-17
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `clients/python/src/slate/{_details.py (new),errors.py}`, `clients/python/tests/test_details.py` (new), `clients/go/slate/{error.go,details_test.go (new)}`, `clients/go/go.mod`, `clients/go/internal/pb/google/` (deleted), `clients/typescript/src/{details.ts (new),errors.ts}`, `clients/typescript/{package.json,test/details.test.ts (new)}`, the three explorer adapters, `clients/python/PROTOCOL-FINDINGS.md`, `docs/orm-comparison.md`, `site/docs/roadmap.html`
- **Kind:** fix

## What changed

`SlateError.reason` / `Error.Reason` / `error.reason` is populated on **every**
failure the head node reports, not only a batched one. Each client decodes the
`google.rpc.ErrorInfo` the server has been putting in `grpc-status-details-bin`
all along. Three decoders, three unit suites over one captured blob, and the
conformance runner now compares the token on every refusal case it already had.

Two things found on the way and dealt with: `clients/go/internal/pb/google/`
was dead code that panics the process if imported, and is deleted; Python's
`NotLeader.__init__` silently did not accept the new keyword.

## Why

The previous entry shipped `max_returned_rows`, whose refusal is only useful if
a caller can tell it apart from the other four things that arrive as
`RESOURCE_EXHAUSTED` — a join build, a group count, a distinct count, a sort.
Those five want five different fixes. The server already distinguishes them:
`status.rs` packs a stable token per kernel variant into every status.

Measured before building anything, against a node with the cap set low:

```text
type    ResourceLimit
code    StatusCode.RESOURCE_EXHAUSTED
reason  ''
message resource_exhausted: a predicate write matched more than 3 rows, ...
```

and, decoding the same call's trailers by hand:

```text
  ON THE WIRE -> type.googleapis.com/google.rpc.ErrorInfo
  reason      -> PREDICATE_WRITE_TOO_LARGE
  domain      -> slate-orm
```

The token was on the wire and every client threw it away. So the only way to
act on the distinction was to match on the message text, which the clients'
own documentation forbids in the same file that dropped the token.

This was a *known* gap — all three clients carried a comment admitting it,
ending "nobody has needed it enough to do that yet". Something needed it.

## Alternatives rejected

**Generate `google/rpc/*.proto` into the Python package**, as `records.proto`
already is. This is the obvious implementation and it is a trap: it breaks
`import slate` outright for anyone who also has `googleapis-common-protos`,
which is most people — `grpcio-status`, every `google-cloud-*` and the OTLP
exporter all depend on it. Generated modules register in protobuf's *default*
descriptor pool under the proto's own path, and two non-identical descriptors
for that path are a hard error:

```text
TypeError: Couldn't build proto file into descriptor pool:
duplicate file name google/rpc/error_details.proto
```

Identical copies are tolerated, which is what makes it quiet — but this
repository's `error_details.proto` declares `ErrorInfo` alone where upstream
declares ten messages, so the collision is certain rather than possible. Both
halves were demonstrated before the design was chosen: identical copies
coexisting, and differing ones failing at import.

**Depend on `googleapis-common-protos` (or `grpcio-status`) in Python.** The
canonical route, and it cannot collide, because it is the package that owns
those symbols. Rejected for what it costs: the client has exactly two runtime
dependencies today, and this adds two more so that one string can be read.

**Hand-roll a varint reader in Python.** No dependency and no descriptor pool,
about thirty lines. Rejected because a client library that parses protobuf by
hand is a maintenance liability out of proportion to the feature, and because
there was a third option that uses the real parser.

**What was built instead:** the three messages are declared in a
`DescriptorPool` of `_details.py`'s own, under a file name nothing else can
claim. Protobuf is structural — field numbers and wire types are the contract,
names are not — so these decode the server's bytes exactly as generated
`google.rpc` classes would, with no global registration and no new dependency.

**Use Go's own generated `internal/pb/google/rpc` instead of `genproto`.** The
appealing option: the package was already committed, so no dependency moves.
It is unusable. grpc-go links `genproto/googleapis/rpc/status`, which registers
`google/rpc/status.proto`, and the internal copy registers it again — importing
it panics at init, inside `file_google_rpc_status_proto_init`. Demonstrated
with a throwaway test before choosing. Nothing imported it, nothing generated
it (the generator emits `slate/v1` only), and it would crash the process if
anyone reached for it, so it is deleted rather than left beside the thing a
future reader would reach for first. `genproto` was already an indirect
dependency; this promotes it to a direct one and adds no module.

**Surface `ErrorInfo.metadata` as well as `reason`.** It is populated — `index`
and `table` on a unique violation, `limit` on this one. Rejected for now: the
keys differ per variant, so exposing the map means promising something about a
shape that changes from error to error, in three languages. The token alone is
what lets a caller branch below a status code and is what three clients can
agree on. Recorded in `PROTOCOL-FINDINGS.md` as available rather than built.

**Move `NOT_LEADER` onto the token.** It is a token now, so the `slate-leader`
trailer looks redundant. Left exactly as it was: the trailer predates the
details blob, a client that reads the trailer but not the blob still follows
the redirect, and churning a working discriminator to make a diagram tidier is
how redirects break.

**Add a conformance case for the cap itself.** It would need the demo's head
node capped at 3 — which would make a *demo* refuse any predicate delete over
three rows — or ten thousand seeded rows per run. The plan asked for the token
compared on *a lone failure*, and the suite already had five. Adding `reason`
to the adapters' error shape turned all five into three-way token comparisons
without inventing a sixth case or distorting the demo.

## Evidence

The reproduction above, and the decoded blob. `site/check/docs.py` passes.
223 Python tests, 117 TypeScript, the Go suite, the pre-commit hook's own
suite, `check_workspace.py`, `gofmt -l` clean. **80 conformance cases: the
three SDKs agree on all of them.** The demo's browser e2e: 18 passed, 0 failed.

The TypeScript lockfile was out of sync after `protobufjs` was declared, and
`npm ci --dry-run` reported "up to date" anyway — the root `dependencies` block
is what real `npm ci` compares, and it still listed two packages. Caught by
reading that block rather than by trusting the dry run, then proved by deleting
`node_modules` and running `npm ci` for real.

`mypy` is clean on the two files this change touches. It is **not** clean on
`client.py`, which reports five errors that predate this change — verified by
stashing. It is also not run anywhere in CI, which is why nobody has seen them.
Not fixed here: it is a separate change and pretending otherwise would bury it.

**Ten mutations, two survivors, both now killed.**

| mutation | result |
| --- | --- |
| Python: the `type_url` check is dropped | **survived** → killed |
| Python: the domain is returned instead of the reason | killed |
| Python: `reason` is declared as field 2 | killed |
| Python: `Any.value` is declared as field 3 | killed |
| Python: the decode returns nothing | killed |
| Go: `fromRPC` stops asking for a token | **survived** → killed |
| Go: the `*errdetails.ErrorInfo` assertion accepts any type | killed |
| TypeScript: `fromServiceError` stops asking for a token | killed |
| TypeScript: the namespace import of a CommonJS module | killed |
| the node adapter stops reporting `reason` | killed (conformance) |

**The dropped type check** passed every test, because the fixture meant to
catch it — a detail packed under another type URL — was malformed and decoded
to nothing whichever branch ran. The replacement is encoded rather than
decoded, in the opposite direction from the code under test, and is shaped
exactly like an `ErrorInfo` so that skipping the check reads one message's
field 1 as another's. It comes with a **negative control**: the same bytes
under the real URL must come back as the token, which is what says the first
test passes for the right reason. Both exist in all three suites.

**`fromRPC` dropping the token** survived every decoder test in the Go suite,
because they all call `reasonOf` directly. The conformance runner caught it,
which is a slow and indirect way to learn that one client stopped filling one
field, so all three clients now have a wiring test through their own error
constructor. That mutation is precisely the state this change fixed, and the
tests written first would have let it back in.

**The TypeScript namespace import was not a mutation.** It was a real bug,
written here and caught by the conformance runner rather than by the build:
under NodeNext, `import * as protobuf` from a CommonJS module yields the module
namespace rather than `module.exports`, so `protobuf.Root` was `undefined`. It
type-checked, and the decoder swallows its own failures by design, so the only
symptom was this client reporting `""` while the other two reported a real
token — a three-SDK disagreement, exactly what that suite is for. Kept in the
table because reverting the fix is a mutation worth having caught.

**`NotLeader.__init__`** was the other real bug: the one subclass with its own
constructor, which silently did not accept the keyword added to the base. It
surfaced as a `TypeError` on a redirect, from tests that already existed.

## What this does not do

**No client branches on the token yet.** The field is populated and tested; the
hierarchy still stops where it stopped. Splitting `Unavailable` four ways needs
the metadata map, which is not surfaced.

**The Python decoder's field numbers are pinned by tests, not by a compiler.**
Declaring the descriptors by hand is what avoids the collision, and the cost is
that `proto/google/rpc/` and `_details.py` must be kept in step by a person.
They are seven fields that have not changed since 2015, and a drift shows up as
the captured blob failing to decode.

**One captured blob, one shape of error.** It is a `RESOURCE_EXHAUSTED` with a
single detail. A status carrying two details, or none, is covered only by
hand-built cases.

**Nothing measures the cost of decoding.** It happens once per failure, on a
path that is already doing a round trip, and the protobuf root is loaded lazily
in TypeScript for that reason — but "already doing a round trip" is an argument
rather than a measurement.
