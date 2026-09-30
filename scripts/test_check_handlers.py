#!/usr/bin/env python3
"""Tests for `check_handlers.py`, over files this one writes.

Run against `service.rs`, a check that had stopped checking would pass for as
long as `service.rs` stayed correct — which is the failure mode the guard
exists to prevent, one level up. Each case builds the smallest Rust-shaped file
that exhibits one rule.

The cases that must NOT fire matter as much as the ones that must: a guard
people switch off because it cries wolf is worth less than no guard.
"""

from __future__ import annotations

import contextlib
import io
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_handlers

#: One function for every entry on every roster in `check_handlers.py`, plus
#: the two spellings — a converter and a wire handler — that its never-fires
#: guards look for. A case whose fixture omits one fails on the stale-entry
#: check, or on a never-fires guard, rather than on what it is testing, so
#: every fixture below defines all of them.
#:
#: The *order* is load-bearing, because three cases below are built by cutting
#: a slice out of this text: each trimmed function must sit inside the slice
#: named for the rule it is meant to disarm, and nothing else may. See the
#: comments on `NO_VIEWS` and `NO_CONVERTER` in `run`.
PREAMBLE = """\
impl Head {
    fn authorized_table(&self, name: &str) -> Result<&TableDef, Status> {
        let table = self.table(name)?;
        Ok(table)
    }

    fn resolve_relation(&self, relation: &Relation) -> Result<Step, Status> {
        let child = self.table(&relation.table)?;
        Ok(child)
    }

    fn query_from_proto_at(query: &Query, table: &TableDef) -> Result<(), Status> {
        fingerprint::check(table, query.schema.as_ref())?;
        Ok(())
    }

    fn lowered(views: &Views, tables: &[TableDef]) -> Result<Serving, Fault> {
        let table = tables.iter().find(|t| t.name() == view.table);
        Ok(())
    }

    fn views(declared: &[config::View], tables: &[TableDef]) -> Started<Views> {
        if tables.iter().any(|t| t.name() == view.name) {
            return Err(shadowed);
        }
        Ok(())
    }

    fn authorized_read_source(&self, name: &str) -> Result<(&TableDef, Expr), Status> {
        if let Some(view) = self.views.get(name) {
            let table = self.authorized_table(&view.table)?;
            return Ok((table, view.predicate.clone()));
        }
        Ok((self.authorized_table(name)?, Expr::True))
    }

    fn serving_views(mut self, views: Views) -> Self {
        self.views = views;
        self
    }

    fn no_such_table(&self, name: &str) -> Status {
        if self.views.contains_key(name) {
            return Status::new(Code::NotFound, format!("`{name}` is a view"));
        }
        Status::new(Code::NotFound, format!("no table named `{name}`"))
    }

    fn join_from_proto(
        wire: &pb::JoinQuery,
        catalog: &Catalog,
    ) -> Result<Join, Status> {
        let table = catalog
            .table_by_name(&query.table)
            .ok_or_else(|| Status::not_found(format!("no table named `{}`", query.table)))?;
        Ok(())
    }

    fn aggregate_from_proto_query(
        wire: &pb::AggregateQuery,
        catalog: &Catalog,
    ) -> Result<Grouped, Status> {
        let table = catalog
            .table_by_name(&wire.table)
            .ok_or_else(|| Status::not_found(format!("no table named `{}`", wire.table)))?;
        Ok(())
    }

    async fn get(&self, request: Request<pb::GetRequest>) -> Result<(), Status> {
        let context = self.context(&request)?;
        Ok(())
    }
"""

CASES: list[tuple[str, str | dict[str, str] | None, int, str]] = [
    (
        "a handler that authorises before fingerprinting passes",
        """
    async fn insert(&self) -> Result<(), Status> {
        let table = self.authorized_table(&context, &r.table, Action::Insert)?;
        fingerprint::check(table, r.schema.as_ref())?;
    }
""",
        0,
        "",
    ),
    (
        "a handler using the bare resolver fails and names itself",
        """
    async fn query(&self) -> Result<(), Status> {
        let table = self.table(&wire.table)?;
    }
""",
        1,
        "`query` resolves a table",
    ),
    (
        "a fingerprint with no authorisation above it fails",
        """
    async fn insert(&self) -> Result<(), Status> {
        let table = something_else(&r.table)?;
        fingerprint::check(table, r.schema.as_ref())?;
    }
""",
        1,
        "no `authorized_table`",
    ),
    (
        # The spelling case. `check_named` is the same check under a
        # caller-supplied name, and it arrived by renaming the call in
        # `query_from_proto_at` — which made the old pattern match nothing
        # there and silently dropped rule 2's cover from the converter every
        # read goes through. The only symptom was a roster entry reported as
        # stale, which is a thin thread to hang a security rule on.
        "a check_named with no authorisation above it fails too",
        """
    async fn insert(&self) -> Result<(), Status> {
        let table = something_else(&r.table)?;
        fingerprint::check_named(table, &r.table, r.schema.as_ref())?;
    }
""",
        1,
        "no `authorized_table`",
    ),
    (
        "an authorisation too far above the fingerprint does not count",
        """
    async fn insert(&self) -> Result<(), Status> {
        let table = self.authorized_table(&context, &r.table, Action::Insert)?;
        let a = 1;
        let b = 2;
        let c = 3;
        let d = 4;
        fingerprint::check(table, r.schema.as_ref())?;
    }
""",
        1,
        "no `authorized_table`",
    ),
    (
        "a fingerprint whose table arrives already authorised passes",
        # `query_from_proto_at` is in PREAMBLE and fingerprints with no
        # `authorized_table` above it. It must pass on its exemption, or the
        # exemption list does nothing.
        """
    async fn insert(&self) -> Result<(), Status> {
        let table = self.authorized_table(&context, &r.table, Action::Insert)?;
        fingerprint::check(table, r.schema.as_ref())?;
    }
""",
        0,
        "",
    ),
    (
        "a file with no fingerprint check at all fails, rather than passing",
        # The check-that-never-fires failure: handlers move, the guard reads a
        # tree with nothing in it, and green means "found nothing wrong" and
        # "stopped looking" at the same time.
        "NO_PREAMBLE",
        1,
        "no `fingerprint::check` anywhere",
    ),
    (
        "every file in the directory is read, not only the first",
        {
            "a_first.rs": PREAMBLE + "}\n",
            "z_last.rs": """
impl Head {
    async fn query(&self) -> Result<(), Status> {
        let table = self.table(&wire.table)?;
    }
}
""",
        },
        1,
        "`query` resolves a table",
    ),
    (
        "a stale FINGERPRINT_BY_CALLER entry is reported too",
        # `resolve_relation` is here so UNAUTHORIZED stays current; the
        # by-caller exemption is the only stale one, so the finding can only
        # come from the second list.
        {
            "only.rs": """
impl Head {
    fn authorized_table(&self, name: &str) -> Result<&TableDef, Status> {
        let table = self.table(name)?;
        Ok(table)
    }

    fn resolve_relation(&self, relation: &Relation) -> Result<Step, Status> {
        let child = self.table(&relation.table)?;
        Ok(child)
    }

    async fn insert(&self) -> Result<(), Status> {
        let table = self.authorized_table(&context, &r.table, Action::Insert)?;
        fingerprint::check(table, r.schema.as_ref())?;
    }
}
""",
        },
        1,
        "FINGERPRINT_BY_CALLER lists `query_from_proto_at`",
    ),
    (
        "an authenticator missing from the roster fails",
        {
            "a.rs": PREAMBLE + "}\n",
            "auth.rs": """
const AUTHENTICATORS: [&str; 1] = ["Known"];
impl Authenticator for Known {}
impl Authenticator for Forgotten {}
""",
        },
        1,
        "`impl Authenticator for Forgotten` is not in AUTHENTICATORS",
    ),
    (
        "a roster naming an authenticator that no longer exists fails too",
        # Both directions: a stale name means a test looping over something
        # gone, which passes while covering one case fewer than it claims.
        {
            "a.rs": PREAMBLE + "}\n",
            "auth.rs": """
const AUTHENTICATORS: [&str; 2] = ["Known", "Departed"];
impl Authenticator for Known {}
""",
        },
        1,
        "AUTHENTICATORS names `Departed`",
    ),
    (
        "authenticators with no roster anywhere fails",
        {
            "a.rs": PREAMBLE + "}\n",
            "auth.rs": "impl Authenticator for Alone {}\n",
        },
        1,
        "no AUTHENTICATORS list anywhere",
    ),
    (
        "a tree with no authenticators needs no roster",
        {"a.rs": PREAMBLE + "}\n"},
        0,
        "",
    ),
    (
        "a handler converting before authorising fails",
        # Finding 8 on four RPCs, in miniature: the converter takes the wire
        # request and a catalog, so it resolves the tables the request names
        # with nothing to check them against, and the handler called it first.
        """
    async fn join(&self) -> Result<(), Status> {
        let plan = join_from_proto(&wire, self.catalog())?;
    }
""",
        1,
        "with no authorisation in the",
    ),
    (
        "a handler that authorises its inputs first passes",
        """
    async fn join(&self) -> Result<(), Status> {
        self.authorize_join_inputs(&context, &wire, Action::Read)?;
        let plan = join_from_proto(&wire, self.catalog())?;
    }
""",
        0,
        "",
    ),
    (
        "a function taking a catalog but no request is not a converter",
        # The regression this case exists for: an earlier version of the rule
        # treated *any* `catalog: &Catalog` parameter as the hazard and
        # reported 59 problems, every one of them CLI and startup code —
        # `describe`, `reconcile`, `seed::load` — where there is no request, no
        # caller and no grant to check. A rule that fires on those gets
        # switched off, and then it guards nothing.
        """
    fn describe(catalog: &Catalog) -> String {
        String::new()
    }

    async fn print_schema(&self) -> Result<(), Status> {
        let text = describe(self.catalog());
    }
""",
        0,
        "",
    ),
    (
        "a converter holding a context can check for itself",
        # The exemption is structural rather than listed: the reason these two
        # converters are a hazard is that they have nothing to authorise
        # *with*. One that takes a context does not need its callers to.
        """
    fn safe_from_proto(
        wire: &pb::JoinQuery,
        catalog: &Catalog,
        context: &SecurityContext,
    ) -> Result<Join, Status> {
        Ok(())
    }

    async fn join(&self) -> Result<(), Status> {
        let plan = safe_from_proto(&wire, self.catalog(), &context)?;
    }
""",
        0,
        "",
    ),
    (
        "one converter delegating to another is not a handler",
        # `aggregate_from_proto_query` calls `join_from_proto` for its join
        # arm. Neither has a context, so requiring an authorisation between
        # them would be unsatisfiable — the obligation belongs to whoever
        # called the outer one, which the rule already covers.
        """
    fn aggregate_from_proto_query(
        wire: &pb::AggregateQuery,
        catalog: &Catalog,
    ) -> Result<Agg, Status> {
        let inner = join_from_proto(wire, catalog)?;
        Ok(())
    }
""",
        0,
        "",
    ),
    (
        "an RPC handler that never authenticates fails",
        # `leadership` took `_request` and answered anybody who could reach the
        # port, including under a configuration whose banner promises to refuse
        # every request.
        """
    async fn leadership(&self, _request: Request<pb::LeadershipRequest>) -> Result<(), Status> {
        Ok(())
    }
""",
        1,
        "never derives a SecurityContext",
    ),
    (
        "a handler that authenticates passes, even with no table to authorise",
        # The other half: `begin`, `commit` and `leadership` name no table, so
        # rules 1 to 3 have nothing to say about them. Authentication is still
        # owed, and satisfying it must be enough.
        """
    async fn begin(&self, request: Request<pb::BeginRequest>) -> Result<(), Status> {
        let context = self.context(&request)?;
        Ok(())
    }
""",
        0,
        "",
    ),
    (
        "a helper taking no wire request is not a handler",
        # The criterion is `Request<pb::..>`, not "takes a context" or "is
        # async": an internal helper handed an already-derived context owes
        # nothing, and a rule that demanded otherwise would be unsatisfiable.
        """
    async fn read_view(&self, context: &SecurityContext) -> Result<(), Status> {
        Ok(())
    }
""",
        0,
        "",
    ),
    (
        "a handler that reads the view registry itself is reported",
        # Rule 6. The handler authenticates and never touches `self.table`, so
        # rules 1 and 5 are both satisfied — which is the point: a view
        # resolved outside `authorized_read_source` looks like an ordinary
        # authorised read to every other rule.
        """
    async fn sneaky(&self, request: Request<pb::SneakyRequest>) -> Result<(), Status> {
        let context = self.context(&request)?;
        let view = self.views.get("recent");
        Ok(())
    }
""",
        1,
        "`sneaky` reads the view registry",
    ),
    (
        "the refusal names the offending function rather than a placeholder",
        # The literal `{owner}` shipped in rule 2's message for as long as that
        # rule existed, because the interpolation sat on a continuation line
        # that was not an f-string. It cost nothing but a grep, and it is
        # exactly the kind of thing nobody notices in a message they hope never
        # to read.
        """
    async fn sneaky(&self, request: Request<pb::SneakyRequest>) -> Result<(), Status> {
        let context = self.context(&request)?;
        let view = self.views.get("recent");
        Ok(())
    }
""",
        1,
        "add `sneaky` to RESOLVES_VIEWS",
    ),
    (
        "a tree that reads the view registry nowhere fails, rather than passing",
        # The never-fires case for rule 6, on the same reasoning as rules 3 and
        # 5: `self.views` is a spelling, and renaming the field would leave the
        # rule matching nothing and printing `ok` over a tree where any handler
        # may resolve a view unauthorised.
        "NO_VIEWS",
        1,
        "so rule 6 checked nothing",
    ),
    (
        "a stale RESOLVES_VIEWS entry is reported",
        # One roster entry survives so rule 6 still fires; the other is stale,
        # so the finding can only come from the staleness check.
        {
            "only.rs": PREAMBLE.replace(
                """    fn serving_views(mut self, views: Views) -> Self {
        self.views = views;
        self
    }

""",
                "",
            )
            + "}\n",
        },
        1,
        "RESOLVES_VIEWS lists `serving_views`",
    ),
    (
        "a function building a missing-table refusal itself is reported",
        # Rule 7's subject. The refusal is the one `Head::no_such_table` exists
        # to intercept, and a fourth site building it by hand is how a declared
        # view goes back to being reported as a name the operator mistyped.
        """
    fn step_for(&self, name: &str) -> Result<Step, Status> {
        let table = self
            .pool
            .catalog()
            .table_by_name(name)
            .ok_or_else(|| Status::not_found(format!("no table named `{name}`")))?;
        Ok(table)
    }
""",
        1,
        "add `step_for` to NAMES_A_MISSING_TABLE",
    ),
    (
        "a tree that builds no missing-table refusal fails, rather than passing",
        # Rule 7's never-fires guard, and the wording is the whole exposure: the
        # rule is one literal string, so rewording the refusal — which is an
        # ordinary, harmless-looking edit — would leave it matching nothing and
        # printing `ok`. Reworded rather than deleted here, because that is the
        # edit that actually happens.
        #
        # The two converters go stale in the same run and are reported too: they
        # are on no other roster, so with the wording moved there is nothing
        # left to see them by. That is the guard's design working, not noise to
        # suppress — the assertion below names the never-fires message, which is
        # the only place this case's string comes from.
        {"a.rs": PREAMBLE.replace("no table named", "unknown table") + "}\n"},
        1,
        "so rule 7 checked nothing",
    ),
    (
        "a stale NAMES_A_MISSING_TABLE entry is reported",
        # One refusal survives so rule 7 still fires; the aggregate converter is
        # gone, so the finding can only come from the staleness check.
        {
            "only.rs": PREAMBLE[
                : PREAMBLE.index("    fn aggregate_from_proto_query(")
            ]
            + PREAMBLE[PREAMBLE.index("    async fn get(") :]
            + "}\n",
        },
        1,
        "NAMES_A_MISSING_TABLE lists `aggregate_from_proto_query`",
    ),
    (
        "a lookup resolving by `name()` is reported",
        # Rule 8's subject. `Catalog::table_by_name` being the only way a name
        # becomes a `TableDef` is what makes a view kept out of the catalog
        # refused everywhere without a line written; this is the shape that
        # quietly ends that.
        """
    fn text_index(&self, name: &str) -> Option<&IndexDef> {
        self.indexes.iter().find(|i| i.name() == name)
    }
""",
        1,
        "add `text_index` to FINDS_BY_NAME",
    ),
    (
        "an `any` that resolves by `name()` is reported too",
        # The `any` arm of rule 8's pattern, which has no case of its own
        # otherwise: an `any` is a resolution whose answer happens to be a bool,
        # and one edit away from a `find`. Dropping `any` from the pattern would
        # be invisible here without this.
        """
    fn shadows(&self, name: &str) -> bool {
        self.tables.iter().any(|t| t.name() == name)
    }
""",
        1,
        "add `shadows` to FINDS_BY_NAME",
    ),
    (
        "a tree that resolves nothing by `name()` fails, rather than passing",
        # Rule 8's never-fires guard. Rewritten into a shape the pattern does
        # not see — a helper rather than a method call — which is both the way
        # this rule stops firing and, per its own comment, the shape a fourth
        # lookup would be written in.
        {"a.rs": PREAMBLE.replace("|t| t.name()", "|t| name_of(t)") + "}\n"},
        1,
        "so rule 8 checked nothing",
    ),
    (
        "a stale FINDS_BY_NAME entry is reported",
        # `lowered` keeps rule 8 firing; `views` is gone, so the finding can
        # only come from the staleness check.
        {
            "only.rs": PREAMBLE.replace(
                """    fn views(declared: &[config::View], tables: &[TableDef]) -> Started<Views> {
        if tables.iter().any(|t| t.name() == view.name) {
            return Err(shadowed);
        }
        Ok(())
    }

""",
                "",
            )
            + "}\n",
        },
        1,
        "FINDS_BY_NAME lists `views`",
    ),
    (
        "a tree with no wire handler at all fails, rather than passing",
        # The never-fires case for rule 5: `Request<pb::` is a spelling, and a
        # crate that aliased the generated module would leave it matching
        # nothing and printing ok.
        "NO_HANDLER",
        1,
        "so rule 5 checked nothing",
    ),
    (
        "a tree with no converter at all fails, rather than passing",
        # The never-fires failure for rule 3, which is the rule most able to
        # stop matching quietly: a converter is recognised by three conditions
        # on a signature, not by one literal string.
        "NO_CONVERTER",
        1,
        "so rule 3 checked nothing",
    ),
    (
        "an exemption for a function that no longer exists fails",
        # `resolve_relation` is in UNAUTHORIZED but this fixture drops its
        # bare call, so the entry is stale.
        None,
        1,
        "UNAUTHORIZED lists `resolve_relation`",
    ),
]


#: The smallest workspace rule 9 can be satisfied by: a manifest with one
#: member whose `src/` holds nothing this guard is about.
#:
#: Every case needs one, because rule 9 reads a `Cargo.toml` rather than the
#: files it is handed, and its never-fires half fails a run whose manifest
#: yields no members. Without it, `main`'s `root` would keep defaulting to this
#: repository while every other rule read a fixture — which is exactly what
#: happened on the first run: nine cases failed reporting
#: `crates/slate-server/src/auth.rs`, a file in *this* tree, against a
#: `SOURCES` the fixture had replaced. `check_cost_prose.py`'s `main` carries
#: the same note about `docs` and `readmes`, and this is the third time.
#: The smallest `impl Catalog` rule 10 can be satisfied by: the four public
#: methods that hand out a `TableDef` today, and two that take one and hand
#: back something else.
#:
#: `insert` and `from_tables` are here to be *ignored*. They mention `TableDef`
#: in their signatures and the rule must not roster them, which is the whole
#: reason it splits on the arrow; a fixture with only the four would pass a
#: rule keyed on the whole signature just as happily.
CATALOG = """\
pub struct Catalog {
    tables: Vec<TableDef>,
}

impl Catalog {
    pub const fn new() -> Self {
        Self { tables: Vec::new() }
    }

    pub fn from_tables<I: IntoIterator<Item = TableDef>>(tables: I) -> Result<Self> {
        Ok(Self::new())
    }

    pub fn insert(&mut self, table: TableDef) -> Result<()> {
        Ok(())
    }

    pub fn table(&self, id: TableId) -> Option<&TableDef> {
        self.tables.iter().find(|t| t.id() == id)
    }

    pub fn table_by_name(&self, name: &str) -> Option<&TableDef> {
        self.tables.iter().find(|t| t.name() == name)
    }

    pub fn tables(&self) -> &[TableDef] {
        &self.tables
    }

    pub fn referencing(&self, parent: TableId) -> Vec<(&TableDef, &ForeignKeyDef)> {
        Vec::new()
    }
}
"""


#: The other `name()` comparison rule 8's widened half must find: a lookup
#: that is not a table, in the crate that holds the anchor.
#:
#: Written into every fixture rather than into the cases that need it,
#: because `FINDS_BY_NAME_OUTSIDE` is checked in both directions — an entry
#: whose subject is absent is reported, so a fixture missing this file fails
#: every case for a reason none of them is about.
TABLE_RS = """\
impl TableBuilder {
    pub fn build(self) -> Result<TableDef> {
        if self.checks.iter().any(|c| c.name() == check.name()) {
            return Err(SchemaError::DuplicateCheck);
        }
        Ok(TableDef {})
    }
}
"""


def workspace(
    root: pathlib.Path, catalog: str | dict[str, str] | None = CATALOG
) -> None:
    # `crates/slate-schema` is a member because rule 8's widened half walks
    # the manifest, exactly as rule 9 does, and skips what `SOURCES` already
    # covers. A fixture that listed only `crates/quiet` would leave that half
    # reading nothing while the real repository read three files.
    (root / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["crates/quiet", "crates/slate-schema"]\n'
    )
    src = root / "crates" / "quiet" / "src"
    src.mkdir(parents=True)
    (src / "lib.rs").write_text("pub fn nothing() {}\n")
    # Every case needs one for the same reason every case needs a manifest:
    # rule 10 reads a fixed path under `root` rather than the files it is
    # handed, and its never-fires half fails a run that finds no catalog. On
    # the first run without this, twelve cases failed naming a temporary
    # directory — the same shape as the nine that failed when rule 9 arrived,
    # and the fourth time in this file's history.
    #
    # A dict is several files in the crate's `src/`, which is the only way to
    # write a case about rule 10 reading a tree rather than one file.
    if catalog is not None:
        schema = root / "crates" / "slate-schema" / "src"
        schema.mkdir(parents=True)
        files = {"catalog.rs": catalog} if isinstance(catalog, str) else catalog
        for name, text in files.items():
            (schema / name).write_text(text)
        # Unless a case wrote its own, which is how the both-directions
        # case takes the lookup away without taking the file away.
        if "table.rs" not in files:
            (schema / "table.rs").write_text(TABLE_RS)


def run(
    body: str | dict[str, str] | None,
    crates: dict[str, str] | None = None,
    inside_crate: str | None = None,
    catalog: str | dict[str, str] | None = CATALOG,
) -> tuple[int, str]:
    """`crates` adds `crates/<name>/src/lib.rs` files and lists them as members,
    which is the only way to write a case about rule 9.

    `inside_crate` puts the service files in that crate's `src/` instead of in
    a directory of their own, so the member *is* the scanned tree. Without it
    no case reaches rule 9's skip-what-is-already-covered branch, and a
    mutation deleting that branch survived — the fixture's source directory was
    never a listed member."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        workspace(root, catalog)
        if crates:
            listed = ['"crates/quiet"', '"crates/slate-schema"'] + [
                f'"crates/{name}"' for name in crates
            ]
            (root / "Cargo.toml").write_text(
                "[workspace]\nmembers = [" + ", ".join(listed) + "]\n"
            )
            for name, text in crates.items():
                src = root / "crates" / name / "src"
                src.mkdir(parents=True)
                (src / "lib.rs").write_text(text)
        # The service files go in a directory of their own, so rule 9's
        # fixture crates are not also walked by the other eight — unless a case
        # is about a crate that *is* the scanned tree.
        if inside_crate is None:
            inside = root / "svc"
            inside.mkdir()
        else:
            inside = root / "crates" / inside_crate / "src"
        directory = str(inside)
        path = inside / "service.rs"
        if isinstance(body, dict):
            # Several files, handed over as a *directory*. Every other case
            # passes one file, which never exercises the walk — and the walk is
            # the whole point of scanning a tree rather than the one file the
            # first version read.
            for name, text in body.items():
                (inside / name).write_text(text)
            out = io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
                code = check_handlers.main([directory], root=root)
            return code, out.getvalue()
        if body == "NO_HANDLER":
            path.write_text(
                PREAMBLE[: PREAMBLE.index("    async fn get(")] + "}\n"
            )
        elif body == "NO_CONVERTER":
            # Only the converter is trimmed: dropping everything after it would
            # take the handler too, and the case would fail on rule 5 rather
            # than on the rule it is named for.
            converter = PREAMBLE[
                PREAMBLE.index("    fn join_from_proto(") : PREAMBLE.index(
                    "    async fn get("
                )
            ]
            path.write_text(PREAMBLE.replace(converter, "") + "}\n")
        elif body == "NO_VIEWS":
            # Only the three functions that read `self.views` are trimmed, so
            # the case fails on rule 6 rather than on a rule it is not named
            # for. `lowered` and `views` are rule 8's subject and read no
            # registry, which is why they sit above this slice rather than
            # among the view functions they live beside in the real tree.
            views = PREAMBLE[
                PREAMBLE.index("    fn authorized_read_source(") : PREAMBLE.index(
                    "    fn join_from_proto("
                )
            ]
            path.write_text(PREAMBLE.replace(views, "") + "}\n")
        elif body == "NO_PREAMBLE":
            # Resolutions and exemptions present, no fingerprint anywhere.
            path.write_text(
                PREAMBLE.replace(
                    "        fingerprint::check(table, query.schema.as_ref())?;\n", ""
                )
                + "}\n"
            )
        elif body is None:
            path.write_text(
                "impl Head {\n"
                "    fn authorized_table(&self, name: &str) -> Result<(), Status> {\n"
                "        let table = self.table(name)?;\n"
                "        Ok(())\n"
                "    }\n}\n"
            )
        else:
            path.write_text(PREAMBLE + body + "}\n")
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = check_handlers.main([str(path)], root=root)
        return code, out.getvalue()


#: Rule 9's cases: a crate outside `SOURCES` carrying what these rules guard.
#:
#: name, the extra crates, the exit code, and the text the report must carry.
#: The service itself is always the passing one, so a failure here is rule 9's
#: and not another rule's.
SCOPE_CASES: list[tuple[str, dict[str, str], int, str]] = [
    (
        # The caveat: "a converter in another crate reached from a handler is
        # outside SOURCES", and the one under it: the never-fires halves fire
        # on "nothing found anywhere", never on "a tree nobody listed".
        "a crate outside SOURCES that fingerprints is reported",
        {"rogue": "fn f() {\n    fingerprint::check(table, schema)?;\n}\n"},
        1,
        "crates/rogue/src/lib.rs contains `fingerprint::check`",
    ),
    (
        "a crate outside SOURCES with a fourth Authenticator is reported",
        {"rogue": "impl Authenticator for Mine {}\n"},
        1,
        "contains `impl Authenticator for`",
    ),
    (
        "a crate outside SOURCES serving the wire is reported",
        {"rogue": "async fn get(&self, r: Request<pb::GetRequest>) {}\n"},
        1,
        "contains `Request<pb::`",
    ),
    (
        "and it says which line to add to SOURCES",
        {"rogue": "fn f() {\n    fingerprint::check(table, schema)?;\n}\n"},
        1,
        "Add `crates/rogue/src` to SOURCES",
    ),
    (
        "a crate outside SOURCES carrying none of them is clean",
        {"rogue": "pub fn add(a: u8, b: u8) -> u8 {\n    a + b\n}\n"},
        0,
        "",
    ),
    (
        # The other half of the rule, and the one that has to hold for the
        # whole of `SOURCES`: a crate that is scanned carries every marker
        # there is and must be silent. A mutation deleting the
        # already-covered branch survived until this case existed, because no
        # fixture's source directory was also a listed member.
        "a crate that IS the scanned tree is not reported against itself",
        {},
        0,
        "",
    ),
    (
        # Measured, not assumed: with `self.table(` in MARKERS this rule
        # reported three of this repository's crates and all three were false.
        # `self.table` names a method on whatever `self` is, and rule 1 is
        # about a `self` that is a gRPC service.
        "a bare `self.table(` in another crate is not this guard's business",
        {"parser": "fn inner(&self) {\n    let t = self.table()?;\n}\n"},
        0,
        "",
    ),
]


#: Rule 10's cases: what `crates/slate-schema/src/catalog.rs` may hand out.
#:
#: name, the catalog source (`None` for no file at all), the exit code, and
#: the text the report must carry. The service is always the passing fixture,
#: so a failure here is rule 10's and no other rule's.
CATALOG_CASES: list[tuple[str, str | dict[str, str] | None, int, str]] = [
    (
        "the four accessors that exist today are rostered",
        CATALOG,
        0,
        "",
    ),
    (
        # The obvious second resolver, and the one a parameter-keyed rule
        # would also catch.
        "a second lookup taking a name is reported",
        CATALOG.replace(
            "    pub fn tables(",
            "    pub fn matching(&self, prefix: &str) -> Vec<&TableDef> {\n"
            "        Vec::new()\n"
            "    }\n\n    pub fn tables(",
        ),
        1,
        "`Catalog::matching` is public and hands out a `TableDef`",
    ),
    (
        # The case the rule is keyed on the *return* type for. This method
        # takes no name at all and is a name-to-table lookup for every caller
        # that holds the map; a rule looking for `&str` in the parameters
        # passes it in silence.
        "a lookup that takes no name and returns the whole mapping is reported",
        CATALOG.replace(
            "    pub fn tables(",
            "    pub fn by_name(&self) -> HashMap<&str, &TableDef> {\n"
            "        HashMap::new()\n"
            "    }\n\n    pub fn tables(",
        ),
        1,
        "`Catalog::by_name` is public and hands out a `TableDef`",
    ),
    (
        # A second inherent `impl` block is legal Rust and is where an
        # accessor added to a long file lands. The first version of the loop
        # stopped at the first closing brace and never saw this.
        "a lookup in a second `impl Catalog` block is reported",
        CATALOG + "\nimpl Catalog {\n"
        "    pub fn later(&self, name: &str) -> Option<&TableDef> {\n"
        "        None\n"
        "    }\n}\n",
        1,
        "`Catalog::later` is public and hands out a `TableDef`",
    ),
    (
        # The reason the rule splits on the arrow: both of these mention
        # `TableDef` and neither hands one back. They are in `CATALOG`
        # already, so this case is the assertion that the passing fixture is
        # passing for the right reason rather than for want of a subject.
        "a method that takes a `TableDef` and returns none is not rostered",
        CATALOG.replace(
            "    pub fn table(",
            "    pub fn absorb(&mut self, other: Vec<TableDef>) -> Result<()> {\n"
            "        Ok(())\n"
            "    }\n\n    pub fn table(",
        ),
        0,
        "",
    ),
    (
        "a private lookup is not the public surface",
        CATALOG.replace(
            "    pub fn tables(",
            "    fn hidden(&self, name: &str) -> Option<&TableDef> {\n"
            "        None\n"
            "    }\n\n    pub fn tables(",
        ),
        0,
        "",
    ),
    (
        # A wrapped signature. The accumulate-to-the-brace loop exists for
        # this, and without it the rule reads `pub fn wrapped(` — which has
        # no arrow — and misses the method entirely.
        "a signature wrapped across lines is still read",
        CATALOG.replace(
            "    pub fn tables(",
            "    pub fn wrapped(\n"
            "        &self,\n"
            "        name: &str,\n"
            "    ) -> Option<&TableDef> {\n"
            "        None\n"
            "    }\n\n    pub fn tables(",
        ),
        1,
        "`Catalog::wrapped` is public and hands out a `TableDef`",
    ),
    (
        # The crate is read as a tree, not as one file. `catalog.rs` is where
        # the accessors are today and nothing makes them stay there; the first
        # version of this rule named the file and recorded the gap as a limit
        # rather than closing it.
        "a lookup in another file of the same crate is reported",
        {
            "catalog.rs": CATALOG,
            "views.rs": "impl Catalog {\n"
            "    pub fn view_base(&self, name: &str) -> Option<&TableDef> {\n"
            "        None\n"
            "    }\n}\n",
        },
        1,
        "`Catalog::view_base` is public and hands out a `TableDef`",
    ),
    (
        # The other half: a file in the crate that is about something else
        # must not be read as though it were the catalog.
        "another file with no `impl Catalog` changes nothing",
        {
            "catalog.rs": CATALOG,
            "value.rs": "impl Value {\n"
            "    pub fn only(&self, name: &str) -> Option<&TableDef> {\n"
            "        None\n"
            "    }\n}\n",
        },
        0,
        "",
    ),
    (
        # An `impl Catalog` inside a `mod` is legal and was invisible to the
        # column-zero match the first version used. The `}` that ends it is
        # the one at the `impl`'s own indentation, not the first one in the
        # file — which is what the `mod`'s trailing brace is here to check.
        "a lookup in an indented `impl Catalog` is reported",
        CATALOG + "\nmod extra {\n"
        "    impl Catalog {\n"
        "        pub fn nested(&self, name: &str) -> Option<&TableDef> {\n"
        "            None\n"
        "        }\n"
        "    }\n}\n",
        1,
        "`Catalog::nested` is public and hands out a `TableDef`",
    ),
    (
        # And the block still *ends*: a method after the indented `impl`
        # closes is outside it and must not be read as one of its own.
        "a method after an indented block closes is not inside it",
        CATALOG + "\nmod extra {\n"
        "    impl Catalog {\n"
        "        pub fn nested(&self, name: &str) -> Option<&Table> {\n"
        "            None\n"
        "        }\n"
        "    }\n\n"
        "    pub fn loose(c: &Catalog) -> Option<&TableDef> {\n"
        "        None\n"
        "    }\n}\n",
        0,
        "",
    ),
    (
        # Rule 8's widened half. The shape rule 10 cannot see: the slice from
        # `tables()` walked by name, in a crate that is neither
        # `slate-server` nor `slate-serverd`.
        "a name lookup in another workspace crate is reported",
        {
            "catalog.rs": CATALOG,
            "loader.rs": "impl Loader {\n"
            "    pub fn resolve(&self, catalog: &Catalog, n: &str) -> bool {\n"
            "        catalog.tables().iter().any(|t| t.name() == n)\n"
            "    }\n}\n",
        },
        1,
        "`resolve` resolves something by comparing `name()`",
    ),
    (
        # And it names the key a person would add, path and function
        # together — not the bare `resolve`, which would exempt every
        # `resolve` in thirteen crates.
        "and it says which key to add, with the path in it",
        {
            "catalog.rs": CATALOG,
            "loader.rs": "impl Loader {\n"
            "    pub fn resolve(&self, catalog: &Catalog, n: &str) -> bool {\n"
            "        catalog.tables().iter().any(|t| t.name() == n)\n"
            "    }\n}\n",
        },
        1,
        "crates/slate-schema/src/loader.rs::resolve` to FINDS_BY_NAME_OUTSIDE",
    ),
    (
        # The roster is checked both ways, and keyed on path *and* function:
        # a `build` elsewhere does not satisfy the entry for this one.
        "a roster key whose lookup is gone is reported",
        {
            "catalog.rs": CATALOG,
            "table.rs": "impl TableBuilder {\n"
            "    pub fn build(self) -> Result<TableDef> {\n"
            "        Ok(TableDef {})\n"
            "    }\n}\n",
        },
        1,
        "FINDS_BY_NAME_OUTSIDE lists `crates/slate-schema/src/table.rs::build`",
    ),
    (
        "no name lookup outside SOURCES at all fails, not passes",
        None,
        1,
        "rule 8's widened half checked nothing",
    ),
    (
        "a roster entry for a method that is gone is reported",
        CATALOG.replace(
            "    pub fn referencing(&self, parent: TableId) -> Vec<(&TableDef, &ForeignKeyDef)> {\n"
            "        Vec::new()\n"
            "    }\n",
            "",
        ),
        1,
        "HANDS_OUT_A_TABLE lists `referencing`",
    ),
    (
        # The anchor's own never-fires half, separate from the one below: the
        # file parsed and methods were found, and the sentence rule 10
        # defends had stopped being about anything.
        "a renamed `table_by_name` fails rather than passing",
        CATALOG.replace("pub fn table_by_name(", "pub fn lookup(").replace(
            "    pub fn table(&self, id: TableId) -> Option<&TableDef> {",
            "    pub fn table(&self, id: TableId) -> Option<&TableDef> {",
        ),
        1,
        "is not among the",
    ),
    (
        "a catalog with no public accessor at all fails, not passes",
        "pub struct Catalog {\n    tables: Vec<TableDef>,\n}\n"
        "\nimpl Catalog {\n    pub const fn new() -> Self {\n"
        "        Self { tables: Vec::new() }\n    }\n}\n",
        1,
        "so rule 10 checked nothing",
    ),
    (
        "no catalog file at all fails, not passes",
        None,
        1,
        "so rule 10 checked nothing",
    ),
]


def main() -> int:
    failed = 0
    for name, body, expected, wanted in CASES:
        code, said = run(body)
        ok = code == expected and (not wanted or wanted in said)
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected exit {expected} and {wanted!r}, got {code}")
            for line in said.splitlines():
                print(f"      {line}")
    for name, crates, expected, wanted in SCOPE_CASES:
        # The one case whose service lives inside a listed member, named by
        # what it is about rather than by a fifth column nothing else uses.
        served = "served" if "IS the scanned tree" in name else None
        if served:
            crates = {served: "pub fn nothing() {}\n"}
        code, said = run("", crates=crates, inside_crate=served)
        ok = code == expected and (not wanted or wanted in said)
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected exit {expected} and {wanted!r}, got {code}")
            for line in said.splitlines():
                print(f"      {line}")

    for name, catalog, expected, wanted in CATALOG_CASES:
        code, said = run("", catalog=catalog)
        ok = code == expected and (not wanted or wanted in said)
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected exit {expected} and {wanted!r}, got {code}")
            for line in said.splitlines():
                print(f"      {line}")

    # Rule 9's never-fires half, which needs a manifest the helper above will
    # not write: an empty `members` leaves the rule comparing SOURCES against
    # nothing and reporting a crate it never looked for.
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        workspace(root)
        (root / "Cargo.toml").write_text("[workspace]\nmembers = []\n")
        inside = root / "svc"
        inside.mkdir()
        (inside / "service.rs").write_text(PREAMBLE + "}\n")
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = check_handlers.main([str(inside / "service.rs")], root=root)
    said = out.getvalue()
    ok = code == 1 and "no workspace members parsed" in said
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a workspace with no members fails, not passes")
    if not ok:
        print(f"        exit {code}: {said}")

    total = len(CASES) + len(SCOPE_CASES) + len(CATALOG_CASES) + 1
    print(f"\n{total - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
