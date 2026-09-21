#!/usr/bin/env python3
"""A handler must authorise before it touches a table's shape.

Security finding 8 of `docs/security-review.md` was that a handler resolved its
table and *used* it — fingerprinting it, converting a request against it —
before anything checked the caller's grant, so a caller with no grant read the
schema out of the refusals. It was fixed twice, months apart, because the first
fix covered the handlers the finding was written about and the read paths went
on doing it. `query` gave up a table's exact column count, in one request, to a
role granted nothing.

That is the third time in one session a fix covered one path of several — the
cross-tenant catalog refusal covered one constructor of two, and the duplicate
identity header one `Authenticator` of two. Each time the remaining paths
"looked the same" from reading. This is the check that makes the next one fail
loudly instead:

1. **Every bare `self.table(..)`** — the resolver that does *not* authorise —
   is either inside `authorized_table` or listed in `UNAUTHORIZED` with a
   reason it is safe.
2. **Every `fingerprint::check(..)`** has an `authorized_table` above it in the
   same function, within `REACH` lines. The fingerprint is the original
   channel and the ordering is the whole fix.
3. **Every call to a converter that resolves a request's tables** — one taking
   a `&pb::` request and a `&Catalog` and no `SecurityContext`, so it has
   nothing to check them against — has an authorisation above it. This is the
   rule that would have caught finding 8 on `join`, `explain_join`,
   `aggregate` and `explain_aggregate`, which rules 1 and 2 could not see
   because the resolution happens a function away.
4. **Every `impl Authenticator for T`** is named in the `AUTHENTICATORS`
   roster, and every name in the roster still implements it. Finding 6 was
   fixed in one of two implementations; the roster is what makes a third
   arrive with a failing check.
5. **Every RPC handler authenticates.** A method taking a `Request<pb::..>` is
   reachable from the wire by definition, and must derive a `SecurityContext`
   before doing anything. `leadership` did not — it took `_request` and
   answered anybody who could reach the port, including under
   `mode = "deny-all"`, whose banner promises to "refuse every request".

6. **Every read of the view registry** — `self.views` — is inside a function
   listed in `RESOLVES_VIEWS`. A view is a name the catalog does not hold, so
   `Catalog::table_by_name` refuses one everywhere by finding nothing; that
   is `docs/views.md` §3a, and it is a property of *nothing looking in the
   other map*. A handler that read `self.views` itself would have a
   `&TableDef` for a base table the caller was never authorised against, and
   rules 1 and 3 would both see an ordinary authorised resolution. This is
   the rule that makes a second view-resolving path a failure rather than a
   silence, and it is the same roster idiom as the rest.

None is a proof. A handler can hold a `&TableDef` from one of the accounted
sites and pass it along, and this will not see it. What they do is make adding
a *new* unauthorised resolution a failure rather than a silence, which is the
`EXPECTED_REFUSALS` idiom this repository already uses in three places: a list
you are forced to edit is a list that stays true.

Run directly: `python3 scripts/check_handlers.py`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Where the handlers live. Directories rather than one file, because the
#: first version of this check read `service.rs` alone and missed a
#: `fingerprint::check` in `convert.rs` — a file away, reachable only through
#: callers, and invisible to a check whose whole subject is "is this reachable
#: without authorising". That is the caveat this check's own entry named,
#: biting within the hour.
SOURCES = (
    ROOT / "crates" / "slate-server" / "src",
    ROOT / "crates" / "slate-serverd" / "src",
)

#: How far above a `fingerprint::check` its authorisation may sit.
#:
#: Four, because in every current site they are adjacent or one line apart, and
#: a check that allowed twenty would accept a handler that authorised one table
#: and fingerprinted another. If a legitimate site ever needs more, widening
#: this is a deliberate edit — which is the point.
REACH = 4

#: Bare `self.table(..)` calls that are *not* a disclosure, and why.
#:
#: Keyed on the enclosing function. The reason is the entry's whole value: a
#: reader deciding whether their new call belongs here needs to know what makes
#: these safe, not that somebody once said so.
#: `fingerprint::check` calls whose table was authorised by their *caller*, and
#: why that is safe rather than a hole.
#:
#: A function taking a `&TableDef` cannot authorise: it has no name to resolve
#: and no context to resolve it for. The check it owes is on its callers, and
#: those are the sites rule 2 covers. Listed rather than skipped by shape,
#: because "takes a table so somebody else checked" is exactly the reasoning
#: that was wrong three times today, and writing it down forces it to be
#: re-argued when a caller is added.
FINGERPRINT_BY_CALLER = {
    "query_from_proto_at": (
        "takes an already-resolved `&TableDef`, so it cannot authorise: no "
        "name to resolve, no context to resolve it for. Its callers are "
        "`query` and `explain` directly, and `join`, `explain_join`, "
        "`aggregate` and `explain_aggregate` through `join_from_proto` and "
        "`aggregate_from_proto_query` — all six authorise before converting. "
        "**The first version of this entry said so when four of the six did "
        "not**, which is the whole reason the reason is written down: it was "
        "a claim from reading, it was wrong, and a caller with no grant read "
        "a table's column count off `join` until it was checked. **Rule 3 now "
        "holds the four indirect callers to it**, so this half of the reason "
        "is a check rather than a claim; the two direct ones are rule 2's"
    ),
}

UNAUTHORIZED = {
    "authorized_table": (
        "this is the authorising resolver itself; it calls the bare one and "
        "then authorises"
    ),
    "resolve_relation": (
        "reached only from `related`, which authorises `relation.table` "
        "immediately before calling it — see the comment at that call site. "
        "Resolving discloses the child's foreign keys, so the grant is checked "
        "before, not here, because this function does not have the context"
    ),
}

#: Where the list of every `Authenticator` lives, and the pattern that finds
#: the implementations it must account for.
#:
#: Finding 6 — a repeated identity header resolved rather than refused — was
#: fixed in one of the two implementations, and the other went on doing it
#: because the trait says nothing about duplicated keys and nothing looked.
#: Both are right now; this is what makes a *third* arrive with a failing check
#: instead of a hole. The list is a Rust `const` rather than a copy here, so
#: the check and the test that uses it cannot drift apart.
#:
#: The list is *found* among the files being scanned rather than read from a
#: second hard-coded path. A path here would be a thing that can go stale
#: silently — which is what the never-fires check below exists to stop, and
#: what moving `SOURCES` from one file to two directories fixed an hour ago.
#: It also lets the tests run this rule over a tree they wrote.
ROSTER_LIST = re.compile(r"const AUTHENTICATORS: \[&str; \d+\] = \[([^\]]*)\]")
IMPLEMENTS = re.compile(r"^impl Authenticator for (\w+)", re.MULTILINE)

#: A converter that resolves tables *itself*, found by its signature.
#:
#: `join_from_proto` and `aggregate_from_proto_query` take a `Catalog` and no
#: `SecurityContext`, so they look tables up with no idea who is asking — and
#: the four handlers calling them converted first. That is finding 8 on four
#: RPCs, and this check could not see it: neither function calls
#: `self.table(..)` nor `fingerprint::check` directly, which is the gap this
#: file's own entry named.
#:
#: The names are *derived* rather than listed, by matching a whole signature.
#: A list would go stale the moment a third converter is written, which is the
#: failure mode of every hard-coded thing in this script so far. No call graph
#: is needed: find the names in one pass, then require every call to them to be
#: authorised in the next.
#:
#: **Three parameters, and the third is the one that took two wrong attempts.**
#: A first version matched a bare `catalog: &Catalog,` line and attributed it to
#: the nearest `fn` above, which picked up struct fields: it derived `run`,
#: `quantile` and `summary` as converters and flagged **47** ordinary calls to
#: things with those very common names. Matching the signature instead dropped
#: that to **59** — worse, and the interesting part is *why*. Nothing was
#: mis-parsed. `reconcile`, `render_plan`, `describe`, `security::catalog`,
#: `seed::load`, `analyze` and `store_for` all genuinely take a `&Catalog`.
#: Taking a catalog is simply not the hazard: those are CLI and startup code,
#: where there is no request, no caller and no grant to check, and demanding an
#: `authorized_table` above each call would be demanding nonsense.
#:
#: The hazard is narrower than "resolves a table". It is **resolving a table
#: whose name came off the wire, with nothing to check it against** — so the
#: signature must show both halves: a `&pb::` request type *and* a `&Catalog`,
#: and no `SecurityContext` that would let it check for itself. That is the
#: shape of finding 8 stated exactly, rather than a proxy for it that happens
#: to fit two functions.
TAKES_CATALOG = re.compile(r"fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)", re.S)
#: What a request's table names arrive in, and what would let a converter
#: authorise them itself.
WIRE = "&pb::"
CONTEXT = "SecurityContext"
CATALOG = "catalog: &Catalog"

#: A method reachable from the wire, found by its signature.
#:
#: `Request<pb::..>` is not a proxy for "is an RPC handler" — it is what being
#: one consists of, which is the property the converter rule above took two
#: wrong criteria to find. Nothing else in either crate takes one, and every
#: tonic service method does.
#:
#: The obligation is authentication, not authorisation: rules 1 to 3 cover the
#: grant on a named table, and a handler naming no table (`begin`, `commit`,
#: `leadership`) still has to establish *who is asking* before it answers.
TAKES_REQUEST = re.compile(r"fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)", re.S)
WIRE_REQUEST = "Request<pb::"
AUTHENTICATES = re.compile(r"self\.context\(")

FUNCTION = re.compile(r"^\s*(?:pub(?:\(crate\))?\s+)?(?:async\s+)?fn\s+([a-z_][a-z0-9_]*)")
BARE = re.compile(r"self\.table\(")
FINGERPRINT = re.compile(r"fingerprint::check\(")
AUTHORIZED = re.compile(r"authorized_table\(")
#: The multi-table helpers, which authorise a whole request's inputs at once.
AUTHORIZES = re.compile(r"authorize_\w+\(")
#: Reading the view registry, which resolves a name the catalog does not hold.
VIEWS = re.compile(r"self\.views")

#: The functions allowed to read the view registry at all.
#:
#: Exactly one of them turns a view's name into its base table, and that
#: narrowness is the feature: `docs/views.md` §3a's build order is "opt *one*
#: read path in, deliberately", and a roster with one resolver on it is what
#: makes the second arrive as a diff somebody has to justify rather than as a
#: line nobody notices. Widening it is a decision about which handlers may read
#: through a view, and §4 already says writes may not.
#:
#: The other two are here because the rule is written on the *read*, not on
#: what the read is for — a rule that tried to tell "resolving" from "merely
#: looking" would be guessing at intent from a regex. Each entry's reason says
#: which it is, and for both non-resolvers the return type settles it: a
#: `Self` and a `Status` are not a `&TableDef`.
RESOLVES_VIEWS = {
    "authorized_read_source": (
        "the opt-in resolver itself; it authorises the base table through "
        "`authorized_table` with the action the kernel will check, so RLS and "
        "the grant both see the base table — `docs/views.md` §1"
    ),
    "serving_views": "the constructor that installs the registry; it resolves nothing",
    "no_such_table": (
        "builds the refusal for a name the catalog has no table for, and reads "
        "the registry only to say a view is a view rather than a typo. Its "
        "return type is `Status`: it cannot hand a caller a `TableDef`, so "
        "widening it is not a way to reach a view"
    ),
}


def enclosing_functions(lines: list[str]) -> list[str]:
    """The innermost `fn` name in scope at each line.

    Indentation rather than brace counting: a nested `fn` inside a handler —
    `resolve_relation` has one — is the name a reader would give that line, and
    both are found by taking the most recent `fn` above it.
    """
    names: list[str] = []
    current = "<file>"
    for line in lines:
        found = FUNCTION.match(line)
        if found:
            current = found.group(1)
        names.append(current)
    return names


def catalog_converters(files: list[Path]) -> set[str]:
    """Functions that resolve a request's tables with nothing to check them by.

    All three conditions matter and the third is the one that makes this a
    rule rather than a nuisance — see `TAKES_CATALOG` above for the two
    attempts that got it wrong and what the second one cost.
    """
    names: set[str] = set()
    for path in files:
        for found in TAKES_CATALOG.finditer(path.read_text()):
            params = found.group(2)
            if CATALOG in params and WIRE in params and CONTEXT not in params:
                names.add(found.group(1))
    return names


def unauthorised_conversions(files: list[Path], converters: set[str]) -> list[str]:
    """Every call to such a converter has an authorisation above it.

    Same `REACH` window as the fingerprint rule, and the same limitation: this
    sees adjacency, not that the authorisation covers the tables the
    conversion will resolve. What it stops is the shape that actually
    happened — a handler calling the converter with nothing above it at all.
    """
    calling = re.compile(r"\b(" + "|".join(sorted(converters)) + r")\(")

    problems = []
    for path in files:
        lines = path.read_text().splitlines()
        owners = enclosing_functions(lines)
        for at, line in enumerate(lines):
            found = calling.search(line)
            if found is None or owners[at] in converters:
                # A converter calling a converter is the inner machinery, not
                # a handler: `aggregate_from_proto_query` delegates to
                # `join_from_proto` for its join arm, and neither has a
                # context to check with. The obligation is on whoever called
                # the outer one, which this rule already covers.
                continue
            window = lines[max(0, at - REACH) : at]
            if not any(AUTHORIZED.search(above) or AUTHORIZES.search(above) for above in window):
                problems.append(
                    f"{path.name}:{at + 1}: `{owners[at]}` calls "
                    f"`{found.group(1)}` with no authorisation in the "
                    f"{REACH} lines above it.\n"
                    "  That converter takes a `Catalog` and no context, so it "
                    "resolves and converts for a caller nobody has checked — "
                    "security finding 8, which reached four RPCs this way. "
                    "Authorise the tables the request names first."
                )
    return problems


def wire_handlers(files: list[Path]) -> set[str]:
    """Methods taking a `Request<pb::..>`, which is what reaches the wire."""
    names: set[str] = set()
    for path in files:
        for found in TAKES_REQUEST.finditer(path.read_text()):
            if WIRE_REQUEST in found.group(2):
                names.add(found.group(1))
    return names


def unauthenticated_handlers(files: list[Path], handlers: set[str]) -> list[str]:
    """Every wire handler derives a `SecurityContext` somewhere in its body.

    Anywhere rather than within `REACH`, unlike the rules above: those are
    about *ordering* — the check must precede the use — and this one is about
    presence. A handler that authenticates at all has established who is
    asking; where in the body it does so is not the hazard.
    """
    authenticating: set[str] = set()
    for path in files:
        lines = path.read_text().splitlines()
        owners = enclosing_functions(lines)
        for at, line in enumerate(lines):
            if AUTHENTICATES.search(line):
                authenticating.add(owners[at])

    problems = []
    for name in sorted(handlers - authenticating):
        problems.append(
            f"`{name}` takes a `{WIRE_REQUEST}..>` and never derives a "
            "SecurityContext.\n"
            "  It is reachable from the wire, so it answers whoever can open a "
            "socket — `leadership` did exactly that, under a configuration "
            "whose banner promises to refuse every request. Call "
            "`self.context(&request)?` even where there is no table to "
            "authorise."
        )
    return problems


def unrostered_authenticators(files: list[Path]) -> list[str]:
    """Every `impl Authenticator for T` is named in `AUTHENTICATORS`.

    Both directions. An implementation missing from the list means a new
    authenticator nothing exercises; a name in the list with no implementation
    means a test looping over something that is gone, which passes while
    covering one case fewer than it claims.
    """
    implemented: set[str] = set()
    rostered: set[str] | None = None
    for path in files:
        text = path.read_text()
        implemented |= set(IMPLEMENTS.findall(text))
        found = ROSTER_LIST.search(text)
        if found is not None:
            rostered = set(re.findall(r'"(\w+)"', found.group(1)))
    if not implemented:
        # Not an error here: a caller may be checking a subtree with no
        # authenticators in it. The roster check is about agreement, and there
        # is nothing to agree about.
        return []

    if rostered is None:
        return [
            f"{len(implemented)} `impl Authenticator for` and no AUTHENTICATORS "
            "list anywhere to account for them"
        ]

    problems = []
    for name in sorted(implemented - rostered):
        problems.append(
            f"`impl Authenticator for {name}` is not in AUTHENTICATORS in "
            "the AUTHENTICATORS list.\n"
            "  Add it, and give it a case in "
            "`no_authenticator_resolves_a_duplicated_identity_key` — finding 6 "
            "was a duplicated identity key resolved rather than refused, and it "
            "survived in the implementation nobody was testing."
        )
    for name in sorted(rostered - implemented):
        problems.append(
            f"AUTHENTICATORS names `{name}`, which implements `Authenticator` "
            "nowhere any more. Delete it and its case."
        )
    return problems


def main(argv: list[str] | None = None) -> int:
    # The subject is an argument so the tests beside this file can run the real
    # checks over a file they wrote, rather than against `service.rs` — where a
    # broken check passes for whatever `service.rs` happens to contain.
    argv = sys.argv[1:] if argv is None else argv
    sources = [Path(one) for one in argv] if argv else list(SOURCES)

    files: list[Path] = []
    for source in sources:
        if source.is_dir():
            files.extend(sorted(source.rglob("*.rs")))
        elif source.exists():
            files.append(source)

    problems: list[str] = []
    seen: set[str] = set()
    bare = 0
    checks = 0
    views = 0

    for path in files:
        lines = path.read_text().splitlines()
        names = enclosing_functions(lines)
        for at, line in enumerate(lines):
            if BARE.search(line):
                bare += 1
                owner = names[at]
                seen.add(owner)
                if owner not in UNAUTHORIZED:
                    problems.append(
                        f"{path.name}:{at + 1}: `{owner}` resolves a table with the "
                        "bare `self.table(..)`, which does not authorise.\n"
                        "  Use `self.authorized_table(&context, name, action)` with "
                        "the action the kernel will check, or add it to "
                        "UNAUTHORIZED with a reason it is safe."
                    )
            if VIEWS.search(line):
                views += 1
                owner = names[at]
                seen.add(owner)
                if owner not in RESOLVES_VIEWS:
                    problems.append(
                        f"{path.name}:{at + 1}: `{owner}` reads the view registry, "
                        "which resolves a name the catalog does not hold.\n"
                        "  A view's base table must be authorised through "
                        "`authorized_table` before the caller sees a row of it, and "
                        "only one read path has opted in. Call "
                        f"`authorized_read_source`, or add `{owner}` to "
                        "RESOLVES_VIEWS with the reason it is safe — which is a "
                        "decision about `docs/views.md` §3a, not a formality."
                    )
            if FINGERPRINT.search(line):
                checks += 1
                owner = names[at]
                seen.add(owner)
                window = lines[max(0, at - REACH) : at]
                if owner in FINGERPRINT_BY_CALLER:
                    continue
                if not any(AUTHORIZED.search(above) for above in window):
                    problems.append(
                        f"{path.name}:{at + 1}: `fingerprint::check` in `{owner}` "
                        f"with no `authorized_table` in the {REACH} lines above "
                        "it.\n"
                        "  Fingerprinting before authorising is security finding "
                        "8: a caller with no grant confirms a guessed schema one "
                        "fingerprint at a time. If the table arrives already "
                        f"authorised, add `{owner}` to FINGERPRINT_BY_CALLER "
                        "with the callers that check it."
                    )

    problems.extend(unrostered_authenticators(files))
    converters = catalog_converters(files)
    if converters:
        problems.extend(unauthorised_conversions(files, converters))
    handlers = wire_handlers(files)
    problems.extend(unauthenticated_handlers(files, handlers))

    # A check that finds nothing has stopped checking, and reads identically to
    # one that found nothing wrong. `CLAUDE.md`: "a check that never fires is a
    # check nobody has debugged". If the handlers move, this fails rather than
    # going quietly green over an empty tree.
    if not files:
        problems.append(f"no Rust sources under {[str(one) for one in sources]}")
    elif checks == 0:
        problems.append(
            f"no `fingerprint::check` anywhere in {len(files)} file(s). Either "
            "the handlers moved and SOURCES is stale, or the check is no longer "
            "what this guards — both need a person, not a pass."
        )
    elif not handlers:
        # The never-fires guard for rule 5, on the same reasoning as the one
        # below: `Request<pb::` is a spelling, and a crate that aliased the
        # generated module to anything but `pb` would leave this matching
        # nothing and printing `ok`.
        problems.append(
            f"no method takes a `{WIRE_REQUEST}..>` in {len(files)} file(s), "
            "so rule 5 checked nothing. Either the service moved or the "
            "generated proto module is no longer spelled `pb` — both need a "
            "person, not a pass."
        )
    elif views == 0:
        # The same never-fires reasoning as the others. `self.views` is a
        # spelling; a rename of the field would leave this rule matching
        # nothing and printing `ok` over a tree where any handler may resolve a
        # view unauthorised.
        problems.append(
            f"nothing reads `self.views` in {len(files)} file(s), so rule 6 "
            "checked nothing. Either views were removed — in which case delete "
            "this rule and RESOLVES_VIEWS deliberately — or the field was "
            "renamed. Both need a person, not a pass."
        )
    elif not converters:
        # The same never-fires reasoning as above, and this rule needs it more.
        # `fingerprint::check` is one literal string; a converter is recognised
        # by three conditions on a signature, so a rename of the generated
        # proto module from `pb` to anything else would leave the rule matching
        # nothing and reporting success. Zero is a person's problem, not a
        # pass — and if the converters really are gone, deleting this branch is
        # the deliberate edit that says so.
        problems.append(
            f"no converter taking a `{WIRE}` request and a `{CATALOG}` in "
            f"{len(files)} file(s), so rule 3 checked nothing. Either both "
            "converters went away, or the signature they are recognised by "
            "moved — a proto module renamed out of `pb`, a catalog passed by "
            "value. Both need a person."
        )

    # A stale exemption is its own defect: it reads as a live hazard somebody
    # accepted, and the next person weighs a decision nobody is making.
    for listed, where in (
        (UNAUTHORIZED, "UNAUTHORIZED"),
        (FINGERPRINT_BY_CALLER, "FINGERPRINT_BY_CALLER"),
        (RESOLVES_VIEWS, "RESOLVES_VIEWS"),
    ):
        for name, reason in listed.items():
            if name not in seen:
                problems.append(
                    f"{where} lists `{name}`, which no longer does what the entry "
                    f"exempts. Delete it; its reason was: {reason}"
                )

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    authenticators = sum(len(IMPLEMENTS.findall(path.read_text())) for path in files)
    print(
        f"ok    {len(files)} files, {bare} bare resolutions all accounted for, "
        f"{checks} fingerprint checks all authorised first, "
        f"{len(converters)} converters all called from authorised handlers, "
        f"{len(handlers)} wire handlers all authenticating, "
        f"{authenticators} authenticators all rostered, "
        f"{views} view-registry reads all rostered"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
