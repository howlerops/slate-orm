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
import subprocess
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
    # The presence half runs in `check.sh`; this is the half that fetches, and
    # `check.sh` is the command you can run in a container with no network.
    # See the docstring of `scripts/check_outside_premises.py` for why the two
    # are separate rather than one check that always reaches the internet.
    "python3 scripts/check_outside_premises.py --run": (
        "needs the network, which `check.sh` deliberately does not"
    ),
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
    "python3 scripts/test_run_teardown.py": (
        "starts the explorer's whole stack — a head node, a Go binary and two"
        " npm trees — and leaves listeners on four ports if it fails. `check.sh`"
        " promises to need no built binary and to be safe to run beside another"
        " session's work, and this is neither"
    ),
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
    "sh examples/helpdesk/run.sh --smoke": (
        "stands the helpdesk up on SlateDB over an object store, in a "
        "`--release` build. `check.sh` needs no built binary and promises "
        "seconds; this is a cargo release build and a bucket"
    ),
    "sh scripts/run_examples.sh slate-orm --smoke": (
        "the same, for the ORM's one end-to-end tour"
    ),
    # `check.sh` runs `check_versions.py` with no argument, which is the whole
    # of what it can check here: the tree agreeing with itself. The tag is the
    # other half and only a tag run has one, so this spelling — with the ref
    # interpolated — is a different command and belongs here rather than in
    # the script.
    "python3 scripts/check_versions.py ${{ startsWith(github.ref, "
    "'refs/tags/v') && github.ref_name || '' }}": (
        "the same guard `check.sh` runs, plus the tag, which exists only on a "
        "tag run"
    ),
}

#: Steps `check.sh` runs that no workflow does, and why that is right.
#:
#: The mirror of [`ELSEWHERE`], and the half that did not exist. This guard
#: read one direction — every workflow step is in the script — and the comment
#: in `main` said the other way round was "allowed and is the point of having
#: it". That is true of a *few* steps and was being used to excuse fourteen:
#: nine repository guards and five toolchain checks ran only when somebody
#: remembered `check.sh`. `check_versions.py` was the one anybody noticed, by
#: reading a run's step list by hand, and noticing the rest needed this rule.
#:
#: So the allowance is a roster rather than a blanket, for the reason
#: `ELSEWHERE` is one: a list you are forced to edit is a list that stays true.
#: Empty today, deliberately — everything `check.sh` runs now runs in CI too,
#: and the next step that should not has to say why here.
ONLY_LOCAL: dict[tuple[str, str], str] = {}

#: `check.sh` commands a workflow spells differently, and the spelling it uses.
#:
#: One shape only: `gofmt -l` prints what it would rewrite and exits zero
#: either way, so both places wrap it and the wrappers cannot be byte-identical
#: — the script has a shell function and the workflow has a one-liner. Matching
#: on a substring instead would let `go vet ./...` satisfy a rule about
#: `go vet ./... --some-flag`, which is the false-positive shape this
#: repository keeps rejecting, so the pair is written out.
SPELLED_IN_CI: dict[tuple[str, str], str] = {
    ("clients/go", "gofmt -l ."): (
        'test -z "$(gofmt -l .)" || { echo "gofmt would rewrite:"; gofmt -l .; exit 1; }'
    ),
    ("examples/explorer/backends/go", "gofmt -l ."): (
        'test -z "$(gofmt -l .)" || { echo "gofmt would rewrite:"; gofmt -l .; exit 1; }'
    ),
}


def unrun_locally(
    steps: list[tuple[str, str]], covered: set[tuple[str, str]]
) -> tuple[list[str], list[str]]:
    """Complaints about `check.sh` steps no workflow runs, and stale rosters."""
    in_ci = set(steps)
    said: list[str] = []
    for where in sorted(covered):
        if where in in_ci or where in ONLY_LOCAL:
            continue
        spelled = SPELLED_IN_CI.get(where)
        if spelled is not None and (where[0], spelled) in in_ci:
            continue
        directory, command = where
        said.append(
            f"check.sh runs it and no workflow does: (in {directory}) {command}"
            " — add it to a workflow, or to ONLY_LOCAL with a reason"
        )

    # The other half, same as `ELSEWHERE`'s: a roster entry naming a step the
    # script has dropped is a reader believing in a check that is gone.
    stale = [
        f"ONLY_LOCAL names a step check.sh does not run: (in {d}) {c}"
        for d, c in sorted(ONLY_LOCAL)
        if (d, c) not in covered
    ] + [
        f"SPELLED_IN_CI names a step check.sh does not run: (in {d}) {c}"
        for d, c in sorted(SPELLED_IN_CI)
        if (d, c) not in covered
    ]
    return said, stale


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
    # The container image and the registries, covered from 2026-09-29. Each
    # needs a docker daemon, a git remote or a package registry — three things
    # `check.sh` promises never to need, which is what makes it runnable on a
    # branch with no network.
    "It starts inside the image, and validates a configuration": (
        "runs the image the step above built, in a docker daemon"
    ),
    "The base is still distroless": "runs the image, in a docker daemon",
    "The whole suite over gRPC-web": (
        "the TypeScript client's suite a second time with SLATE_TRANSPORT=web, "
        "which needs npm, node_modules and a built head node, none of which "
        "check.sh may require"
    ),
    "The tarball contains what the package promises": (
        "runs `npm pack`, which needs npm and an installed node_modules. Its "
        "rules are covered in check.sh by scripts/test_check_npm_package.py, "
        "which stubs that call out."
    ),
    "The binary inside is built for the architecture on the tin": (
        "inspects and runs the built image, in a docker daemon. It is the "
        "cross-compile's own check: a Dockerfile that lost its "
        "`--platform=$BUILDPLATFORM` handling still builds and still tags the "
        "result arm64, and only the architecture of the binary inside says so."
    ),
    "Tag the module path": "pushes a git tag to the remote",
    "Warm the proxy, and fail if it will not resolve": "asks proxy.golang.org",
}


#: Workflow-level `env:` variables `check.sh` deliberately does not export.
#:
#: Same shape and same reason as `ELSEWHERE`: a variable that changes what a
#: check *means* has to be set here too, and one that does not still has to be
#: named, so that deciding which it is happens once rather than never.
ENV_ELSEWHERE: dict[str, str] = {}


def workflow_env() -> dict[str, str]:
    """Every workflow's top-level `env:` block, as name to value.

    Only the top-level block, which is the one that applies to every step and
    is therefore the one nothing in a `run:` line reveals. A `env:` nested
    under a job or a step sits beside the command it modifies, where a reader
    comparing the two files can see it.

    **Every workflow, not `ci.yml` alone.** It read one file while the step
    roster above read five, which
    `ledger/2026-09-29-a-rule-scoped-to-one-file-is-a-rule-about-that-file.md`
    recorded as the same hole one level down: `mutations.yml` growing a
    workflow-level `env:` would have changed what its steps mean and nothing
    would have noticed. Only `ci.yml` has one today, so this returns the same
    dictionary it did — which is the point at which a widening is cheap.

    A name set to two different values by two workflows is reported rather
    than silently taking the last, because `check.sh` exports one value and
    cannot satisfy both.
    """
    found: dict[str, str] = {}
    for path in workflows():
        inside = False
        for line in path.read_text().splitlines():
            if line.rstrip() == "env:":
                inside = True
                continue
            if not inside:
                continue
            entry = re.match(r"^  ([A-Za-z_][A-Za-z0-9_]*): (.+)$", line)
            if entry:
                name, value = entry.group(1), entry.group(2).strip()
                if name in found and found[name] != value:
                    # Reported through the value itself, so the complaint the
                    # caller builds names both. A separate channel would mean
                    # threading a second list through a function whose one job
                    # is to read a block.
                    value = f"{found[name]!r} in one workflow and {value!r} in another"
                found[name] = value
                continue
            # The block ends at the first line that is not one of its entries,
            # which in these files is the blank line before `jobs:`.
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
    """Complaints about `check.sh`'s exports against every workflow's `env:`."""
    wanted = workflow_env()
    if not wanted:
        # Reported, not asserted — the same reason the three never-fires
        # checks below are reported: an `AssertionError` kills the run before
        # the summary line `scripts/mutate.py` reads, so a guard correctly
        # refusing scores as a suite that never started.
        return ["no workflow sets a top-level env: block; this check is blind"]
    exported = script_env()
    complaints = []
    for name, value in sorted(wanted.items()):
        if name in ENV_ELSEWHERE:
            continue
        if name not in exported:
            complaints.append(
                f"a workflow sets {name}={value} for every step and check.sh does not "
                f"export it, so the same command is a different check in the two "
                f"places; export it or give it a reason in ENV_ELSEWHERE"
            )
        elif exported[name] != value:
            complaints.append(
                f"a workflow sets {name}={value} and check.sh exports {name}={exported[name]}"
            )
    for name in sorted(set(ENV_ELSEWHERE) - set(wanted)):
        complaints.append(f"ENV_ELSEWHERE names {name}, which no workflow sets")
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
    both, _ = workflow_steps([*TWO_FILES, ["      - run: second-command"]])
    if len(both) <= len(one):
        said.append("a second workflow's steps are not read; the glob or the loop is gone")

    return said


#: How `CLAUDE.md` spells a job count, and the numbers it could mean.
#:
#: Words rather than digits because that file writes prose, and only the range
#: a repository plausibly occupies: a list long enough to cover every number is
#: a list nobody maintains, and one too short fails loudly the day a
#: twenty-sixth job lands, which is the right failure.
COUNTED = {
    # `check.sh` says how many jobs hold a static check, which is a different
    # quantity from the job total and lives lower: eleven of twenty-three
    # today. The map covers both because the alternative is two maps of number
    # words, and a second one would go stale on its own schedule.
    "one": 1,
    "two": 2,
    "three": 3,
    "four": 4,
    "five": 5,
    "six": 6,
    "seven": 7,
    "eight": 8,
    "nine": 9,
    "ten": 10,
    "eleven": 11,
    "twelve": 12,
    "thirteen": 13,
    "fourteen": 14,
    "fifteen": 15,
    "sixteen": 16,
    "seventeen": 17,
    "eighteen": 18,
    "nineteen": 19,
    "twenty": 20,
    "twenty-one": 21,
    "twenty-two": 22,
    "twenty-three": 23,
    "twenty-four": 24,
    "twenty-five": 25,
}

#: `  job-name:` at the top level of a workflow's `jobs:` block.
#:
#: The expression `CLAUDE.md` prints for itself, kept here so the count and the
#: sentence cannot disagree about what is being counted.
A_JOB = re.compile(r"^  ([a-z][a-z0-9-]*):\s*$", re.M)


#: Where CLAUDE.md states the job count, and how to read each one.
#:
#: `(what, pattern)`, one per occurrence, each capturing the spelled number.
#: A list rather than one pattern because the two sentences are different
#: sentences — "twenty-four jobs covering formatting…" in *What runs, and
#: where*, and "A push starts twenty-four jobs and nothing in this container
#: reports how they ended" in the practical notes — and the drift this exists
#: for hit *one* of two occurrences twice. Checking them separately names
#: which one is wrong.
CLAUDE_MD_COUNTS: list[tuple[str, str]] = [
    ("`What runs, and where` sentence", r"\*\* — ([a-z]+(?:-[a-z]+)?) jobs covering"),
    ("`Read the run's conclusion` note", r"A push starts\s+([a-z]+(?:-[a-z]+)?) jobs"),
]


#: Every tracked file that could describe CI's job count, in words.
#:
#: Not `ledger/`: an entry is dated and append-only, so "twenty-three of
#: twenty-four jobs green" in one is a record of a run and is *supposed* to
#: stay at the number it was. Not `clients/` or `crates/` either — nothing
#: there describes the workflow — but the sweep is cheap and widening it is
#: one tuple.
SWEPT_FOR_A_JOB_COUNT = ("README.md", "CLAUDE.md", "docs/", "site/", "scripts/", ".github/")
SWEPT_SUFFIXES = (".md", ".sh", ".py", ".yml", ".yaml", ".html", ".txt")

#: `<a spelled number> jobs`, anywhere in a swept file.
#:
#: Built from `COUNTED`'s own keys rather than from `[a-z]+`, which is the
#: whole difference between this and the loop
#: `ledger/2026-09-29-the-file-named-after-the-guard-carried-the-stale-count.md`
#: records as crying wolf: "the jobs that find the interesting failures" does
#: not match, because `the` is not a number. Only a sentence stating a count
#: is read as one.
JOB_COUNT_CLAIM = re.compile(
    r"\b(" + "|".join(sorted(COUNTED, key=len, reverse=True)) + r") jobs\b"
)

#: Phrases that say `<number> jobs` and are not a claim about today's CI.
#:
#: `{(file, phrase): why}`. Checked in **both** directions, like
#: `check_toolchain_pins.py`'s `NOT_AN_INSTALLER` and for the reason that
#: entry gives: an exemption for a phrase that is gone is a line a reader
#: takes for a live decision, and if the phrase comes back meaning something
#: else the exemption hides it.
NOT_THE_JOB_COUNT: dict[tuple[str, str], str] = {
    (
        "scripts/check_workspace.py",
        "twenty-two jobs",
    ): "narration of CI run 532, which really did have twenty-two green and one "
    "red. A past run's score does not move when the job list does.",
    (
        "scripts/test_check_sh.py",
        "twenty-four jobs",
    ): "this file quoting the two CLAUDE.md sentences it anchors, so that a "
    "reader of CLAUDE_MD_COUNTS can see what the patterns are matching. It is "
    "the pattern's documentation, the way check_toolchain_pins.py's own "
    "example is, and it moves when they do because it is quoting them.",
    (
        "scripts/test_check_sh.py",
        "seventeen jobs",
    ): "narration of the drift this function was written for — check.sh said "
    "seventeen against twenty-three. The wrong number is the point.",
    (
        "scripts/test_check_sh.py",
        "twenty-two jobs",
    ): "this table quoting the phrase it excuses in check_workspace.py. A "
    "roster of exemptions has to name what it excuses, so the roster itself "
    "matches the pattern — the same self-reference that makes "
    "check_toolchain_pins.py skip its own file by name. Found by running the "
    "sweep, which reported the exemption as an unanchored claim.",
}


def jobs_sharing_the_prebuilt_binary() -> int:
    """How many `ci.yml` jobs run against the binary the `build` job uploads.

    `scripts/prebuilt.py` says CI "shares it across N jobs", which is the
    third file stating a job count — the one
    `ledger/2026-09-29-the-file-named-after-the-guard-carried-the-stale-count.md`
    predicted and nothing looked for:

      > A third file describing it in words that match neither pattern is
      > exactly as stale as these two were, and nothing looked.

    It said *eight* against six. Derived here rather than counted by hand,
    from the two variables `prebuilt.py` is about, so the sentence and the
    workflow cannot disagree.
    """
    text = (ROOT / ".github/workflows/ci.yml").read_text()
    holding: set[str] = set()
    current: str | None = None
    for line in text.splitlines():
        head = A_JOB.match(line)
        if head:
            current = head.group(1)
            continue
        if current and re.search(r"\bSLATE_(?:SERVERD|TESTSERVER):", line):
            holding.add(current)
    return len(holding)


def every_job_count_is_anchored() -> list[str]:
    """Every `<number> jobs` in a swept file is checked, or exempt with a reason.

    The two counts this file already anchors were found by grepping for
    `test_check_sh` and for `seventeen`, which is a sweep that finds the
    files somebody thought of. This is the sweep that finds the rest: a file
    stating a job count in any words at all fails until the count is either
    held to `ci.yml` or written off here.

    It found one immediately, which is the evidence that the caveat was
    right: `scripts/prebuilt.py` said CI shares the prebuilt binary across
    *eight* jobs and the number is six.
    """
    anchored = {
        "CLAUDE.md": [pattern for _, pattern in CLAUDE_MD_COUNTS],
        "scripts/check.sh": [
            r"`ci\.yml` is ([a-z]+(?:-[a-z]+)?) jobs",
            r"spread across ([a-z]+(?:-[a-z]+)?) of them",
        ],
        "scripts/prebuilt.py": [r"shares it across\s+([a-z]+(?:-[a-z]+)?) jobs"],
    }
    said: list[str] = []
    seen: set[tuple[str, str]] = set()
    listing = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
    )
    for name in sorted(listing.stdout.split()):
        if not name.startswith(SWEPT_FOR_A_JOB_COUNT):
            continue
        if not name.endswith(SWEPT_SUFFIXES):
            continue
        try:
            text = (ROOT / name).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        # `check.sh`'s prose is a comment block, so a sentence spans lines
        # with a `# ` between them — the same unwrapping `job_count_matches`
        # does, for the same reason.
        unwrapped = re.sub(r"\n#\s*", " ", text)
        covered = [
            span
            for pattern in anchored.get(name, [])
            for span in (found.span() for found in re.finditer(pattern, unwrapped))
        ]
        for claim in JOB_COUNT_CLAIM.finditer(unwrapped):
            phrase = claim.group(0)
            if any(start <= claim.start() and claim.end() <= end for start, end in covered):
                continue
            if (name, phrase) in NOT_THE_JOB_COUNT:
                seen.add((name, phrase))
                continue
            said.append(
                f"{name} says {phrase!r} and nothing holds it to ci.yml. "
                f"A count stated in prose beside a list that grows is how "
                f"CLAUDE.md said 'seventeen' for weeks. Anchor it in this "
                f"file's `anchored` table, or add it to NOT_THE_JOB_COUNT "
                f"with the reason it is not a claim about today's CI."
            )
    said.extend(
        f"NOT_THE_JOB_COUNT excuses {phrase!r} in {name}, which no longer says it. "
        f"An exemption for a phrase that is gone reads as a live decision, and "
        f"hides the phrase if it comes back meaning something else."
        for name, phrase in sorted(set(NOT_THE_JOB_COUNT) - seen)
    )
    return said


def jobs_holding_a_static_check(steps: list[tuple[str, str]]) -> int:
    """How many `ci.yml` jobs run at least one step `check.sh` also runs.

    `check.sh`'s own header says the static checks are "spread across N of
    them", which is the second number in that file with the same weakness as
    the first — prose beside a thing that grows. It said *six* against eleven.

    Derived rather than counted by hand, and from the same `script_checks()`
    the rules above use, so the sentence and the roster cannot disagree about
    what is being counted. `SPELLED_IN_CI`'s values are included because a job
    whose only overlap is the `gofmt` wrapper is still a job holding a check
    the script runs.
    """
    commands = {command for _, command in script_checks()} | set(SPELLED_IN_CI.values())
    text = (ROOT / ".github/workflows/ci.yml").read_text()
    holding: set[str] = set()
    current: str | None = None
    for line in text.splitlines():
        head = A_JOB.match(line)
        if head:
            current = head.group(1)
            continue
        step = re.match(r"^\s*- run: (.+)$", line)
        if step and current and step.group(1).strip() in commands:
            holding.add(current)
    return len(holding)


def job_count_matches(steps: list[tuple[str, str]]) -> list[str]:
    """Complaints about `CLAUDE.md`'s job count against `ci.yml`.

    `CLAUDE.md` says how many jobs CI runs, twice, and says of itself:

    > That count is a count of `jobs:` keys in `ci.yml` and nothing checks it,
    > which is why it said *seventeen* for as long as it did: jobs were added
    > and the sentence was not.

    It then drifted a second time, to *twenty-one* against twenty-two, and was
    corrected by hand in
    `ledger/2026-09-29-i-filtered-the-summary-line-out-of-my-own-check.md`.
    Twice is the count this repository uses to decide a habit needs a check.

    Both occurrences, not the first: they are sentences apart and a correction
    that reached one and not the other is the failure mode
    `check_retired_claims.py` exists for.
    """
    jobs = len(A_JOB.findall((ROOT / ".github/workflows/ci.yml").read_text()))
    if not jobs:
        return ["no jobs found in ci.yml; the pattern that counts them is blind"]

    prose = (ROOT / "CLAUDE.md").read_text()
    said = []
    # Anchored, like `check.sh`'s half below and unlike this half's first
    # version, which looped over every `<word> jobs` in the file.
    # `ledger/2026-09-29-the-file-named-after-the-guard-carried-the-stale-count.md`
    # recorded that asymmetry as a caveat: the loop has no way to tell a count
    # from ordinary English, so the day CLAUDE.md writes "the jobs that find
    # the interesting failures" the guard reports `'the'` as a number it does
    # not recognise. A guard that cries wolf is one people stop reading.
    #
    # Every anchor must match. A reworded sentence therefore fails *loudly* —
    # "the sentence moved, put this back on it" — rather than silently
    # checking nothing, which is the failure mode the unanchored loop could
    # not have and is the price of anchoring. Both halves now behave the same
    # way, which is the point.
    for what, pattern in CLAUDE_MD_COUNTS:
        found = re.search(pattern, prose)
        if not found:
            said.append(
                f"CLAUDE.md's {what} no longer matches, so this check has "
                f"nothing to hold to the workflow. Either the sentence was "
                f"reworded — update the pattern in CLAUDE_MD_COUNTS — or it "
                f"is gone and so is the reason for this."
            )
            continue
        word = found.group(1)
        if word not in COUNTED:
            said.append(
                f"CLAUDE.md's {what} says {word!r} jobs, which is not a number "
                f"COUNTED knows; add it if CI really has that many"
            )
        elif COUNTED[word] != jobs:
            said.append(
                f"CLAUDE.md's {what} says {word!r} jobs and ci.yml has {jobs}. "
                f"The count has drifted twice before; correct every "
                f"occurrence, not the first."
            )

    # `check.sh`'s own header carries the same two numbers and nothing read
    # them. It said "seventeen jobs" against twenty-three and "spread across
    # six of them" against eleven — the identical drift this function was
    # written for, in the file the function is named after, found only because
    # somebody grepped for the word `seventeen`.
    # Unwrapped first: `check.sh` is a shell file whose prose is a comment
    # block, so a sentence spans lines with a `# ` between them and a naive
    # match sees "six" and "of them" as unrelated. The first draft of this
    # missed the `spread across` sentence entirely for exactly that reason,
    # and reported it as gone rather than as wrong — a guard saying "the
    # sentence moved" about a sentence that is still there.
    script = re.sub(r"\n#\s*", " ", (ROOT / "scripts/check.sh").read_text())

    # Anchored on the claim rather than on the word "jobs", unlike
    # `CLAUDE.md`'s loop above, because this file uses "jobs" as an ordinary
    # noun too: "the jobs that find the *interesting* failures" is prose. A
    # loop over every word before "jobs" reported `'the'` as a number it did
    # not recognise, which is a guard crying wolf on English — and a guard
    # that cries wolf is one people stop reading. Anchoring keeps the
    # unknown-word case, which matters: a count past what `COUNTED` knows has
    # to fail loudly rather than pass quietly.
    claim = re.search(r"`ci\.yml` is ([a-z]+(?:-[a-z]+)?) jobs", script)
    if not claim:
        said.append(
            "check.sh no longer says how many jobs ci.yml has, so this half "
            "has nothing to hold to. Either the sentence moved — put this back "
            "on it — or it is gone and so is the reason for this."
        )
    elif claim.group(1) not in COUNTED:
        said.append(
            f"check.sh says ci.yml is {claim.group(1)!r} jobs, which is not a "
            f"number COUNTED knows; add it if CI really has that many"
        )
    elif COUNTED[claim.group(1)] != jobs:
        said.append(f"check.sh says {claim.group(1)!r} jobs and ci.yml has {jobs}")

    # The third file, found by the sweep in `every_job_count_is_anchored`
    # and anchored here rather than exempted: it is a live claim about
    # `ci.yml`, so it belongs with the other two.
    shared = jobs_sharing_the_prebuilt_binary()
    prebuilt = re.sub(r"\n#\s*", " ", (ROOT / "scripts/prebuilt.py").read_text())
    across = re.search(r"shares it across\s+([a-z]+(?:-[a-z]+)?) jobs", prebuilt)
    if not across:
        said.append(
            "prebuilt.py no longer says how many jobs share the binary, so "
            "this half has nothing to hold to. Either the sentence moved — "
            "put this back on it — or it is gone and so is the reason for this."
        )
    elif across.group(1) not in COUNTED:
        said.append(
            f"prebuilt.py says the binary is shared across {across.group(1)!r} "
            f"jobs, which is not a number COUNTED knows"
        )
    elif COUNTED[across.group(1)] != shared:
        said.append(
            f"prebuilt.py says {across.group(1)!r} jobs share the prebuilt "
            f"binary and {shared} set SLATE_SERVERD or SLATE_TESTSERVER"
        )

    holding = jobs_holding_a_static_check(steps)
    spread = re.search(r"spread across ([a-z]+(?:-[a-z]+)?) of them", script)
    if not spread:
        said.append(
            "check.sh no longer says how many jobs hold the static checks, so "
            "this half has nothing to hold to. Either the sentence moved — put "
            "this back on it — or it is gone and so is the reason for this."
        )
    elif spread.group(1) not in COUNTED:
        said.append(
            f"check.sh says the checks are spread across {spread.group(1)!r} "
            f"jobs, which is not a number COUNTED knows"
        )
    elif COUNTED[spread.group(1)] != holding:
        said.append(
            f"check.sh says the checks are spread across {spread.group(1)!r} "
            f"jobs and {holding} of ci.yml's jobs run one"
        )
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
    # Both directions now. This comment used to say the script running checks
    # CI does not was "allowed and is the point of having it", and named the
    # four it meant. It was excusing fourteen: the four it named, plus nine
    # repository guards, plus one more `tsc`. See `ONLY_LOCAL`.
    only_local, stale_local = unrun_locally(steps, covered)
    # The same pair read the other way: a workflow spelling that `SPELLED_IN_CI`
    # already ties to a `check.sh` step is covered, and reporting it here would
    # be the one mapping contradicting itself.
    spelled = {(where[0], ci) for where, ci in SPELLED_IN_CI.items()}
    missing = [
        (directory, command)
        for directory, command in steps
        if (directory, command) not in covered
        and (directory, command) not in spelled
        and command not in ELSEWHERE
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
            "check.sh exports every workflow's top-level env",
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
            "CLAUDE.md's job count is ci.yml's",
            job_count_matches(steps),
        ),
        (
            "every job count stated in prose is anchored or excused",
            every_job_count_is_anchored(),
        ),
        (
            "every check.sh step runs in a workflow or is in ONLY_LOCAL",
            only_local,
        ),
        (
            "no ELSEWHERE entry names a step the workflows have dropped",
            [f"ELSEWHERE names a step no workflow has: {c}" for c in stale_elsewhere]
            + [f"ELSEWHERE_BLOCKS names a block no workflow has: {n}" for n in stale_blocks],
        ),
        (
            "no ONLY_LOCAL or SPELLED_IN_CI entry names a step check.sh has dropped",
            stale_local,
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
