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

#: The exemptions the real sources need, plus one converter and one view
#: resolution. A case whose fixture omits one fails on the stale-entry check —
#: or, for the converter and the view registry, on the never-fires check —
#: rather than on what it is testing, so every fixture below defines all of
#: them.
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

    fn join_from_proto(
        wire: &pb::JoinQuery,
        catalog: &Catalog,
    ) -> Result<Join, Status> {
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


def run(body: str | dict[str, str] | None) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as directory:
        path = pathlib.Path(directory) / "service.rs"
        if isinstance(body, dict):
            # Several files, handed over as a *directory*. Every other case
            # passes one file, which never exercises the walk — and the walk is
            # the whole point of scanning a tree rather than the one file the
            # first version read.
            for name, text in body.items():
                (pathlib.Path(directory) / name).write_text(text)
            out = io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
                code = check_handlers.main([directory])
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
            # Only the two view functions are trimmed, so the case fails on
            # rule 6 rather than on a rule it is not named for.
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
            code = check_handlers.main([str(path)])
        return code, out.getvalue()


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
    print(f"\n{len(CASES) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
