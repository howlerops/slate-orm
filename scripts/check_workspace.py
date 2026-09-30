#!/usr/bin/env python3
"""Every crate in this repository is a member of the root workspace.

A crate outside it is never built by `cargo test --workspace`, so nothing
notices when it stops compiling. That is not hypothetical: `clients/python/
testserver` spent its life detached, and three separate things rotted in it
unnoticed — a missing `StreamExt` import, two indexes sharing an id, and grants
that predated `Action::Explain` becoming its own action. All three surfaced at
once on the day it was made a member.

So this refuses a `Cargo.toml` that declares a package the root workspace does
not list, and it refuses one that declares a `[workspace]` of its own — the
detachment marker, which is how the testserver got out in the first place.

# And the Dockerfile's copy list

A third rule, added after the failure it describes. `Dockerfile` copies part
of the tree and runs `cargo build -p slate-serverd` in it, and cargo resolves
the **whole** workspace before it builds one package — so a member the image
does not copy is `failed to load manifest for workspace member` and a red
`container image` job. Adding `examples/helpdesk` to `members` did exactly
that: twenty-two jobs green and one red, on a change that touched nothing the
image contains.

Nothing local reproduces it, because every other command runs in a complete
checkout. That is the shape this file already exists for — a list maintained
by hand, in a place where being wrong is silent — so the rule is here rather
than in a comment in the Dockerfile.

**And `.dockerignore` is the second half, added an hour later because the
first half was not enough.** That rule shipped with a recorded caveat saying
it did not read `.dockerignore` and that "nothing currently excludes a
directory holding a member" — a claim about a file nobody had opened. The
file is an *allow-list*: `*` and then a handful of `!` re-inclusions, chosen
so a new large directory is excluded by default rather than forgotten. So the
very next run failed on the other side of the same mistake:

    failed to compute cache key: "/examples/helpdesk": not found

Two lists, both hand-maintained, and a member has to be in both. Checking one
and reasoning about the other is how the second push failed for the same
reason as the first.

Run it directly, or let CI: `python3 scripts/check_workspace.py`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Directories with their own build systems and no relationship to cargo, plus
#: everything a build writes. `target/` holds vendored sources of dependencies
#: that are emphatically not ours to enrol.
SKIP = {"target", "node_modules", ".git", "dist", "dist-test", "build", ".venv"}


def members() -> set[Path]:
    """The paths the root manifest lists, resolved."""
    text = (ROOT / "Cargo.toml").read_text()
    block = re.search(r"^members\s*=\s*\[(.*?)^\]", text, re.DOTALL | re.MULTILINE)
    if block is None:
        raise SystemExit("the root Cargo.toml has no `members` list to read")
    return {(ROOT / name).resolve() for name in re.findall(r'"([^"]+)"', block.group(1))}


def manifests() -> list[Path]:
    """Every Cargo.toml under the repository except the root's."""
    found = []
    for path in ROOT.rglob("Cargo.toml"):
        if any(part in SKIP for part in path.relative_to(ROOT).parts):
            continue
        if path.parent.resolve() != ROOT:
            found.append(path)
    return sorted(found)


def copied(dockerfile: Path) -> list[Path]:
    """The build context paths a `Dockerfile` copies into the image.

    `COPY --from=<stage>` is skipped: it copies out of an earlier stage rather
    than out of the tree, so it says nothing about what the context needs. The
    destination argument is dropped — only the sources matter here.
    """
    out: list[Path] = []
    for line in dockerfile.read_text().splitlines():
        words = line.split()
        if not words or words[0].upper() != "COPY":
            continue
        words = [w for w in words[1:] if not w.startswith("--")]
        # The last word is the destination. A `COPY a b c dst` copies three.
        for source in words[:-1]:
            out.append((ROOT / source).resolve())
    return out


def admitted(ignore: Path) -> list[Path] | None:
    """The paths a deny-everything `.dockerignore` lets back in.

    `None` when the file does not have that shape — no bare `*` line — which
    means it is a deny-list and this parser cannot say what it admits. The
    caller refuses rather than passing: a rule that cannot read its input and
    says nothing is the failure this whole file is about.

    Trailing slashes are stripped so `!crates/` and `!crates` are one thing,
    and a `!` re-inclusion of a *parent* admits everything under it, which is
    how `!crates/` covers eleven members.
    """
    lines = [one.strip() for one in ignore.read_text().splitlines()]
    lines = [one for one in lines if one and not one.startswith("#")]
    if "*" not in lines:
        return None
    return [
        (ROOT / one[1:].rstrip("/")).resolve() for one in lines if one.startswith("!")
    ]


def main() -> int:
    listed = members()
    problems: list[str] = []

    dockerfile = ROOT / "Dockerfile"
    if not dockerfile.exists():
        # The never-fires case: a renamed or deleted Dockerfile would make the
        # rule below check nothing and say nothing, which is the failure this
        # whole file is about one level up.
        problems.append(
            "there is no Dockerfile, so the copy rule below checks nothing. "
            "Either it moved — point this at it — or the image is gone and "
            "the rule should go with it."
        )
    else:
        sources = copied(dockerfile)
        for member in sorted(listed):
            if any(member == one or one in member.parents for one in sources):
                continue
            problems.append(
                f"the Dockerfile copies no path containing "
                f"{member.relative_to(ROOT)}, which the root workspace lists "
                f"in `members`. `cargo` resolves the whole workspace before it "
                f"builds one package, so the image's `cargo build` will fail "
                f"with `failed to load manifest for workspace member`. Add a "
                f"`COPY` for it, or take it out of `members`."
            )

    ignore = ROOT / ".dockerignore"
    if not ignore.exists():
        # No `.dockerignore` means nothing is excluded, so every `COPY` above
        # finds its source. Not a problem — but say so, because a reader of
        # the pass line below would otherwise assume this rule ran.
        print("note  there is no .dockerignore, so nothing is excluded from the context")
    else:
        allowed = admitted(ignore)
        if allowed is None:
            problems.append(
                ".dockerignore has no bare `*` line, so it is a deny-list and "
                "the rule below cannot say what it admits. It was an "
                "allow-list — `*` and a few `!` re-inclusions — chosen so a "
                "new large directory is excluded by default. Read it, and "
                "either restore that shape or teach `admitted()` the new one."
            )
        else:
            for member in sorted(listed):
                if any(member == one or one in member.parents for one in allowed):
                    continue
                problems.append(
                    f".dockerignore admits no path containing "
                    f"{member.relative_to(ROOT)}, which the root workspace "
                    f"lists in `members`. The file excludes everything and "
                    f"names what comes back, so a `COPY` of this path finds "
                    f"nothing: `failed to compute cache key: not found`. Add "
                    f"a `!` line for it beside the `COPY`."
                )

    for manifest in manifests():
        where = manifest.parent.resolve()
        relative = manifest.relative_to(ROOT)
        text = manifest.read_text()

        # A `[workspace]` table in a nested manifest detaches it deliberately.
        # Refused by name, because the symptom of the detachment — a crate that
        # simply never builds — looks like nothing at all.
        if re.search(r"^\[workspace\]", text, re.MULTILINE):
            problems.append(
                f"{relative} declares its own [workspace], which detaches it from the root one"
            )
            continue

        # A manifest with no `[package]` is a workspace-only or template file.
        if not re.search(r"^\[package\]", text, re.MULTILINE):
            continue

        if where not in listed:
            problems.append(
                f"{relative} is a crate the root workspace does not list in `members`"
            )

    # And the reverse: a member that no longer exists is a manifest error the
    # next `cargo` command reports anyway, but naming it here is cheaper than
    # reading a cargo backtrace.
    for member in sorted(listed):
        if not (member / "Cargo.toml").exists():
            problems.append(
                f"`members` lists {member.relative_to(ROOT)}, which has no Cargo.toml"
            )

    if problems:
        print("the workspace and the image's copy list disagree:\n", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        print(
            "\nAdd it to `members` in the root Cargo.toml, or say in a comment there\n"
            "why it is deliberately excluded and add it to SKIP here.",
            file=sys.stderr,
        )
        return 1

    print(
        f"{len(manifests())} crate(s), all members of the root workspace, and "
        f"all {len(listed)} member(s) inside a path the Dockerfile copies and "
        f"the .dockerignore admits"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
