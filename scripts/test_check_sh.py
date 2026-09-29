#!/usr/bin/env python3
"""`scripts/check.sh` covers every static check in `ci.yml`, or says why not.

A convenience script that runs "the checks" is worth exactly as much as its
list is current, and a list that quietly falls behind is the failure this
repository keeps meeting in other forms: a workflow filter that matched
nothing, a suite that skipped itself, a job that had never run. The script was
written because three commits in one afternoon went red on checks that existed
and were not run; a fourth, six weeks later, on a check that existed and was
not *listed* would be the same story with an extra step.

So every step in `ci.yml` is accounted for here. It is either in the script, or
it is in `ELSEWHERE` below with a reason it cannot be — and adding a step to
`ci.yml` fails this until somebody chooses. That is the `EXPECTED_REFUSALS`
pattern the conformance runner already uses, for the same reason: a list you
are forced to edit is a list that stays true.

**And every workflow-level `env:` variable, which was the hole.** Comparing
commands is not enough, because `ci.yml` sets `RUSTFLAGS: -D warnings` once at
the top and no `run:` line mentions it. `cargo clippy --workspace
--all-targets` is therefore a *different check* in CI from the byte-identical
command in `check.sh`: a warn-level lint exits zero here and fails the job
there. That happened — `clippy::indexing_slicing` on an in-range index — after
a green local run of this very script. The script now exports those variables
and `environment_matches` below holds the two lists together.

Run directly: `python3 scripts/test_check_sh.py`.
"""

from __future__ import annotations

import pathlib
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Steps `check.sh` deliberately does not run, and why.
#:
#: Keyed on the command as `ci.yml` spells it. The reason is the point of the
#: entry: "needs a built binary" is a fact about the step, and a reader
#: deciding whether to extend the script needs it more than the entry itself.
ELSEWHERE = {
    # Environment, not a check.
    "pip install uv==0.8.17": "installing a tool",
    "uv pip install --system -e '.[dev]'": "installing the client",
    "uv pip install --system -e './clients/python[dev]'": "installing the client",
    "pip install -e clients/python": "installing the client",
    "pip install grpcio protobuf": "installing dependencies",
    "npm ci": "installing dependencies",
    "npm run build": "building the client a harness links to",
    "chmod +x bin/slate-serverd": "unpacking an artefact",
    "chmod +x bin/slate-testserver": "unpacking an artefact",
    "cargo install wasm-bindgen-cli --version 0.2.128 --locked": "installing a tool",
    "npx playwright install --with-deps chromium": "installing a browser",
    "sh site/build-wasm.sh": "a build, and it needs the tool above",
    "cargo build -p slate-serverd --bin slate-serverd": "a build",
    "cargo build -p slate-testserver --bin slate-testserver": "a build",
    # Workflows other than `ci.yml`, covered from 2026-09-29.
    "python3 scripts/run_mutations.py": (
        "breaks the real tree at every guard and restores it. `check.sh` is run"
        " beside other sessions' work and before every commit; a script that"
        " mutates tracked files is the one thing it must not do"
    ),
    "git diff --exit-code": (
        "asserts the working tree is clean, which is true of a checkout and"
        " false of anybody's machine mid-change"
    ),
    "ls -l dist": "listing an artefact another job built",
    # Suites. Each needs a built binary, a browser or a container, which is
    # what `check.sh` promises not to need. Listed one by one rather than by a
    # pattern, so that a *new* suite is a decision rather than a match.
    "cargo test --workspace": "slow, and the disk note in CLAUDE.md",
    "cargo test -p slate-orm --features json": "a feature build",
    "cargo test -p slate-slatedb --test s3": "needs MinIO",
    "go test ./...": "needs a built slate-serverd",
    "go test ./schema/": "needs the generated declaration and a server",
    "python -m pytest -q": "needs a built slate-testserver",
    "npm test": "needs a built slate-serverd",
    "./run.sh": "needs a built slate-serverd",
    "python3 site/check/docs.py": "reads the built site",
    "python3 site/check/quickstarts.py": "runs the docs' code against a server",
    "python3 site/check/workbench.py": "needs a browser and the wasm build",
    "python3 site/check/pages.py": "needs a browser",
    "./run.sh --trips 20000": "the whole stack, against object storage",
    "./run.sh --conformance": "three SDKs and a server",
    "./run.sh --e2e": "a browser",
    "./run.sh --rows 20 --runs 2": "a benchmark, against a server",
    "crates/slate-headbench/run.sh --smoke": (
        "five benchmarks, each standing up a head node — minutes, and "
        "`check.sh` promises seconds"
    ),
    "crates/slate-slatedb/run.sh --smoke": (
        "eight examples, most of them over a real S3 server in the process — "
        "minutes, and one of them is a server"
    ),
    # These two are the fast ones — about thirty seconds between them in a
    # debug build — and they are still not in `check.sh`, because the cost
    # that matters is the *build*, not the run. `check.sh` needs no built
    # binary, which is the whole reason it is worth running before every
    # commit; `cargo build -p slate-kernel --examples` from cold is minutes.
    "sh scripts/run_examples.sh slate-kernel --smoke": (
        "four examples, and building them is a cargo build — `check.sh` "
        "needs no built binary, which is what makes it cheap"
    ),
    "sh scripts/run_examples.sh slate-orm --smoke": (
        "the same, for the ORM's one end-to-end tour"
    ),
}

#: Multi-line `run: |` blocks, by the `name:` above them.
#:
#: None of them is a static check: two are shell assertions about artefacts and
#: one starts a container. Named rather than counted, so that adding a block
#: fails here with its own name in the message.
ELSEWHERE_BLOCKS = {
    "Both binaries exist": "asserts on an artefact this script does not build",
    "The generated declarations match the catalog": "needs a built slate-serverd",
    "The retention example's declaration matches its catalog": (
        "needs a built slate-serverd"
    ),
    "Start MinIO": "starts a container",
    "The protobuf compiler, for the stub-freshness check": "installs a tool",
    "Soak the tuple codec properties": "a release build, minutes",
    # pages.yml and release-build.yml, covered from 2026-09-29. Every one
    # asserts on something built or cross-compiled by the job above it, which
    # is the category `check.sh` promises to stay out of.
    "The pages exist": "asserts on the built site",
    "If that failed, here is why and what to do": (
        "prints guidance when the step above fails; it runs only on failure and"
        " checks nothing"
    ),
    "Cross-compilation toolchain": "installs a toolchain",
    "Build": "a release build for a shipping target",
    "It starts, and validates a configuration": "runs the binary the step above built",
    "Name it after its target": "renames an artefact",
}


#: Workflow-level `env:` variables `check.sh` deliberately does not export.
#:
#: Same shape and same reason as `ELSEWHERE`: a variable that changes what a
#: check *means* has to be set here too, and one that does not still has to be
#: named, so that deciding which it is happens once rather than never.
ENV_ELSEWHERE: dict[str, str] = {}


def workflow_env() -> dict[str, str]:
    """The workflow-level `env:` block of `ci.yml`, as name to value.

    Only the top-level block, which is the one that applies to every step and
    is therefore the one nothing in a `run:` line reveals. A `env:` nested
    under a job or a step sits beside the command it modifies, where a reader
    comparing the two files can see it.
    """
    lines = (ROOT / ".github/workflows/ci.yml").read_text().splitlines()
    found: dict[str, str] = {}
    inside = False
    for line in lines:
        if line.rstrip() == "env:":
            inside = True
            continue
        if not inside:
            continue
        entry = re.match(r"^  ([A-Za-z_][A-Za-z0-9_]*): (.+)$", line)
        if entry:
            found[entry.group(1)] = entry.group(2).strip()
            continue
        # The block ends at the first line that is not one of its entries,
        # which in this file is the blank line before `jobs:`.
        if line.strip():
            break
    return found


def script_env() -> dict[str, str]:
    """Every variable `check.sh` exports, as name to value."""
    text = (ROOT / "scripts/check.sh").read_text()
    # The value may or may not be quoted — `RUSTFLAGS="-D warnings"` has to be
    # and `CARGO_TERM_COLOR=always` does not — so the quote is captured and
    # back-referenced rather than stripped afterwards, which would also strip a
    # value that legitimately ends in one.
    return {
        name: value
        for name, _quote, value in re.findall(
            r'^export ([A-Za-z_][A-Za-z0-9_]*)=("?)(.*?)\2$', text, re.MULTILINE
        )
    }


def environment_matches() -> list[str]:
    """Complaints about `check.sh`'s exports against `ci.yml`'s `env:`."""
    wanted = workflow_env()
    assert wanted, "ci.yml has no workflow-level env: block; this guard is blind"
    exported = script_env()
    complaints = []
    for name, value in sorted(wanted.items()):
        if name in ENV_ELSEWHERE:
            continue
        if name not in exported:
            complaints.append(
                f"ci.yml sets {name}={value} for every step and check.sh does not "
                f"export it, so the same command is a different check in the two "
                f"places; export it or give it a reason in ENV_ELSEWHERE"
            )
        elif exported[name] != value:
            complaints.append(
                f"ci.yml sets {name}={value} and check.sh exports {name}={exported[name]}"
            )
    for name in sorted(set(ENV_ELSEWHERE) - set(wanted)):
        complaints.append(f"ENV_ELSEWHERE names {name}, which ci.yml no longer sets")
    return complaints


#: Every workflow, not only `ci.yml`.
#:
#: It was `ci.yml` alone, and `ledger/2026-09-29-a-weekly-run-and-a-button.md`
#: recorded what that costs: a second workflow adding a step is outside the
#: rule, so `mutations.yml`'s two steps happened to be accounted for by
#: coincidence rather than by anything. A rule scoped to one file is a rule
#: about that file, and this repository now has five.
#:
#: `sorted()` so the report is stable; the glob so a sixth workflow is covered
#: the day it lands rather than the day somebody remembers.
def workflows() -> list[pathlib.Path]:
    """Every workflow file, sorted."""
    directory = ROOT / ".github" / "workflows"
    return sorted(directory.glob("*.yml")) + sorted(directory.glob("*.yaml"))


def workflow_steps(
    files: list[list[str]] | None = None,
) -> tuple[list[tuple[str, str]], list[str]]:
    """Every `- run:` in every workflow: single-line steps, and named blocks.

    `files` is one list of lines per workflow, and defaults to the real ones.
    It is a parameter only so that the joining below can be tested: the two
    properties that matter — a second workflow is read at all, and one
    workflow's last step cannot borrow the next one's `working-directory:` —
    are properties of *several* files, and nothing could exercise them while
    this function's only input was the repository.
    """
    if files is None:
        files = [path.read_text().splitlines() for path in workflows()]
    lines: list[str] = []
    for one in files:
        lines.extend(one)
        # A sentinel between files, so a `- run:` at the end of one and a
        # `working-directory:` at the start of the next cannot be read as one
        # step. Nothing else in this parser looks across a line boundary, and
        # no workflow here ends that way today — this is the case that would
        # be silently wrong the first time one does.
        lines.append("")
    steps: list[tuple[str, str]] = []
    blocks: list[str] = []
    for at, line in enumerate(lines):
        block = re.match(r"^\s*- name: (.+)$", line)
        if block:
            # A `- name:` whose step is a `run:` rather than a `uses:`. The
            # look-ahead is two lines because `run: |` puts the body below.
            following = "\n".join(lines[at + 1 : at + 3])
            if "run:" in following:
                blocks.append(block.group(1).strip())
            continue
        step = re.match(r"^\s*- run: (.+)$", line)
        if not step:
            continue
        command = step.group(1).strip()
        if command == "|":
            continue
        # `working-directory:` follows the `run:` it applies to, in this file.
        directory = "."
        if at + 1 < len(lines):
            where = re.match(r"^\s*working-directory: (.+)$", lines[at + 1])
            if where:
                directory = where.group(1).strip()
        steps.append((directory, command))
    return steps, blocks


def script_checks() -> set[tuple[str, str]]:
    """Every (directory, command) `check.sh` lists."""
    text = (ROOT / "scripts/check.sh").read_text()
    listed = re.search(r"cat <<'LIST'\n(.*?)\nLIST\n", text, re.DOTALL)
    if not listed:
        raise AssertionError("check.sh no longer has a LIST block; this guard is blind")
    found = set()
    for line in listed.group(1).splitlines():
        if not line.strip():
            continue
        _, directory, command = line.split("|", 2)
        found.add((directory, command))
    return found


#: Two files, to exercise the joining that one file cannot.
#:
#: The first ends on a `- run:` and the second opens with a
#: `working-directory:`, which is the arrangement the sentinel exists for and
#: the one no real workflow has today. Written after a mutation deleting the
#: sentinel survived every other case here: a defence against a shape the tree
#: does not yet contain is untested by construction unless something supplies
#: the shape.
#: The second file's *first* line is the `working-directory:`, which is the
#: only arrangement that reaches the bug. A first draft put `jobs:` above it
#: and the sentinel mutation survived anyway — the fixture was a picture of
#: the shape rather than the shape, and the picture proves nothing.
TWO_FILES = [
    ["jobs:", "  a:", "    steps:", "      - run: first-command"],
    ["      working-directory: clients/go", "jobs:", "  b:", "    steps:"],
]


def joining() -> list[str]:
    """Complaints about how `workflow_steps` stitches several files together."""
    said: list[str] = []

    steps, _ = workflow_steps(TWO_FILES)
    if ("clients/go", "first-command") in steps:
        said.append(
            "a `- run:` at the end of one workflow took the `working-directory:` "
            "at the start of the next; the sentinel between files is gone"
        )
    if (".", "first-command") not in steps:
        said.append(f"the two-file fixture parsed to {steps}, which has lost the step")

    # And that a second file is read at all, which is the whole widening.
    one, _ = workflow_steps(TWO_FILES[:1])
    both, _ = workflow_steps(TWO_FILES + [["      - run: second-command"]])
    if len(both) <= len(one):
        said.append("a second workflow's steps are not read; the glob or the loop is gone")

    return said


def main() -> int:
    steps, blocks = workflow_steps()

    # The never-fires guards, reported rather than asserted. They were three
    # bare `assert`s, and a mutation narrowing the glob back to `ci.yml` scored
    # as **NOTHING RAN** rather than as caught: an `AssertionError` kills the
    # run before the `N passed, M failed` line `scripts/mutate.py` reads, so a
    # guard that correctly refused looked exactly like a suite that never
    # started. That is the second of `mutate.py`'s six lies, met inside a file
    # whose whole job is to stop a check going quiet.
    found = workflows()
    blind = []
    if len(found) < 2:
        blind.append(f"found {len(found)} workflow file(s); the glob is not matching")
    if len(steps) <= 40:
        blind.append(f"the parser found {len(steps)} steps; it is not parsing")
    if len(blocks) < 3:
        blind.append(f"the parser found {len(blocks)} named blocks; it is not parsing")

    covered = script_checks()
    # The script runs some checks CI does not — `gofmt -l`, two `tsc --noEmit`
    # runs, `go vet` on the demo's adapter — which is allowed and is the point
    # of having it. Only the other direction is a failure.
    missing = [
        (directory, command)
        for directory, command in steps
        if (directory, command) not in covered and command not in ELSEWHERE
    ]
    unknown_blocks = [name for name in blocks if name not in ELSEWHERE_BLOCKS]
    env_complaints = environment_matches()

    # Entries that no longer match anything are the other half: a stale reason
    # is a reader believing a step exists that does not.
    commands = {command for _, command in steps}
    stale_elsewhere = sorted(set(ELSEWHERE) - commands)
    stale_blocks = sorted(set(ELSEWHERE_BLOCKS) - set(blocks))

    # Reported as named checks with a `N passed, M failed` summary, which is
    # the shape every other `scripts/test_*.py` here prints — and, more to the
    # point, the shape `scripts/mutate.py` can read. This script used to print
    # one `ok` line and no count, so a mutation run against it reported "no
    # test results at all" and could say nothing about whether a guard worked.
    # `test_codegen.py` had the identical problem and it was found the same
    # way.
    checks: list[tuple[str, list[str]]] = [
        (
            "every workflow step is in check.sh or in ELSEWHERE",
            [f"not in check.sh and not in ELSEWHERE: (in {d}) {c}" for d, c in missing],
        ),
        (
            "every named run-block is accounted for",
            [f"a named run-block nothing accounts for: {name}" for name in unknown_blocks],
        ),
        (
            "check.sh exports ci.yml's workflow-level env",
            env_complaints,
        ),
        (
            "the parser still sees the workflows",
            blind,
        ),
        (
            "several workflows are stitched together without bleeding into each other",
            joining(),
        ),
        (
            "no ELSEWHERE entry names a step the workflows have dropped",
            [f"ELSEWHERE names a step no workflow has: {c}" for c in stale_elsewhere]
            + [f"ELSEWHERE_BLOCKS names a block no workflow has: {n}" for n in stale_blocks],
        ),
    ]

    passed = 0
    failed = 0
    for name, complaints in checks:
        if complaints:
            failed += 1
            print(f"FAIL  {name}")
            for complaint in complaints:
                print(f"      {complaint}")
        else:
            passed += 1
            print(f"ok    {name}")

    print(f"\n{passed} passed, {failed} failed")
    if failed:
        return 1
    print(
        f"      {len(steps)} steps, {len(blocks)} blocks and "
        f"{len(workflow_env())} env vars, all accounted for"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
