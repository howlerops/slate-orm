# The demo declares a view, three SDKs read through it — and doing that found a client that could not.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #257 (F5c)
- **Touches:** `examples/explorer/{head.toml,CONTRACT.md}`, the three adapters, `examples/explorer/conformance/conformance.py`, `examples/explorer/web/{src,test,e2e}`, `crates/slate-server/src/{fingerprint.rs,convert.rs}`, `scripts/{check_handlers.py,test_check_handlers.py}`
- **Kind:** feature, and one fix that only a running stack could have found

## What changed

`examples/explorer/head.toml` declares `classics = SELECT * FROM books WHERE
year < 1980`. All three adapters accept it on `/api/query` and report it under
a new `views` key on `/api/meta`, beside `tables` and not among them. The
conformance runner has four cases and two `MUST_DIFFER` pairs; the demo's table
picker offers it; the browser e2e has a case; `CONTRACT.md` says what a view is
on this contract.

And a server fix the demo produced: `fingerprint::check_named` verifies a
client's schema claim under **the name the request used**, so a claim about
`classics` is checked against `books`'s columns hashed under `classics`.
`query_from_proto_at` passes `query.table`, which is the same string as
`table.name()` on every path but a view.

## Why

The step-2 entry recorded that nothing outside `crates/slate-serverd/tests/views.rs`
had ever started a server with a view in it. That is the gap this closes, and
closing it was worth more than the tests it adds, because it found a defect no
amount of reading had.

`books` rather than a table with no policy, and `year < 1980` rather than
something disjoint from `modern_only`, because the only interesting thing a
view can demonstrate here is **that the answer depends on who asks**. Eight
rows as `app`, six as `reader`: 1955 and 1951 are inside the view and outside
the policy. That is `docs/views.md` §1 made visible — the view is substituted
away before planning, the base table's `TableId` reaches `row_filter_with`, and
a view carrying its own id would have had no policy and handed a reader all
eight.

## Alternatives rejected

**A separate `/api/view` endpoint.** Rejected because the server has no such
distinction: `query` resolves a table *or* a view and every other handler
resolves a table, so a `table` field that accepts either is the contract the
server already has. A second endpoint would have invented a difference the
protocol does not make, and the conformance runner would have compared three
adapters' implementations of a demo idea rather than of the protocol.

**A view in `tables` on `/api/meta`.** One list is simpler and wrong. Two of
the three clients hold a catalog, and the generator emits a row type per table;
a view listed among them would be described as something with an id, an index
and a write path. Separate lists cost one key.

**A column list of its own for the view, in each adapter.** The adapters build
it from the base table's — `Table("classics", BOOKS.columns, BOOKS.primary_key)`
in Python, `TABLES["books"]` in the web app, nothing at all in the Node adapter
because its client sends no claim for an undeclared name. That is not brevity:
a view may not narrow columns, so a view's ordinals *are* its base table's, and
a second list would be a thing that can disagree. The frontend test asserts
identity rather than equality, so a copy fails it.

**Teaching `scripts/codegen.py` to emit view declarations.** The right long-term
home — `--print-schema` already publishes `views` — and not needed here, which
is why it is not in this commit. Because each adapter derives the view from the
generated base table, there is no duplicated column list for the generator to
de-duplicate; what it would save is one line per adapter naming the view. Worth
doing when a second view exists, or when a client outside this repository wants
one.

**Skipping the fingerprint check for a view.** The first idea, and it would have
dropped the ordinal protection at exactly the point it is still needed: a view's
ordinals are the base table's, so a stale client declaration misreads a view's
rows precisely as it would misread the table's. Checking under the caller's name
keeps the whole of it.

**Hashing the name differently, or leaving it out of the fingerprint.** The
name is in the hash because a client can be wrong about which table it is
talking to. Removing it to make views work would trade a real check for a
feature.

## Evidence

**The finding.** The first conformance run reported three disagreements: Go and
Node read through the view and Python was refused with *"the schema check on
table `books` does not match this catalog"*. The Python client sends a schema
claim on a read, `fingerprint_of` hashes the table's name, and a client that
declared `classics` could never match `books` however right its columns were.
So: **a schema-checking client could not read through a view at all**, and
step 2 shipped without knowing it. Nothing in the Rust tests could have caught
it — `crates/slate-serverd/tests/views.rs` sends no claim, because the harness
builds requests by hand. It took a real client.

It also arrived disguised. Two of three adapters agreed, so the report read as
"the Python adapter is wrong", which is the first thing three-way agreement
makes you think. The conformance runner's value here was that it ran the same
request through a client that checks and two that do not.

`examples/explorer/run.sh --conformance`: **130 cases, the three SDKs agree on
all of them.** Four are new:

- a read through the view as `app` — eight rows;
- the same request as `reader` — six, because `modern_only` runs on the base
  table;
- a caller filter of `year >= 1970` composed with the view's `year < 1980`,
  which is the decade between rather than the whole of either;
- `/api/explain` naming the view, refused by the *server* (all three adapters
  share their query builder with explain, so all three send the name), which
  makes it the cross-SDK check of `Head::no_such_table`'s wording as well as
  of its kind.

Two `MUST_DIFFER` pairs, and they are the point rather than decoration: three
clients agreeing about eight rows proves nothing about the row policy, because
a view with its own `TableId` returns *the same eight* to a reader and all
three clients agree about it perfectly. The evidence that the base table's
policy ran is that a different caller gets a different answer, and that needs
two cases and a comparison. The second pair does the same for the caller's
filter being composed rather than dropped.

`examples/explorer/run.sh --e2e`: 24 passed, one new — the view narrows the
rows, the policy narrows them again, and the plan panel names the view. It
asserts the decade as well as the counts, because two arbitrary numbers
shrinking proves less than the right rows disappearing, and it has a control in
each direction (no pre-1960 book in the view would make the reader assertion
vacuous; a post-1980 book would mean the view's own predicate did nothing).

`crates/slate-server`: `a_view_is_checked_under_the_name_the_caller_used` is
the unit test for the fix, with two assertions because either alone is
satisfied by a function that ignores the name, and a third that a misspelled
column is still caught under a view's name. `--lib --test schema_check --test
wire --test server --test predicate_wire --test rls_join`: 22 + 12 + 18 + 25 +
30 + 15 passing.

`python3 scripts/codegen.py --check` against the demo config: all three
generated declarations still match, so a `[[views]]` block changes nothing the
generator emits. `pytest examples/explorer/backends/python/adapter`: 30 passed.
`gofmt`, `go vet`, `tsc --noEmit` on both TypeScript trees: clean.
`sh scripts/check.sh`: 34 passed.

**The handler guard caught the rename, by a thread.** `check_handlers.py` rule
2 matches `fingerprint::check\(`, so renaming the call in
`query_from_proto_at` made the pattern match nothing there and silently dropped
rule 2's cover from the converter every read goes through. The only symptom was
a *roster entry reported as stale* — an accurate report of a different problem.
The pattern now matches `check_named` too, and
`test_check_handlers.py` has a case for it, because a rule that can be switched
off by a rename is one rename from being off.

Frontend mutations, three, all caught by
`every view the UI offers reads a table, with that table's columns`: the view's
column list replaced by a copy of the same literal, the view pointed at
`authors`, and a view the UI offers that `head.toml` does not declare.

`scripts/mutate.py` could not score those: the demo frontend's suite is `node
--test`, whose output is TAP, and the script reads libtest, this repository's
own guards and pytest. It refused the run rather than guessing, which is
correct. They were run through a scratch script applying the same four
protections — anchor must occur exactly once, restore in a `finally`, zero
cases reported is a hard error, spec as JSON with no shell — and the working
tree was never left mutated. Teaching `mutate.py` the TAP dialect is the better
answer and is not in this commit.

## What this does not do

**Nothing mutates the conformance cases or the e2e case.** Both are expensive
to run under a mutation loop — each starts a head node, three adapters and in
one case a browser — and neither was mutation-tested. The `MUST_DIFFER` pairs
are the structural substitute on the conformance side: they fail if the two
answers stop differing, which is the failure a dropped predicate produces. The
e2e case has only its own internal controls.

**The Go and Node adapters read through the view unchecked.** Neither sends a
schema claim for `classics` — the Go client has no catalog and the Node one
returns no claim for an undeclared name — so the fix above is exercised by the
Python adapter and by one unit test, and by nothing else. Declaring the view in
the Node adapter's `Schemas` would close that and is one line; it is not here
because the Node adapter's declaration is generated and this would be the one
hand-written entry beside it, which is the drift the whole file avoids. The
generator change above is the right fix and the reason this waits for it.

**No client library gained a way to say "this is a view".** Each adapter builds
the declaration itself. A `Table.as_view(name)` in the three SDKs is the
obvious shape and would be a public API addition in three languages with its
own docs, codegen and tests; it is a task, not a line.

**Still one view, still one base table, still no view over a view.** Unchanged
from step 2, and the conformance corpus inherits every limit the feature has.

**Nothing measures a view's cost.** Unchanged from step 2 and now with a live
stack that could measure it: `slate-headbench` has no view case, and the demo's
eleven books would not produce a number worth reporting anyway.
