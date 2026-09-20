# Security review

> **Status, later the same session.** Findings 1, 2, 3, 5, 6, 7 and 8 are fixed
> and their probes now assert the refusal. 4 is closed as inherent, with the
> claim it contradicted narrowed to the truth and one correction to the finding
> itself: `upsert` leaks the same bit as `insert`, which this document
> originally said it did not. Each finding carries
> its own status line below. The fixes are in the commits that reference this
> file.

An adversarial end-to-end review of the authentication, RBAC, row-level
security, tenant isolation, wire protocol and configuration surfaces, done as
one pass rather than feature by feature. Findings are ranked by impact, each
with a test that exhibits it. The areas probed and found clean are listed at
the end, because a review that only lists what it found tells the next person
nothing about where not to look again.

Everything below is reachable by an **ordinary authenticated caller** — a
principal with a tenant and an `app`-shaped role — unless it says otherwise.
Nothing here needs a superuser, and nothing here found a way to *obtain* one.

Demonstrations:

| file | what it holds |
| --- | --- |
| `crates/slate-kernel/tests/security_probe_cascade.rs` | findings 1, 2, 4, 5 |
| `crates/slate-server/tests/security_probe.rs` | finding 2 over gRPC, finding 6 |
| `crates/slate-kernel/tests/security_probe_explain.rs` | finding 3 |
| `crates/slate-kernel/tests/security_probe_resources.rs` | finding 7 |
| `crates/slate-kernel/tests/rls_probe.rs` | the paths probed and found clean |

The probe tests originally asserted the *current* behaviour — the hole — and
each named what to change it to once the finding was fixed. The fixes landed,
and inverting a probe generally renamed it. **The names below are the ones that
exist now**, with the original given where it differs, because a review whose
demonstrations cannot be located demonstrates nothing.

---

## 1. A `CASCADE` from a shared parent deletes other tenants' rows

**Status: FIXED**, and then fixed again in the place the first fix missed.
`Catalog::from_tables` refuses a referential action from a non-tenant-scoped
parent to a tenant-scoped child, naming both tables. Refused rather than
confined at the scan — confining stops the destruction and leaves other
tenants' children pointing at a parent that is gone, trading a security hole
for a correctness one. Turning the refusal on broke nothing but the pins.

`Catalog::insert` refuses it too, and for a while did not. `from_tables`
inserts every table and then calls `validate_foreign_keys`, so the refusal
lived only in that second call — which is public, opt-in, and documented with
"call it yourself after building a catalog with `Catalog::insert`". A caller
who built a catalog the other way got this finding back in full: measured,
tenant A deleting the shared org left `docs` **empty**, tenant B's row
included. Only the daemon's own path was ever safe, because
`slate-serverd/src/schema.rs` goes through `from_tables`; the exposure was to
anyone using `slate-schema` as a library.
`a_catalog_assembled_by_insert_is_refused_in_either_order` pins it, in both
insertion orders, because the check skips an edge whose parent is not in the
catalog yet and only one of the two orders would exercise a check that looked
solely at the table being inserted.

**Impact: high — cross-tenant data destruction, no read needed.**
`crates/slate-kernel/src/record.rs`, `deletion_closure`.

`delete` walks the foreign-key closure with `SecurityContext::superuser()`.
That is deliberate and documented — a child hidden from the deleter is still a
child — and the doc comment bounds it with two claims:

> A cascade requires the caller to hold delete on the referencing table … And
> when the child is tenant-scoped its foreign key carries the tenant — the
> parent's key begins with it — so the search is confined to the caller's own
> tenant by the key encoding rather than by a filter.

The second claim only holds when the **parent** is itself tenant-scoped. A
shared reference table — `plans`, `regions`, `categories`, the table every
multi-tenant schema has — has a primary key that carries no tenant, so the
equality the cascade search derives from it pins nothing about the tenant, and
`referencing_rows` runs unpoliced over the whole child table.

```
orgs (shared)        pk (org_id)
docs (tenant-scoped) pk (tenant_id, id), FK org_id -> orgs ON DELETE CASCADE
```

A caller in tenant A holding `delete` on both tables deletes org 1 and takes
tenant B's `docs` with it. The `RESTRICT` variant is the read-side of the same
hole — a refusal tells tenant A that *somebody* references the org when nothing
they can read does.

Both are now `a_shared_parent_with_a_tenant_scoped_child_is_refused`, which
asserts that `Catalog::from_tables` returns `CrossTenantForeignKey` naming
`docs` and `orgs`, looping over `Cascade` and `Restrict` so neither arm can
regress alone. It is one test rather than two because the fix refuses the
**edge** and not either action — before it, the two halves were
`a_cascade_from_a_shared_parent_crosses_the_tenant_boundary`, which asserted
that after tenant A's delete the table was empty rather than holding tenant B's
row, and `a_restrict_refusal_discloses_another_tenants_row`.

**Fix.** The cascade search is the right shape; its *reach* is not. Two
options, in order of preference:

- Confine the search physically. When the child is tenant-scoped, prefix the
  scan with the deleting principal's tenant (`keys::table_tenant_prefix`)
  instead of relying on the foreign key to carry it. A child in another tenant
  then simply is not found — which turns the cross-tenant `CASCADE` into a
  dangling reference, so it has to be paired with the second option.
- Refuse the schema. `Catalog::from_tables` can reject a `CASCADE` (and a
  `RESTRICT`) edge from a non-tenant-scoped parent to a tenant-scoped child,
  the same way it already rejects a tenant column that is not the key prefix.
  That is the honest answer: the edge is not expressible safely, and a startup
  refusal naming the two tables is much better than a silent cross-tenant
  delete.

Either way the doc comment on `delete` needs its second bound narrowed to
"when the *parent* is tenant-scoped".

---

## 2. `insert_many` / `upsert_many` are a free cross-tenant existence oracle

**Status: FIXED.** `write_many` decides the row policy before its batched
reads, as single-row `insert` already did. Not applied to `update_many`, which
is already indistinguishable — pre-checking there would replace `RowNotFound`
with `RowCheckFailed` and reintroduce this disclosure from the other side.

**Impact: high — cross-tenant disclosure of primary keys *and* unique-index
values, over gRPC, with nothing written.**
`crates/slate-kernel/src/record.rs`, `write_many`.

The single-row `insert` runs `check_row` — the tenant restriction and the
policy's `WITH CHECK` — *before* it reads storage, so its `DuplicatePrimaryKey`
can only ever be about the caller's own tenant. `write_many` runs the same
check **after** the reads and after `check_unique_slots`:

```
authorize → validate → intra-batch checks → check_constraints
  → read_rows_concurrently(arbitrary keys)     ← unpoliced
  → check_unique_slots(...)                    ← unpoliced
  → per row: existing.is_some() && Insert -> DuplicatePrimaryKey
             check_row(...)                    ← the tenant restriction, finally
```

So a caller in tenant A sends a row carrying `tenant_id = B` and reads the
tenant boundary off the error code:

| what is true in tenant B | what tenant A gets back |
| --- | --- |
| the primary key is taken | `ALREADY_EXISTS` (`DuplicatePrimaryKey`) |
| a unique-index value is taken | `ALREADY_EXISTS` (`UniqueViolation`, **naming the index**) |
| neither | `PERMISSION_DENIED` (`RowCheckFailed`) |

Both probes fail, so nothing is written and the probe is free and repeatable.
The unique-value probe is worse than the key probe: it discloses a *value*
(`secret@two.example`), it batches — `check_unique_slots` runs over the whole
batch before any per-row check — and `ErrorInfo.metadata["index"]` tells the
attacker which column they just confirmed.

Every wire insert goes through this path: `Write::apply` in
`crates/slate-server/src/service.rs` calls `insert_many`/`upsert_many` even for
a one-row request. Demonstrated over a real loopback gRPC connection by
`a_client_probes_another_tenants_rows_through_insert`, and at the kernel level
by `insert_many_discloses_another_tenants_primary_keys`,
`insert_many_discloses_another_tenants_unique_values` and
`upsert_many_discloses_another_tenants_primary_keys`.
`the_single_row_insert_gives_the_same_answer_both_ways` and
`update_many_gives_the_same_answer_both_ways` are the controls: those two paths
are correct.

**Fix (small and clearly correct).** In `write_many`, hoist the `WITH CHECK`
above the reads, exactly as single-row `insert` does. After the existing
`for row in rows { check_constraints(table, row)?; }` loop and *before*
`read_rows_concurrently`, add:

```rust
for row in rows {
    // Insert's WITH CHECK is a property of the row alone, so it can be
    // decided before anything is read — and must be, or the reads below
    // answer questions about tenants the caller cannot name.
    if mode.may_insert() {
        self.check_row(context, table, Action::Insert, row)?;
    }
}
```

`Action::Insert` is the right action to pre-check even for `Upsert`: the tenant
restriction is the same expression for both, so a row outside the caller's
tenant is refused either way, and a row inside it that turns out to exist still
gets the full `Action::Update` check in the existing loop. For `BulkMode::Update`
the pre-check must **not** be added — `update_many` is already
indistinguishable, and pre-checking would make `RowCheckFailed` fire where
`RowNotFound` fires today.

That closes the cross-tenant case entirely. The same-tenant, RLS-hidden case
(§4) survives it and needs a separate decision.

---

## 3. `EXPLAIN` reads other tenants' values out of the planner's histograms

**Status: FIXED by gating, not by removing the channel** — and the distinction
matters enough that a test asserts it. Explaining a plan is now
`Action::Explain`, a distinct action that `Action::ALL` deliberately excludes,
so a `read` grant no longer carries it. The statistics stay global and nothing
about planning or execution changed.

The reopening knob is a grant rather than a flag, so it is per-role and
per-table: `actions = ["all", "explain"]` in the daemon's TOML, or
`everything`. `granting_explain_reopens_the_recovery_in_full` shows what that
costs — the binary search still recovers tenant B's smallest salary exactly —
so the cost is recorded next to the switch rather than in a commit message.

Every table of a multi-table plan is checked, not just the first: an
explanation reports an estimate per side, so a caller permitted to explain one
table could otherwise read the other's statistics by joining to it. That case
was found by mutation testing, not by design —
`explain_on_one_table_does_not_carry_to_the_other_side_of_a_join` exists
because gating only the first table passed the entire suite.

**Impact: high — verbatim recovery of column values from tenants the caller
cannot read a single row of.**
`crates/slate-kernel/src/stats.rs` (`Histogram`, `bounded_selectivity`),
`crates/slate-kernel/src/explain.rs`, `crates/slate-serverd/src/seed.rs::analyze`.

`analyze` is per-caller and correctly describes only the caller's slice — the
RLS matrix pins that. But a deployed head node does **not** use a per-caller
analysis: `analyze_on_start` runs `seed::analyze`, which reads every table as
`SecurityContext::superuser()`, precisely because per-policy statistics would
make the planner optimise for the wrong table. Those global statistics are then
what every caller's plan is costed against.

A row count is a number. A histogram bound is a **value**, sampled out of the
table, and `bounded_selectivity` reports where the caller's literal falls
relative to those bounds. `Explanation::estimated_rows` is on the wire
(`convert.rs`), so a caller who can `Explain` a table can binary-search that
comparison and recover the bounds themselves.

`granting_explain_reopens_the_recovery_in_full` does exactly that: tenant A
can read **zero** rows of `payroll` (asserted), and recovers tenant B's
smallest salary *exactly* by binary search over `EXPLAIN ... WHERE salary >= v`,
plus the shape of the whole distribution from eight more probes. Sixty-five
bucket boundaries are recoverable this way, each one a real value belonging to
whichever tenant it was sampled from. It was
`explain_recovers_a_value_from_a_tenant_the_caller_cannot_read` when this
review was written, and it still passes, because the fix gated `EXPLAIN` behind
an action of its own rather than blurring the estimate — a caller granted it
recovers exactly as much as before, which is a cost somebody should have to
read before granting it.
`explain_is_refused_to_a_caller_holding_only_read` is the refusal beside it.

Equality is not affected: `equality_selectivity` reads only the distinct count,
so `EXPLAIN ... WHERE email = 'x'` answers the same whether or not `x` exists.
It is the ordered comparisons and `LIKE 'prefix%'` (`prefix_selectivity`, same
histogram) that leak.

**Fix.** There is no cheap one; pick a position deliberately.

- **Cheapest and probably right for now:** make `Explain`/`ExplainMulti` a
  privileged operation — a distinct `Action`, or a role check — rather than
  something every reader may call. The planner keeps its global statistics and
  nothing about query execution changes.
- **Or** round what leaves the process: quantise `estimated_rows` to a coarse
  ladder (powers of two, say) before it goes on the wire. This raises the cost
  of the search rather than removing it, and it degrades the thing `EXPLAIN`
  is for.
- **Or** keep per-tenant statistics for tenant-scoped tables and cost with the
  caller's. Correct, and much the most work.

Whatever is chosen, `docs/correctness.md`'s "statistics describe only the
permitted slice" needs a note that this is true of `analyze` and *not* of the
statistics a deployed node plans with.

---

## 4. A write reports whether a row hidden by RLS occupies a key

**Status: CLOSED as inherent, with the claim narrowed and one correction to
this finding.** The oracle is real and is not removable: a unique key is a
resource shared by everyone who can write the table, and withholding the bit
means either overwriting the hidden row or accepting a write that cannot be
stored. `security.rs` now says exactly what each write path discloses, rather
than claiming a property the insert path does not have.

**The correction:** this finding said the claim was "True of `update`, `upsert`
and `delete`". It is not true of `upsert`. An upsert onto a free key succeeds
and an upsert onto a key held by a hidden row is refused, which is the same one
bit. `an_upsert_leaks_the_same_bit_as_an_insert` demonstrates it. That matters
more than it sounds: a caller reaching for an upsert *in order to avoid* the
insert oracle would be choosing it for a property it does not have.

Also checked, because it would have been much worse: an upsert does **not**
overwrite the hidden row. `an_upsert_cannot_overwrite_a_row_the_policy_hides`
pins that.

Two things bound the exposure, and `security.rs` states both: it is same-tenant
only (a tenant-scoped table puts the tenant in the key prefix, so there is no
collision to observe across tenants), and it requires the attacker to be able
to *name* the key — a UUID or sequence-drawn key leaves nothing to probe for.

**Impact: medium — same-tenant existence oracle; contradicts a stated
invariant.**
`crates/slate-kernel/src/security.rs` module docs, `record.rs::insert`.

`security.rs` says:

> A write aimed at a row the policy hides reports the row as missing rather
> than as forbidden, so the error cannot be used to probe for existence.

True of `update`, `upsert` and `delete`. Not true of `insert`: a key occupied
by a row the caller's policy hides answers `DuplicatePrimaryKey`, and a free
key answers success. `an_insert_reports_whether_a_hidden_row_occupies_the_key`
shows Alice failing to read Bob's note and then learning it is there by trying
to take the key.

This is the same oracle Postgres RLS has, and it is not obviously removable —
the key really is taken, and pretending otherwise means either overwriting
Bob's row or accepting a write that cannot be stored. The finding is that the
invariant as written is false, and that §2 makes the same oracle reach across
tenants where this one does not.

**Fix.** Narrow the claim in `security.rs` to the update/delete paths and state
the insert case, rather than changing behaviour. Fixing §2 is what actually
matters.

---

## 5. `RESTRICT` discloses a referencing row in another tenant

**Status: FIXED with finding 1** — it is the read side of the same edge, and
the catalog now refuses the edge for either action.

**Impact: medium — one bit per probe, cross-tenant.** Covered under §1; the
demonstration is the `Restrict` arm of
`a_shared_parent_with_a_tenant_scoped_child_is_refused`, and was
`a_restrict_refusal_discloses_another_tenants_row` before the fix merged the
two. The same fix closes it.

---

## 6. Trusted-header mode: the client's copy of an identity header wins

**Status: FIXED.** `text` reads `get_all` and refuses a key that appears more
than once, rather than resolving it. Refused rather than resolved because there
is no safe pick: taking the last trusts a proxy that appends, taking the first
trusts one that replaces, and the server cannot tell which it is behind. The
refusal covers identical duplicates too — treating those as benign would make
the check pass or fail depending on what the attacker chose to send — and the
message names the duplicate without echoing the identity that was claimed.
`a_duplicated_identity_header_is_refused_rather_than_resolved` now asserts the
refusal.

**Impact: medium — total impersonation, but only under a specific (and easy)
proxy misconfiguration.**
`crates/slate-server/src/auth.rs::MetadataIdentity`.

`MetadataIdentity` reads `metadata.get(key)`, which returns the **first** value
for a repeated header. The mode is documented as correct behind a proxy that
"sets these three headers itself, and strips any copies the client supplied" —
and the failure mode when the proxy *appends* instead of stripping is not
graceful degradation, it is complete: the client's `slate-principal`,
`slate-tenant` and `slate-roles` all win over the proxy's, because they arrive
first. That is the difference between `proxy_set_header` and `add_header` in
nginx, and between `set` and `append` in most mesh sidecar configs.

`a_duplicated_identity_header_is_refused_rather_than_resolved` carries that
fixture: a client claiming principal `666` in tenant `2` with role `admin`
while the proxy's own headers say `1`, `1`, `app`. Under its original name,
`the_first_copy_of_a_duplicated_identity_header_wins`, it asserted the client
won. It now asserts an `Unauthenticated` that names the duplicate and does not
echo `666` — refusing rather than resolving, because resolving either way is a
guess about a proxy this server cannot see.

**Fix (one line each, clearly correct, defence in depth).** Refuse a repeated
identity header rather than picking one. In `text()`:

```rust
fn text(metadata: &MetadataMap, key: &str) -> Result<Option<String>, Status> {
    let mut values = metadata.get_all(key).iter();
    let Some(value) = values.next() else { return Ok(None) };
    if values.next().is_some() {
        return Err(Status::new(
            Code::Unauthenticated,
            format!("`{key}` appears more than once; a proxy that sets the identity \
                     headers must strip the client's copies rather than append to them"),
        ));
    }
    value.to_str().map(|s| Some(s.to_owned())).map_err(...)
}
```

A correct deployment never sees this error. A misconfigured one fails closed
and says why, instead of serving whoever asked.

---

## 7. Unbounded per-request work and memory

**Status: FIXED, in two different ways, because two different things were
wrong.**

The `IN` list was a *cost* bug, not a missing cap: the values stayed in the
residual and were rescanned per row. `Expr::InSorted` arranges them once per
plan, so a row costs a binary search. Five runs: 4.03–4.68x for 50,000 values
against 8, down from 362x. No cap was added, because the amplification it would
have capped is gone.

The three accumulators were genuinely unbounded, and now refuse:
`ExecutionLimits` gives `GROUP BY`, `COUNT(DISTINCT)` and an unlimited
`ORDER BY` a ceiling each, with `TooManyGroups`, `TooManyDistinctValues` and
`SortTooLarge` naming the limit that was passed. Refused rather than truncated
— a silently short answer is worse than an error. The `ORDER BY` message says
that adding a `LIMIT` uses the bounded heap instead, which is the mitigation
that already existed and was unreachable without one.

The daemon gains `max_concurrent_requests` and `request_timeout`, both unset by
default: a concurrency limit low enough to protect a small node is low enough
to break a large one, and there is no right number without knowing the machine.
Having no way to *say* one was the defect. Zero is refused for every one of
these settings rather than read as "no limit", because a config that disables
the feature it appears to configure is worse than one that will not start.

**Impact: medium — one authenticated caller can pin the node.**
`crates/slate-kernel/src/{expr,aggregate,exec}.rs`,
`crates/slate-serverd/src/serve.rs`.

A join is the only thing with a budget: `DEFAULT_BUILD_LIMIT` and
`JoinBuildTooLarge`, and a client-supplied `build_limit` is clamped down and
never up (`build_limit_from_proto` — correct, and checked). Nothing else that
holds unbounded state or does unbounded work per request has an equivalent.
`security_probe_resources.rs` records four:

- **`IN` list length is per-row work.** `MAX_POINT_GETS` and
  `MAX_INDEX_RANGES` (1024 each) stop a long list *becoming an access path*;
  they leave every value in the residual, where `Expr::In` re-scans the list
  linearly for each candidate row. Measured on a 2,000-row table: `IN` with 8
  values, 8 ms; with 50,000 values, **2.98 s** — 362x. tonic's 4 MB default
  decode limit admits a few hundred thousand values, and the amplification is
  multiplied by the table's row count.
- **`GROUP BY` holds one entry per distinct key**, no cap
  (`Grouper::groups`). Grouping a large table by a unique column is a
  request-sized allocation of the whole table.
- **`COUNT(DISTINCT)` holds every distinct encoded value**, no cap
  (`Accumulator::Distinct`), and is documented as exact by design.
- **`ORDER BY` with no `LIMIT` materialises the whole result** before the
  first row (`QueryCursor::open`, the `None` arm). With a limit it uses the
  bounded heap, which is the mitigation not being applied here.

`slate-serverd` adds no concurrency limit, no request timeout and no
`max_decoding_message_size` override, so these are as many concurrent copies
as the caller opens connections. `Limits::max_transactions` (1024) bounds open
transactions and nothing else.

**Fix.** Give the three unbounded accumulators the treatment the join already
has — a limit on the config, an error variant that names it, refused rather
than killed. For the `IN` list, either cap the number of values a predicate may
carry (refused at `expr_from_proto`, where the message is best) or turn a large
`Expr::In` residual into a hash set once per plan rather than a linear scan per
row. A request timeout and a concurrency limit in `serve.rs` are worth having
regardless.

---

## 8. Schema disclosure to an authenticated caller with no grant

**Status: FIXED for the shape; table existence is disclosed deliberately.**
The four handlers that fingerprint-check now authorise first, via
`Head::authorized_table`, so a caller with no grant is refused before the
fingerprint runs and a right guess is indistinguishable from a wrong one —
same code, same message. `a_caller_with_no_grant_cannot_confirm_a_tables_shape`
asserts both.

Table *existence* still differs: an unknown name answers `NOT_FOUND`, a known
one with no grant answers `PERMISSION_DENIED`. Kept, and the same choice
Postgres makes — collapsing them hides a table from someone who cannot use it
anyway, and makes every ordinary misconfiguration indistinguishable from a
typo.

The hazard of authorising early is authorising *differently*, and the first
version of the test could not see it: with only a blanket-granted role to test
against, a handler checking `Explain` where it meant `Delete` passed. The
fixture grew four single-action roles, and each handler is now exercised by a
role holding exactly the one action it needs.

**Impact: low.** `crates/slate-server/src/service.rs`,
`crates/slate-server/src/fingerprint.rs`.

Handlers resolve the table (`NOT_FOUND` for an unknown name) and run
`fingerprint::check` before the kernel's RBAC check, which happens inside the
planner. So a caller who is authenticated but holds no grant on `users` can
still learn that `users` exists, and can confirm a guessed
`(name, type, key)` layout for it one 64-bit fingerprint at a time. The
fingerprint is not a secret and the check has to run before the values are
read — it is on the request path for a good reason — so this is noted rather
than pressed. Both are after authentication; neither is reachable
unauthenticated.

---

## Probed and clean

These were attacked deliberately and did not yield. Listing them so the next
review spends its time elsewhere.

**RLS across access paths** (`crates/slate-kernel/tests/rls_probe.rs`, 20
tests, all green — this is the matrix extended to the paths it does not cover):

- `IN` over a secondary index lowered to disjoint index ranges
  (`Access::IndexScans`), ascending and descending.
- A covering scan over an **expression index**, both ways round: with a policy
  on a column the entry does not hold (the planner correctly refuses to claim
  the scan covers the query, because `needed` is computed from the *secured*
  predicate), and with a genuinely covering scan under tenant scoping alone,
  where the tenant equality is evaluated against the value decoded out of the
  entry's primary key. The `Index Only Scan` shape is asserted in the second
  case so it cannot silently become a re-test of the ordinary path.
- A partial index, forced by hint.
- `count(*)`, `min`, `max`, `sum`, `avg`, `count(distinct)`, each forced onto
  each index in turn, including index-only paths.
- `GROUP BY` on an indexed column through an index-only read, and `GROUP BY`
  on a value taken out of an expression index's entry — no group key from a
  hidden row in either.
- Hash join (both build sides), index nested-loop join, grouped join, and a
  three-table chain.
- `analyze` under a policy, including that no histogram bound comes from a
  hidden row.
- Deep offsets, `ORDER BY` on an unprojected column through the bounded heap,
  an empty projection, and filters naming another tenant directly (as a scan,
  as a forced index scan, and as an `IN` on the tenant column lowered to point
  gets).

**Why these hold, structurally.** The security filter is conjoined onto the
caller's predicate *before* planning (`SecuredReads::plan`), and the whole
conjunction stays in `Plan::residual` and is re-evaluated per row — bounds only
narrow. Covering is decided by `plan::covers` against `needed`, which is built
from that same secured predicate, so a policy on a column an index lacks
prevents the index-only scan rather than being skipped by it. `Expr::columns`
covers every variant, so nothing in a policy is invisible to that check. Joins
and chains plan every side through `SecuredReads::plan`, and a nested loop's
probe is a whole secured read with the join equality conjoined on. Three-valued
logic means a column missing from a rebuilt row makes its comparison unknown,
which does not admit.

**Authentication** (`crates/slate-serverd/src/auth.rs`,
`crates/slate-server/src/auth.rs`): no configuration produces a superuser (there
is already a test for it, and it holds); `[auth]` is mandatory and every mode
whose safety depends on something outside the process must name it off
loopback; fields belonging to another mode are refused rather than ignored;
bearer tokens are compared in constant time with the whole list walked, are
read only from the environment or a file, have a 32-character floor, and
duplicate secrets are refused at startup. `TokenIdentity` ignores the
`slate-*` identity headers entirely, so token mode cannot be talked into
trusting a header. The only authentication finding is §6.

**Session ownership** (`crates/slate-server/src/session.rs`): a transaction
handle is bound to the full `Principal` — id, tenant *and* role set — and a
mismatch answers `NOT FOUND`, the same as an unknown handle. Transaction ids
are v4 UUIDs.

**Wire ordinal resolution** (`crates/slate-server/src/convert.rs`): every
`ColumnRef` is bounds-checked against the input it names; a computed value is
refused in a joined space (no slot for one), across inputs, and where a stored
column is required; group-key and aggregate references are checked against the
grouping's own arity; `check_input` stops a condition on one input naming
another; `Shape::Joined { visible }` stops a step reading an input it has not
reached. `primary_key_from_proto` checks arity *and* per-column type, and the
refusal is `INVALID_ARGUMENT` — distinct from the deliberately
indistinguishable `found: false`, which is the right way round.

**Tenant isolation as a key prefix** (`crates/slate-kernel/src/keys.rs`,
`slate-schema`): the tenant column is proved to be primary-key column zero at
schema build time; index keys on a tenant-scoped table are tenant-prefixed,
including unique-index slots, so a unique constraint does not span tenants;
`Catalog::from_tables` refuses a duplicate `IndexId` across tables, which had
been a real shared-key-range bug. Read tokens are bare sequence numbers and
carry no tenant. Replica affinity uses the principal's tenant, never one dug
out of the filter, and affinity only chooses *which* replica — every replica
read goes through the same `SecuredReads`.

**The configuration language** (`crates/slate-serverd/src/lang/`,
`security.rs`): `:principal` and `:tenant` are `Value`s dropped into a slot the
parser already typed, never text spliced before parsing, so there is no
injection shape; a missing tenant lowers to `Value::Null`, which is *unknown*
in every position including under `NOT` and `NOT IN`, and unknown never admits;
placeholders are refused in the scopes lowered once at startup (`CHECK`,
partial index, expression index); RLS with no applicable policy is
`Expr::False`, not `Expr::True`; an empty grant list warns and denies; a policy
with no grant behind it warns. A literal is typed by the column opposite it, so
a mistyped one cannot silently compare type-first. I did not find a
configuration that grants more than it reads as granting.

**Foreign-key *checks*** (as opposed to cascades): `check_foreign_keys` and
`visible_parents` use `SecuredReads::get`, so a parent hidden from the caller is
absent for them, and the grant is required too.

**Not examined in depth**, and therefore not cleared: the lease and leadership
protocol (`lease.rs`, `leadership.rs`, `filelease.rs`), the S3 backend and its
credential handling (`slate-slatedb`), the Python client, and the tuple codec's
behaviour on adversarial encoded input beyond the existing
`slate-tuple/tests/untrusted.rs`.

---

## Where the remaining risk is concentrated

**Not in the read path.** The structural argument — the policy *is* the
predicate, bounds only narrow, covering is decided from the secured predicate —
holds up under pressure, and every new access path I could reach honoured it.
That is the part of this system that was designed as a security surface, and it
shows.

**The risk is in the paths that were designed as *correctness* surfaces and
inherited a security consequence.** All four real findings have that shape:

- Referential integrity is deliberately not relative to who is asking, and the
  bound that was supposed to keep it inside a tenant is a property of the key
  encoding that a perfectly ordinary schema does not have (§1).
- The bulk write path exists to overlap round trips, and overlapping them moved
  the reads above the check that decides which keys the caller may name at all
  (§2). The single-row path it was modelled on has the order right; the batch
  version does not, and no test compared them.
- Statistics exist so the planner does not optimise for the wrong table, which
  is exactly why they must be global — and a global histogram is other tenants'
  values in a structure everyone can query (§3).

The common factor is that each is a place where a *non-security* requirement
argued for reading or reporting more than the caller may see, the argument was
written down carefully, and the bound the argument relied on was stated rather
than enforced. `record.rs`'s cascade comment and `security.rs`'s "cannot be
used to probe for existence" are both precise, well-reasoned, and wrong at the
edges — which is a much harder failure to catch by reading than a missing
check.

Concretely, I would look next at:

1. **Every remaining `superuser()` inside the kernel.** There is exactly one
   (`deletion_closure`) and it is finding §1. Any future one deserves the same
   treatment: not "is this justified" but "what physically stops it reaching
   another tenant".
2. **Every pair of single-row and batch operations.** §2 is a divergence
   between two spellings of one operation. `update_many`/`update` and
   `delete` are fine today; the next batch method added is the risk. A
   differential test that runs each pair against a hostile row and asserts the
   *same error variant* would have caught it and would catch the next one.
3. **Everything derived from global state that a per-caller request can
   observe:** statistics (§3), `EXPLAIN` output, plan choice, and timing. The
   histogram is the sharpest instance because it holds literal values, but
   `estimated_rows` is a channel out of superuser-gathered state in general.
4. **Resource budgets** (§7). The join has one and nothing else does; that
   asymmetry looks accidental rather than argued.
