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

Run directly: `python3 scripts/test_check_sh.py`.
"""

from __future__ import annotations

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
    "./run.sh --trips 20000": "the whole stack, against object storage",
    "./run.sh --conformance": "three SDKs and a server",
    "./run.sh --e2e": "a browser",
    "./run.sh --rows 20 --runs 2": "a benchmark, against a server",
}

#: Multi-line `run: |` blocks, by the `name:` above them.
#:
#: None of them is a static check: two are shell assertions about artefacts and
#: one starts a container. Named rather than counted, so that adding a block
#: fails here with its own name in the message.
ELSEWHERE_BLOCKS = {
    "Both binaries exist": "asserts on an artefact this script does not build",
    "The generated declarations match the catalog": "needs a built slate-serverd",
    "Start MinIO": "starts a container",
    "The protobuf compiler, for the stub-freshness check": "installs a tool",
    "Soak the tuple codec properties": "a release build, minutes",
}


def workflow_steps() -> tuple[list[tuple[str, str]], list[str]]:
    """Every `- run:` in `ci.yml`: single-line steps, and named blocks."""
    lines = (ROOT / ".github/workflows/ci.yml").read_text().splitlines()
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


def main() -> int:
    steps, blocks = workflow_steps()
    assert len(steps) > 40, f"the parser found {len(steps)} steps; it is not parsing"
    assert len(blocks) >= 3, f"the parser found {len(blocks)} named blocks"

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

    for directory, command in missing:
        print(f"not in check.sh and not in ELSEWHERE: (in {directory}) {command}")
    for name in unknown_blocks:
        print(f"a named run-block nothing accounts for: {name}")

    # Entries that no longer match anything are the other half: a stale reason
    # is a reader believing a step exists that does not.
    commands = {command for _, command in steps}
    for command in sorted(set(ELSEWHERE) - commands):
        print(f"ELSEWHERE names a step ci.yml no longer has: {command}")
    for name in sorted(set(ELSEWHERE_BLOCKS) - set(blocks)):
        print(f"ELSEWHERE_BLOCKS names a block ci.yml no longer has: {name}")

    stale = (set(ELSEWHERE) - commands) or (set(ELSEWHERE_BLOCKS) - set(blocks))
    if missing or unknown_blocks or stale:
        return 1
    print(f"ok    {len(steps)} steps and {len(blocks)} blocks, all accounted for")
    return 0


if __name__ == "__main__":
    sys.exit(main())
