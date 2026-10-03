# A column grant is only as narrow as the widest path a row leaves by

`views.md` §2 ends on a debt:

> "Give the analysts a view over the non-sensitive columns" is *the* reason
> people reach for views, and it does not work here — the analysts would still
> need a grant on the base table, and having it they could read the columns the
> view omits. … Column-level grants are the feature that would actually serve
> that use, and they are a different item.

This is that item. The obvious design is a list of columns on `Grant` and a
check in the projection. That design would ship a privilege boundary with at
least ten holes in it, because **the projection is not how columns leave this
system**. Most of this note is about the other ways.

It settles seven things, builds nothing, and leaves two questions open.

## What it does today, measured

The table `staff (id, name, salary, owner)` has one policy, `owner =
:principal`. A role `analyst` holds `Read` on the table. Principal 7 owns both
rows. Each read below asks for `name` only:

```
project [name]                         -> [1, "ann", Null,   7]  [2, "bob", Null, 7]
project [name] where salary > 100000   -> [2, "bob", 120000, 7]
get(1)                                 -> [1, "ann", 90000,  7]
```

This was run against the kernel through a throwaway test that was not kept.
Three things come back that were not asked for:

- **`owner`, on every row.** It is the policy's column. The security layer
  ANDs the policy into the query's predicate before planning (`read.rs`,
  `SecuredReads::plan`), and the planner decodes every column the predicate
  reads into `output_columns` (`plan.rs`, `plan_hinted`). The decoded column is
  returned with the row.
- **`salary`, whenever the caller filters on it.** This happens for the same
  reason. A caller who could not *project* `salary` could still read it,
  exactly, by filtering on it.
- **Everything, on `get`.** It reads the row unchecked and applies only the row
  filter.

None of this is a bug today. With table-level grants a caller who can see one
column can see them all, so returning extra columns discloses nothing. **Every
one of them becomes a disclosure the moment a grant can name columns**, and
that is the shape of the work.

## 1. An existing table-level grant means every column, now and later

`Grant::new(role, table, actions)` keeps its meaning. It covers every column
of the table, including columns added after the grant was written. A column
grant is a *second, narrower form* of grant, and a role's readable columns on
a table are the **union** over every grant it holds there:

```
readable(context, table) =
    every column                                   if superuser
    every column                                   if any held grant is table-level
    ⋃ { grant.columns : held column grants }       otherwise, plus the tenant column (see the end)
    ∅  (and the read is denied as today)           if none
```

**Why not make a table grant an exclusion list instead**, with "Read on
`staff`, except `salary`" as a deny rule? Because grants here are **additive
and only additive**. `SecurityCatalog::grants` is an `any` over a list, with
no deny case anywhere. That property is why the check is one line long and
cannot be wrong in an order-dependent way. A deny rule would put precedence
into the authorization model in order to add a convenience. It would also
**fail open on schema change**: a column added to `staff` tomorrow would be
readable by the analysts unless someone remembered to extend the exclusion.

**Why the asymmetry is acceptable.** A table grant sees new columns
automatically and a column grant does not. That is the right way round for
each. The holder of a table grant was trusted with the table. The holder of a
column grant was trusted with a list. A schema change can therefore widen only
what someone was already trusted with wholesale, and it never fails open for a
narrowed role. Postgres draws the same line between table-level and
column-level `SELECT`. That is recalled from its documentation, not tested
here.

**Every column grant must include the whole primary key.** This is refused at
load otherwise. A row is identified by its key: the cursor that pages a result
is the key, `get` takes it, and every write names it. A role that could read
rows but not their keys could not page, could not fetch one row back, and
could not ask about a row it had just seen.

## 2. A query that *references* a column it cannot read is refused, wherever the reference is

It is refused whether the column appears in the projection, the filter, an
`ORDER BY`, a `GROUP BY`, an aggregate's input, a window's partition, order or
argument, a computed value's inputs, or a join key. The check is on the
**caller's** expressions and runs before planning.

The alternatives, and why each fails:

- **Check the projection only.** This is the obvious design, and the
  measurement above is the counterexample: `where salary > 100000` returned
  the salary. Even with the value nulled out of the row, the filter is a
  probing channel. Binary search on `salary > x` reads any salary below
  131,072 to the dollar in seventeen queries: one bit each.
- **Return null for what cannot be read, and allow references.** This has the
  same probing channel. It also adds a null that means "hidden", which no
  caller can tell from a null in the data.
- **Allow aggregates over hidden columns**, on the grounds that a sum is not a
  value. `sum(salary) where id = 5` is a value. Restricting aggregates to
  groups above some minimum size is differential-privacy territory. That is a
  different feature, and the honest version of it is not a row count.

Refusing a reference anywhere is also what Postgres does with column `SELECT`
privilege. It applies to `WHERE` as much as to the target list, again recalled
rather than tested. It means the check needs one walk over the caller's
expressions and no new semantics.

**`Projection::All` is narrowed, not refused.** For a column-restricted
caller it resolves to the readable columns. Every client defaults to all
columns, so refusing `All` would turn every default query of a narrowed role
into an error. Narrowing only ever removes columns, so it cannot widen
anything. A caller who *names* an unreadable column is refused, because they
asked for something specific and should be told they cannot have it.

## 3. What the system adds to a query is decoded, used, and then dropped

The policy's columns, the tenant column and the soft-delete column are added
by the security layer, not by the caller. A policy *may* read a column its
subject cannot, and that is legitimate: "analysts see rows where `region =
:tenant`" does not grant the analysts `region`. These columns are not caller
references, so §2 does not refuse them. The planner must still decode them to
evaluate the filter.

So `output_columns` splits into what is **decoded** and what is **returned**.
The returned set is the caller's projection intersected with `readable`, plus
the primary key. Everything else that was decoded is nulled before the row
leaves the executor. **The mechanism already exists.** `exec.rs` nulls the
inputs of a computed value that the caller did not project (`transient`), for
exactly this reason, so this widens an existing rule rather than adding one.

Rows still being *filtered* by a hidden column is not a leak worth closing.
The caller learns that some row they are allowed to see exists, which the
policy decided they may see. They cannot vary the hidden column's comparand,
because the policy wrote it, not the caller.

## 4. Every path a row leaves by, and what it does

The map is the point of this note. If a path has no line here, it has a hole.

| path | today | with column grants |
| --- | --- | --- |
| `Query` / `execute`, cursors | projection + predicate + sort columns | §2 refusal, §3 split; cursor is the key (§1) |
| `Get` | whole row | narrowed to `readable`, nulls elsewhere |
| `Join`, `Chain` | each side as its own query; probed side returns its join key | §2 per side; a join key is a reference |
| `Aggregate`, grouped forms | values; any column may be aggregated | §2: inputs and keys are references |
| windows | projection widened with partition, order and argument columns | §2: all three are references |
| computed values (incl. vector distance) | always returned | §2: a computed value's inputs are references |
| `Related` | `Query::all()`, whole rows | `All` narrowed (§2) |
| `DeleteWhere` / `UpdateWhere` `returning` | whole rows | narrowed; the predicate and every assignment's scalar are references |
| `Explain*` | the secured predicate as text, policy literals included | refused for a column-restricted caller (§5) |
| `analyze` | statistics over every column | refused for a column-restricted caller (§5) |
| `update_if_unchanged` / `delete_if_unchanged` | compares the whole `expected` row | refused for a column-restricted caller (§6) |
| whole-row `update` / `upsert` | replaces every column | refused for a column-restricted caller (§6) |
| `UniqueViolation` | names the index, so it answers "is this value taken" | unchanged; a write-side question (§6) |

The only paths that need no change are the index-only rebuild and the
error-message texts, which were checked and echo no values.
`row_from_index_entry` restricts to the wanted columns, and CHECK, foreign-key
and row-check failures name tables, indexes and keys only.

**The build will need a guard, as `scripts/check_handlers.py` was for table
grants.** A new row-returning path added later without the narrowing is the
predictable way this regresses. The guard should require every function that
hands rows to a caller to call the one narrowing function, or to be rostered
with a reason.

## 5. `Explain` and `analyze` require every column

`Explain` prints the secured predicate, so the policy's literals and the names
of the columns it reads appear in the plan text. `analyze` returns histogram
bounds that are *values sampled from the table* (`record.rs` says so in its
own docstring), from every column. Each could be narrowed: a redacted plan
printer, or statistics over the readable columns only. Each narrowing is a new
surface written to be partial, and partial printers drift. **Both are refused
for a caller whose `readable` is not every column**, and a serverd
configuration granting `explain` to a role on a table where that role is
column-restricted is refused at load, so the refusal shows up in the config
rather than in a query. `Explain` is already outside `Action::ALL` because it
is privileged. This makes it more so.

## 6. Column grants are `read` only, and a narrowed reader may not replace whole rows

A column grant carrying anything other than `read` is refused at load in this
design. Column-level `insert` and `update` are a real feature with their own
questions, and none of them is needed for the use case that motivates this
note. What does a partial insert put in the columns it cannot write? Defaults,
or must the columns be nullable? Can a role write a column it cannot read?
That is the "blind write" Postgres allows. And `UniqueViolation` answers "is
this value taken" for any unique column the writer touches.

What this design *does* have to decide is the interaction with table-level
writes. A role can hold column-restricted `Read` and table-level `Update`
together, and grants are additive. `update` replaces the whole row, so such a
role would have to submit values for columns it cannot see. It would then
either overwrite them blind or guess them, and `update_if_unchanged` would
turn its `expected` row into an equality oracle on every hidden column.
**Whole-row `update`, `upsert` and the two `_if_unchanged` forms are refused
for a column-restricted reader.** `update_where` remains available, because
its assignments name the columns they write and §2 already makes their scalars
references.

## 7. A hidden column draws the same error as a column that does not exist

A column-restricted caller who names a hidden column gets the same
`NoSuchColumn` as one who names a column that is not there, at the same place
in the wire path (`resolve_stored_column` in `convert.rs`), so the error does
not confirm the column exists. Answering `AccessDenied: column salary`
instead would hand the narrowed role a schema oracle. A role restricted to
`id, name` could enumerate the table's other columns by guessing names. That
is the same class as security-review finding 8, a column *count* disclosed to
a caller with no grant, which was treated as a defect there.

The cost is a worse message for a misconfigured role: "no such column
`salary`" when the column is right there in the schema file. That cost is
accepted, and the operator's side gets the precise answer. The load-time
summary that serverd already prints for grants should list each column grant
in full.

## The wire and the clients

**Configuration, not protocol.** Grants are server configuration. No request
carries one, and `records.proto` says there is no principal on any request.
The surface is one optional key on the existing block:

```toml
[[security.grants]]
role    = "analyst"
tables  = ["staff"]
actions = ["read"]
columns = ["id", "name"]     # absent: every column, as today
```

It is refused at load when any of these holds:
- `columns` is present and `tables` names more than one table;
- `actions` holds anything but `read` (§6);
- a column does not exist;
- the primary key is incomplete (§1);
- the same role holds `explain` on that table (§5).

`deny_unknown_fields` currently makes `columns` a hard error, so an older
server refuses a newer config rather than silently ignoring the restriction.
That is the fail-closed direction, and it should be pinned by a test rather
than left as a side effect.

**The clients need no new API.** A narrowed caller sees nulls in hidden slots
for `All`, and the existing `PERMISSION_DENIED` and `NOT_FOUND` mappings for
refusals. They do need **one conformance case each**: the same restricted
identity, against the same server, sees the same narrowed rows and the same
refusals in Python, Go and TypeScript. That is the three-SDK runner's job.

## How it will be shown to hold

A hand-written test per path would test the paths somebody thought of, and §4
exists because the obvious design forgot most of them. So the primary test is
an **oracle over bytes**:

- Plant a sentinel value in every hidden column, chosen so it appears nowhere
  else in the fixture.
- Run every query shape the existing suites already generate under the
  restricted identity. That includes the RLS matrix, the join and chain
  suites, aggregates, windows, the nearest-neighbour cases, `returning`
  writes, and the three clients' conformance runner.
- Scan **every response, as bytes**, for the sentinel. That covers rows,
  computed values, cursors, error messages, plans and statistics.

Any occurrence is a leak, whatever path produced it. Every refusal must also
be an error the clients map, never a dropped connection. The differential
half: wherever a restricted query is *not* refused, its rows must equal the
unrestricted caller's rows with the hidden columns nulled.

## Build order

Each step is refused-by-default until the next lands, so no intermediate state
fails open:

1. **Kernel.** `Grant` gains an optional column set, and `SecurityCatalog` gains
   `readable(context, table)`. Column grants are refused for any action but
   `Read`, and refused when the key is incomplete.
2. **Reads.** Add the §2 reference check in `SecuredReads::plan` and the §3
   decoded/returned split in the executor. Narrow `get`, `Related` and
   `returning`. Refuse `Explain`, `analyze`, whole-row writes and the
   `_if_unchanged` forms for restricted callers.
3. **The sentinel oracle** across the kernel suites, with the new guard
   requiring every row-returning path to pass through the narrowing function.
4. **serverd.** Add the `columns` key and its load-time refusals, and make the
   wire's column resolution answer `NoSuchColumn` for hidden columns (§7).
5. **The three clients**, through the conformance runner, with the sentinel
   scan over their responses.

## Left open

**Whether a column-restricted role may filter on a column it can read but the
*index* it would use cannot answer.** It may, and nothing here changes
planning. But a covering-index plan and a table scan must return the same
columns. `plan.rs` already insists on that equivalence for projection, and the
§3 split has to preserve it. This is flagged for the build rather than
decided, because the right answer depends on where the split lands in the
executor.

**Whether views should then narrow columns.** `slate-server/src/views.rs`
refuses a column-narrowing view because it would break the plain `and`
composition. With column grants, the analyst use case no longer *needs*
narrowed views: grant the columns. Whether views should carry column lists as
a convenience on top is a separate question, and is not decided here.

## What this note does not do

**It builds nothing.** The only thing run was the three-line measurement at
the top, through a test that was deleted afterwards. Every other claim about
current behaviour is read off the code, and §4 is the map that the build is
expected to falsify in places.

**The Postgres comparisons are recalled, not tested**, as in `views.md`. If
Postgres differs, the decisions here are unaffected. They follow from this
system's additive grants and its missing owner concept, not from imitation.
Only the framing of "what a user will expect" is affected.

**It does not cover column-level writes** (§6) **or inference through
aggregates over readable columns that correlate with hidden ones.** A role that
can read `title` and `department` can often infer `salary` band. No access
control on columns closes that, and this design does not pretend to.

**It does not address the tenant column.** On a tenant-scoped table the tenant
column is the caller's own tenant on every row they can see. Hiding it would
hide only what the caller already supplied, so it is treated as readable
whenever the table is.
