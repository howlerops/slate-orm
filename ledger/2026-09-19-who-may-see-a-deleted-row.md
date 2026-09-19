# Who may see a deleted row

## What changed

`include_deleted` crosses the wire. `Query.include_deleted` is field 13 on the
protocol, the server honours it, and lifting the soft-delete filter now
requires a new grant — `Action::ReadDeleted`, spelled `read_deleted` in a
daemon's security config.

## Why

The soft-delete entry left this undone with a specific reason, written into
`convert.rs`:

> Not on the wire, so always false here: a remote caller cannot ask to see
> soft-deleted rows. … "show me the deleted ones" is a privileged read and the
> protocol has no way to say who may make it.

The observation was right and the conclusion did not follow. The protocol still
has no way to say who may make the read — and it does not need one, because the
*catalog* already answers questions of that shape. `Action::Explain` is the
precedent, and it is the same shape exactly: a capability that is not a weaker
form of `Read`, excluded from the `all` shorthand so no existing blanket grant
silently acquires it, granted per role and per table by the deployment.

Once the grant exists the protocol field is unremarkable. The decision was
never the protocol's to make; it was the catalog's, and the catalog was already
in the business of making it.

## Alternatives rejected

**Imply it from `Read`.** Then every existing reader could see retired rows the
day this shipped, which is a silent widening of access — the precise failure
`Action::ALL` excludes `Explain` to avoid.

**A server-wide flag, like the one that reopens `EXPLAIN`.** A global switch
answers "may anyone see retired rows here", and the real question is per role
and per table: a recycle bin the user empties themselves and a retention
archive only compliance may open are both soft delete, in the same deployment.
The grant is per `(role, table)` and a global knob cannot express that.

**Authorize in the service layer rather than the kernel.** Less code, and it
would leave the kernel API open to a library caller — this repository already
spent a task ("close the RLS matrix") on the principle that every access path
enforces policy, and a second enforcement point in the daemon would be a second
thing to keep in step. The check sits in `plan`, the one function that honours
the flag, so every read, join side and chain step goes through it.

**Demand the grant unconditionally.** `include_deleted` on a table with no
soft-delete column reveals nothing, and demanding a privilege for a no-op
teaches callers to ask for privileges they do not need — and a grant asked for
often enough gets given. It is checked only when the table soft-deletes.

## Evidence

Six mutations, all caught:

| mutation | caught by |
| --- | --- |
| the grant is never checked | `a_plain_reader_cannot_ask_to_see_retired_rows` |
| the grant is demanded without `include_deleted` | `a_plain_reader_still_reads_live_rows` |
| the grant is demanded on a table with no soft delete | `include_deleted_needs_no_grant_on_a_table_that_does_not_soft_delete` |
| `read_deleted` folded into the `all` shorthand | `a_plain_reader_cannot_ask_to_see_retired_rows` |
| the wire drops the flag inbound | the round-trip proptest |
| the wire drops the flag outbound | the same |

The wire proptest is the satisfying one. `wire.rs` pinned `include_deleted:
false` with a comment reading *"it is a kernel-side flag with no field on the
wire, so `true` is a value the round trip cannot preserve. When it does cross,
this becomes generated and this comment goes."* It crosses; it is generated
over `any::<bool>()`; the comment is gone. 600 cases.

`cargo clippy --workspace --all-targets`: clean. 85 test suites across the
three changed crates, no failures. The generated stubs were regenerated from
the canonical proto and verified the way CI does: `test_generated.py`
(byte-identical), `gofmt`/`go build`/`go vet` on the Go client, and a full
build of the TypeScript client.

## What this does not do

**No client exposes it.** Python, Go and TypeScript can all *send* the field —
their stubs carry it — but none has a builder method, so a caller would have to
construct the protobuf by hand. Three small additions, not made here, and the
reason for stopping is that a query-builder method in three languages is its
own task with its own conformance cases rather than a footnote to the wire
change.

**`purge_deleted` now needs two grants.** It authorizes `Delete` and then scans
with `include_deleted`, so a non-superuser purging needs `delete` *and*
`read_deleted`. That is coherent — you should be able to see retired rows to
destroy them — but it is a change to the contract shipped an hour ago and
nothing tests the two-grant path; the purge tests all run as superuser, which
bypasses both.

**Nothing in the demo or the conformance corpus uses it.** The three-SDK suite
does not exercise a retired row across the wire at all, so the only evidence
that the field survives a real round trip is the proptest over the conversion
functions, not a live server.

No `read_deleted` grant appears in any example config, so the first person to
want one will find the keyword in `security.rs` or in this entry.
