#!/usr/bin/env python3
"""Every shippable package declares the same version, and a tag agrees with it.

    python3 scripts/check_versions.py            # the tree agrees with itself
    python3 scripts/check_versions.py v0.2.0     # ...and with that tag

# Why this exists before any of the publishing does

Five things ship from this repository and four of them carry a version number
they were given by hand:

- the Rust workspace (`Cargo.toml`'s `workspace.package.version`), which every
  crate inherits with `version.workspace = true`;
- `clients/typescript/package.json`, published to npm;
- `clients/python/pyproject.toml`, published to PyPI;
- the container image, tagged from the same number;
- and `clients/go`, which has no version of its own at all — the Go proxy reads
  one out of a `clients/go/vX.Y.Z` git tag, so its "declaration" is the tag.

A release is a moment when all of those have to say the same thing, and it is
the one moment nobody is watching: `git tag v0.2.0` on a tree whose
`package.json` still says `0.0.1` publishes **`@slate-orm/client@0.0.1`** —
silently, successfully, and to a registry that will not let the number be
reused. That is not a hypothetical shape for this repository. Its whole
release story until now was *not to publish*, and the entry that recorded that
decision said why in a sentence that generalises:

> The landing page's install lines used to name packages that do not exist
> anywhere; the fix for that was to reword the page, not to wire up a token and
> find out.

Wiring up the token is exactly what is happening now, so the thing that made
"find out" dangerous has to be checked first.

# Why one number for all of them

Independent versions are the more grown-up arrangement and this repository has
not earned it: the three clients are generated from one proto, tested against
one server by one conformance runner, and released together or not at all. One
number means a reader can pair any client with any server by looking, and it
means this check is a comparison rather than a compatibility matrix. When that
stops being true the right move is a matrix and a different check, not a
version that drifts quietly.

Run directly: `python3 scripts/check_versions.py`.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parent.parent

#: `workspace.package.version` in the root manifest is the one number. Every
#: other declaration below is compared against it rather than against each
#: other, so a failure names a direction rather than a disagreement.
WORKSPACE = "Cargo.toml"

#: A `v` tag, as `release.yml` triggers on. `v0.2.0` carries `0.2.0`; a
#: pre-release suffix (`v0.2.0-rc.1`) is kept whole, because npm and PyPI
#: spell those differently and a release wearing one has to say so in every
#: package rather than in the tag alone.
TAG = re.compile(r"^v(.+)$")


def workspace_version(root: Path) -> str:
    return tomllib.loads((root / WORKSPACE).read_text())["workspace"]["package"]["version"]


def declared(root: Path) -> dict[str, str | None]:
    """Each shippable package's declared version, keyed by the file that holds it.

    `None` means the file is there and the version is not, which is a different
    failure from a wrong number and reads differently below. A file that is
    missing entirely is left out — this guard is about agreement, and
    `scripts/check_workspace.py` is about what exists.
    """
    found: dict[str, str | None] = {}

    npm = root / "clients/typescript/package.json"
    if npm.is_file():
        found[str(npm.relative_to(root))] = json.loads(npm.read_text()).get("version")

    python = root / "clients/python/pyproject.toml"
    if python.is_file():
        found[str(python.relative_to(root))] = (
            tomllib.loads(python.read_text()).get("project", {}).get("version")
        )

    return found


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    problems: list[str] = []
    want = workspace_version(ROOT)
    found = declared(ROOT)

    if not found:
        # The never-fires case. Both client manifests moved, and a guard that
        # compared nothing must not print the line a guard that compared two
        # prints.
        print(
            "FAIL  no publishable package manifest found. Either the clients "
            "moved or this guard is looking in the wrong place — both need a "
            "person, not a pass.",
            file=sys.stderr,
        )
        return 1

    for where, has in sorted(found.items()):
        if has is None:
            problems.append(
                f"{where} declares no version, and it is published. Give it "
                f"{want}, the workspace version."
            )
        elif has != want:
            problems.append(
                f"{where} says {has}; {WORKSPACE}'s workspace version is {want}.\n"
                f"      A tag publishes whatever each manifest says, not what "
                f"the tag says — so this ships {has} under a release named for "
                f"{want}, to a registry that will not let the number be reused."
            )

    for tag in argv:
        named = TAG.match(tag)
        if not named:
            problems.append(f"{tag!r} is not a v-tag; release.yml triggers on `v*`.")
        elif named.group(1) != want:
            problems.append(
                f"tag {tag} carries {named.group(1)}; {WORKSPACE}'s workspace "
                f"version is {want}.\n"
                f"      Bump the tree to {named.group(1)} and commit it before "
                f"tagging, or tag v{want}."
            )

    if problems:
        for problem in problems:
            print(f"FAIL  {problem}", file=sys.stderr)
        print(f"\n{len(problems)} version problem(s)", file=sys.stderr)
        return 1

    named = " and ".join(sorted(found)) or "nothing"
    print(
        f"ok    {want} in {WORKSPACE}, {named}"
        + (f", and {len(argv)} tag(s)" if argv else "")
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
