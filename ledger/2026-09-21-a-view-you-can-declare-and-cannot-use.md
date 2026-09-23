# Views can be declared in TOML, resolved at boot, and used by nothing — which is the whole of step 1.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #256 (F5b)
- **Touches:** `crates/slate-serverd/src/{views.rs,config.rs,main.rs}`, `crates/slate-serverd/tests/refusals.rs`, `docs/views.md`, `site/docs/roadmap.html`
- **Kind:** feature

## What changed

`[[views]]` in a `head.toml`: a name and a `SELECT`. At startup the statement
is parsed against the catalog's own tables, checked against a list of
everything a view may not be, and stored in a `BTreeMap` held *beside* the
`Catalog` rather than in it. `--print-schema` publishes the resolved specs
under a new top-level `views` key. No read path knows the map exists, so every
query naming a view is refused — by `Catalog::table_by_name` finding nothing,
not by a check anybody wrote.

`docs/views.md` gains a "Where step 1 landed" note under §3a, and the site's
roadmap page said *"Views are designed and not built"*, which stopped being
true with this commit.

## Why

`docs/views.md` §3a settled the build order before this code: *declare first,
read last*. The argument is that a view kept out of the `Catalog` is already
refused by every path that resolves a table name — writes, joins, chains,
aggregates, paging, all of them — because there is nothing to find. Teaching
the reader about views first and then remembering to refuse the writers is the
shape of every finding in `security-review.md`: a fix that covered one path of
several. This order has no such intermediate state, so it is worth shipping as
its own commit even though the feature does nothing observable yet.

Resolution happens at boot rather than per query for the reason indexes and
checks are validated at boot: a view naming a column that does not exist should
be a refusal to start with a byte offset into the SQL, while somebody is still
reading the config, not a surprise the first time a caller reads through it.

## Alternatives rejected

**Put views in the `Catalog` with their own `TableId`.** The obvious design,
and `views.md` §1 is four paragraphs on why it is wrong: `row_filter_with` keys
row-level security on a `TableId`, a view's own id has no policy, so the filter
comes back `Expr::True` over the base table's rows. It would cost the security
property the whole feature has to preserve, in exchange for not writing a
second map.

**Allow a projection.** The feature people actually want from views is "give
the analysts a narrowed view", and half of narrowing is columns. Refused
because a projection makes the caller's ordinals *view* ordinals, and remapping
them onto base ordinals is a separate piece of work whose failure mode is
silent: a column shifted by one returns the wrong data under the right header.
Row-narrowing alone is still useful, and §2 already records that a view here is
not a privilege boundary anyway — the caller needs the grant on the base table
and can query it directly — so a projection would not be buying the security
people would assume it was.

**Hand-write the refusal list and stop there.** That was the first draft. A
list of `QuerySpec` fields in another crate is exactly the thing that goes
stale, and going stale here means silently *accepting* a clause nobody has
thought about, in the one function whose job is to refuse. So the list now only
picks the *message*; the decision is `beyond_a_where`, which serialises the
spec and refuses any key that is not `table`, `filter` or `filters`. That works
because every other field on `QuerySpec` carries a `skip_serializing_if` for
its default — written that way for the workbench's spec panel, and leaned on
here.

**Refuse by comparing against an allowed shape** (`spec == QuerySpec { table,
filter, ..default() }`). Reads as thorough and is not: an equality check
against a struct literal has the same staleness problem as the list, because a
field added next year is absent from both. The serialised-key check is the
version that cannot miss one.

**Skip the `--print-schema` half.** Without it the resolved views are computed
and dropped, which is dead code clippy would reject and, worse, a validation an
operator cannot see the result of. Publishing the compiled spec is the half
they cannot otherwise get: the SQL is in their own file, what the server
resolved it to is not. Published beside the tables and not among them, so a
generator reading the dump does not emit a row type for something with no id,
no index and no write path.

## Evidence

`cargo test -p slate-serverd`: 197 + 3 + 4 + 13 + 7 + 19 + 49 + 2 = 294
tests, all passing. Eleven new: seven in `tests/refusals.rs` over the real
binary (accepted; unknown column; unknown table; name shadows a table;
duplicate name; seven clauses refused by name, as one table-driven case; and
the `--print-schema` shape), four unit tests in `views.rs`.

Four test expectations I wrote from reading the code were wrong, and the suite
or a re-read said so:

- `--print-schema` shows `filters`, not `filter` — the SQL front end lowers a
  `WHERE` onto the list, not the singleton.
- `HAVING count(*) > 1` is refused with *"has a HAVING"*, not *"groups or
  aggregates"*: `having` is checked before `group_by`, and a grouped query with
  a HAVING trips the earlier branch.
- `SELECT *, size + 1 AS bigger` and `SELECT id, … AS n` do not parse at all.
  The select list takes a name or a call and no alias after a window, so the
  computed-column case had to be dropped and the window case had to lose its
  alias.
- `SELECT * FROM docs LIMIT 10 OFFSET 5` was the OFFSET case, and it trips the
  LIMIT branch, so it proved nothing about the branch it was named for. A bare
  `OFFSET 5` parses, and that is the case now. Caught by re-reading the table
  rather than by a failure, which is the weaker way to find it.

Mutations, via `scripts/mutate.py`: nine runs over eight distinct mutations,
all caught, each by a named test —
the table-shadowing check (`a_view_named_after_a_table_will_not_start`), the
duplicate-name check, the projection branch, the LIMIT branch, the backstop's
refusal, the base table being read from the spec rather than from the view's
name (caught by `print_schema_publishes_views_beside_the_tables`), and
`A_VIEW_MAY_SET` widened by a key, narrowed by a key, and widened again after
the test that caught it was rewritten.

That rewrite was itself a finding. The first version of
`every_allowed_key_is_one_the_named_list_also_allows` compared
`A_VIEW_MAY_SET` against a literal copy of itself, so it could only catch a
change to the constant and proved nothing about the keys a `QuerySpec`
actually serialises to. It is now
`the_allowed_keys_are_the_keys_a_full_view_serialises_to`, which builds a spec
with every clause a view may carry and compares the key set — so a serde
rename in `slate-sql` fails it, which is the *other* staleness direction: not
a clause silently accepted, but a legal view silently refused.

`sh scripts/check.sh`: 34 passed. It took two runs, and the first is worth
recording. It reported `FAILED: rust-clippy` on a tree whose clippy is clean —
disk was down to 877 MB by then, and the run before it had already spent 571 MB
on `target/debug/incremental`. `cargo clippy --workspace --all-targets` alone
exits 0 on the same tree, and after reclaiming 2.9 GB the whole script passed.
That is the `CLAUDE.md` disk note exactly: a build that dies for lack of space
reports as a lint failure. I also reported that clippy run as clean before
checking it: the command ended in `| grep … | sort | uniq -c | head` followed
by an unconditional `echo`, so the pipeline's exit code was the echo's. A
filter that prints nothing is not the same as a command that succeeded.

## What this does not do

**No read path uses a view.** That is step 2 of §3a and the point of stopping
here. A query naming a declared view gets *"no table named `recent`"*, which is
wrong in the way §3a predicted — it exists, and it is not usable *there* — and
improving that message means naming views somewhere that can afford to know
about them, which is step 2's business.

**`compute` and `window` cannot be reached through the SQL front end.** An item
in the select list is also a projection, and the projection refuses first. The
two branches are covered by unit tests against specs built directly; nothing
asserts they are reachable, because they are not.

**The backstop is tested against a clause the named list already knows about.**
`beyond_a_where` is called directly with a `sort` set, which is the closest
thing available to a field that does not exist yet. If someone adds a
`QuerySpec` field *and* a named branch for it in the same change, nothing here
proves the backstop would have caught it alone.

**No example configuration declares a view.** `config.rs` and
`docs/views.md` carry the reasoning, and `tests/refusals.rs` exercises the
parsing over the real binary, but `examples/explorer/head.toml` and the
`fixtures/` configs have no `[[views]]` section. So nothing outside that one
test file has ever started a server with a view in it, and the demo cannot
show the feature. Both wait on step 2, when there is something to show.
