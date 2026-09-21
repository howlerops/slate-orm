#!/bin/sh
# Every static check CI runs, in one command.
#
# # Why this exists
#
# `ci.yml` is seventeen jobs and the static checks in it are spread across six
# of them, in four languages, from five working directories. A session that runs
# "the lint" runs one of them. Three commits in one afternoon went red on a
# check that existed, was cheap, and was run from the wrong directory or not at
# all:
#
#   - `ruff check .` at the repository root passed while `ruff check .` in
#     `clients/python` failed. They are two runs with two configurations over
#     disjoint trees, and `CLAUDE.md` said so in a sentence that is easy to read
#     as a description rather than as a warning.
#   - `pytest tests/test_details.py` passed while the suite failed, on a change
#     to a module the suite depends on.
#   - `cargo clippy -p <one crate>` passed while `--workspace --all-targets`
#     failed.
#
# Each was a different check and the same mistake. So: one command, everything
# it can reach, and a list at the end of what it deliberately cannot.
#
# # What it does not do
#
# Nothing here needs a built server binary, a browser, a container or the
# network, which is what keeps it worth running before every commit. That rules
# out the client test suites, the conformance runner, the e2e, the workbench,
# MinIO and the deployed harness — the jobs that find the *interesting*
# failures. A green run here is necessary and nowhere near sufficient, and the
# closing summary says so rather than leaving it implied.
#
# # Behaviour
#
# Runs everything and reports at the end, rather than stopping at the first
# failure: a session that fixes one check and re-runs pays the whole cost again
# to find the second. Exit status is the number of checks that failed, capped
# the way a shell caps it.
#
#   sh scripts/check.sh            every check
#   sh scripts/check.sh --list     name them and exit
#
# `SLATE_CI_ENV` (default `/tmp/ci-env`) is the virtualenv the repository-wide
# `ty` runs against. It is created on first use, because `ty` resolves imports
# against whatever `site-packages` it can see and CI has almost none — a bare
# `ty check` in this container passes against packages CI does not install,
# which is how a script importing `pyarrow` went red on a file nobody touched.

set -u

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT" || exit 1

CI_ENV=${SLATE_CI_ENV:-/tmp/ci-env}
FAILED=""
PASSED=0

# `ci.yml` sets these once at workflow level, so every step in every job runs
# under them and no individual `run:` line mentions them. That invisibility is
# the point of exporting them here: `RUSTFLAGS=-D warnings` turns a *warning*
# into a failed job, so `cargo clippy --workspace --all-targets` run without it
# is a materially weaker check than the identical command in CI — and the
# command text, which is all `test_check_sh.py` could compare, is identical.
#
# That gap is not theoretical. `clippy::indexing_slicing` on a `words[i % 16]`
# warned here, exited zero, and failed the `clippy, test` job. The local run
# was green and the command was the same one.
#
# `scripts/test_check_sh.py` now holds this list to `ci.yml`'s own `env:`
# block, so a variable added there fails a test until it is either exported
# here or given a reason it cannot be.
export CARGO_TERM_COLOR=always
export RUSTFLAGS="-D warnings"

# Each check is a name, a directory and a command. Kept as one list so that
# `--list` and the runner cannot disagree about what "every check" means.
checks() {
    cat <<'LIST'
rust-fmt|.|cargo fmt --all -- --check
rust-clippy|.|cargo clippy --workspace --all-targets
rust-clippy-json|.|cargo clippy -p slate-orm --features json --all-targets
rust-doc|.|cargo doc --workspace --no-deps
workspace-guard|.|python3 scripts/check_workspace.py
python-client-ty|clients/python|ty check
python-client-ruff|clients/python|ruff check .
python-rest-ty|.|ty check
python-rest-ruff|.|ruff check .
codegen-tests|.|python3 scripts/test_codegen.py
python-decoders|.|python3 -m pytest examples/explorer/backends/python/adapter -q
check-sh-guard|.|python3 scripts/test_check_sh.py
mutate-harness|.|python3 scripts/test_mutate.py
cited-tests|.|python3 scripts/check_cited_tests.py
cited-tests-guard|.|python3 scripts/test_check_cited_tests.py
cited-docs|.|python3 scripts/check_cited_docs.py
cited-docs-guard|.|python3 scripts/test_check_cited_docs.py
handler-auth|.|python3 scripts/check_handlers.py
handler-auth-guard|.|python3 scripts/test_check_handlers.py
write-paths|.|python3 scripts/check_write_paths.py
write-paths-guard|.|python3 scripts/test_check_write_paths.py
client-identity|.|python3 scripts/check_client_identity.py
client-identity-guard|.|python3 scripts/test_check_client_identity.py
go-client-fmt|clients/go|gofmt -l .
go-client-vet|clients/go|go vet ./...
go-adapter-fmt|examples/explorer/backends/go|gofmt -l .
go-adapter-vet|examples/explorer/backends/go|go vet ./...
ts-client-types|clients/typescript|npx --no-install tsc -p tsconfig.json --noEmit
ts-client-build|clients/typescript|npm run build
ts-adapter-types|examples/explorer/backends/node|npx --no-install tsc -p tsconfig.json --noEmit
web-types|examples/explorer/web|npm run typecheck
hook-suite|.|sh .githooks/test-pre-commit.sh
LIST
}

if [ "${1:-}" = "--list" ]; then
    checks | while IFS='|' read -r name dir command; do
        printf '%-20s %s\n' "$name" "(in $dir) $command"
    done
    exit 0
fi

# A virtualenv holding exactly what CI's `scripts` job installs and nothing
# else. Two checks need it, for the same underlying reason: this container has
# whatever `site-packages` it happens to have, and CI has almost none, so a
# check run against the ambient interpreter is a different check.
ci_env() {
    if [ ! -x "$CI_ENV/bin/ty" ]; then
        printf 'building %s (once)\n' "$CI_ENV"
        python3 -m venv "$CI_ENV" >/dev/null || return 1
        "$CI_ENV/bin/pip" install -q -e './clients/python[dev]' >/dev/null || return 1
    fi
}

# `ty` for everything outside `clients/python`.
ci_env_ty() {
    ci_env || return 1
    "$CI_ENV/bin/ty" check --python "$CI_ENV"
}

# The demo's generated Python decoders. In the virtualenv rather than the
# ambient interpreter because the test imports `slate`, and a run that picked
# up some other copy of it would be testing something else.
ci_env_pytest() {
    ci_env || return 1
    "$CI_ENV/bin/python" -m pytest examples/explorer/backends/python/adapter -q
}

# The TypeScript client, built, *before* the adapter is type-checked.
#
# Not a check of its own in spirit — it is a precondition, and it is here
# because leaving it out cost a red CI job on a green local run. The demo's
# Node adapter imports `@slate-orm/client`, which resolves to
# `clients/typescript/dist`, and `dist` is a *build artifact*: the adapter was
# type-checked against whatever was last built there. Adding a variant to
# `Value` made the adapter's `switch` non-exhaustive, and locally the switch
# still looked exhaustive because `dist/value.d.ts` predated the variant. CI
# builds first and found it.
#
# The same class as the `SLATE_SERVERD` staleness guard: a prebuilt artifact
# older than its source turns a suite into a test of the past. That one
# compares modification times and refuses; this one rebuilds, which is cheaper
# than a second staleness rule and removes the question.
#
# `gofmt -l` prints the files it would change and exits zero either way, so a
# check on it has to look at the output rather than at the status.
go_fmt() {
    unformatted=$(gofmt -l .)
    if [ -n "$unformatted" ]; then
        printf 'gofmt would rewrite:\n%s\n' "$unformatted"
        return 1
    fi
}

run() {
    name=$1
    dir=$2
    command=$3
    printf '\n=== %s (in %s)\n' "$name" "$dir"
    (
        cd "$ROOT/$dir" || exit 1
        # Special-cased by *name*, not by a sentinel in the command column, so
        # that the column holds the command `ci.yml` holds and
        # `test_check_sh.py` can compare the two literally.
        case $name in
            python-rest-ty) ci_env_ty ;;
            python-decoders) ci_env_pytest ;;
            *-fmt) go_fmt ;;
            *) eval "$command" ;;
        esac
    )
    if [ $? -eq 0 ]; then
        PASSED=$((PASSED + 1))
    else
        FAILED="$FAILED $name"
    fi
}

# A here-document rather than a pipe, because a pipeline runs its body in a
# subshell and `FAILED` would come back empty — the failure mode where this
# script reports success for a run that failed everything.
while IFS='|' read -r name dir command; do
    [ -n "$name" ] || continue
    run "$name" "$dir" "$command"
done <<LIST
$(checks)
LIST

printf '\n----------------------------------------\n'
if [ -n "$FAILED" ]; then
    printf '%d passed, FAILED:%s\n' "$PASSED" "$FAILED"
else
    printf '%d passed, all of them\n' "$PASSED"
fi
cat <<'CAVEAT'

Not covered here, and each has found a real defect: the Go, Python and
TypeScript client suites, the three-SDK conformance runner, the browser e2e,
the workbench, the MinIO integration, the deployed harness, the site's
quickstarts, and `cargo test --workspace`. They need a built binary, a browser
or a container. Run the ones your change touches.
CAVEAT

[ -z "$FAILED" ]
