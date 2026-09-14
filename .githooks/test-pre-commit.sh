#!/bin/sh
#
# Tests for the pre-commit hook.
#
# The hook had no test, and the first thing it did in anger was reject every
# commit in the repository with no message at all: `set -e` plus a trailing
# `[ ... ] && printf` whose test was false on the last file. A guard that fails
# closed and silent is worse than no guard, because the failure looks like the
# check working.
#
# Each case builds a throwaway repository, stages something, and runs the hook
# the way git runs it -- from the work tree, with the index already written.
# `sh` and `git` only: the hook itself has no dependencies and neither should
# the thing that proves it works.

set -eu

hook=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/pre-commit
[ -x "$hook" ] || { echo "no hook at $hook" >&2; exit 1; }

passed=0
failed=0

# Per-case size limit for the guard. Empty means the hook's own default, which
# `${VAR:-default}` restores. The size cases set it rather than the caller: a
# suite that only tests what it tests when you remember an environment
# variable is a suite that passes for the wrong reason.
limit=

# Run the hook in a fresh repository. $1 is a shell fragment that stages the
# commit under test; $2 is "allow" or "refuse"; $3 names the case.
check() {
    setup=$1 want=$2 name=$3
    work=$(mktemp -d)
    (
        cd "$work"
        git init -q .
        git config user.email t@example.com
        git config user.name t
        mkdir ledger
        eval "$setup"
    ) >/dev/null 2>&1

    out=$(cd "$work" && SLATE_HOOK_SIZE_LIMIT="$limit" sh "$hook" 2>&1) &&
        got=allow || got=refuse

    if [ "$got" = "$want" ]; then
        passed=$((passed + 1))
        printf '  ok    %s\n' "$name"
    else
        failed=$((failed + 1))
        printf '  FAIL  %s: wanted %s, got %s\n' "$name" "$want" "$got"
        printf '%s\n' "$out" | sed 's/^/          /'
    fi

    # A refusal has to say something. The bug this suite exists for was a
    # refusal with an empty message, which is indistinguishable from a crash.
    if [ "$got" = refuse ] && [ -z "$(printf '%s' "$out" | tr -d '[:space:]')" ]; then
        failed=$((failed + 1))
        printf '  FAIL  %s: refused with no message\n' "$name"
    fi

    rm -rf "$work"
}

entry() {
    cat <<'ENTRY'
# A change

## What changed
A thing.

## Why
Because.

## Alternatives rejected
The other thing, which would have cost more.

## Evidence
A test.

## What this does not do
Anything else.
ENTRY
}

echo "pre-commit:"

check 'echo x > ledger/2026-01-01-a.md; git add ledger' \
    allow 'a commit touching only the ledger'

check 'echo x > src.txt; git add src.txt' \
    refuse 'a change outside the ledger with no entry'

check 'echo x > src.txt; echo y > ledger/notes.md; git add .' \
    refuse 'a ledger file that is not a dated entry'

check 'echo x > src.txt; entry > ledger/2026-01-01-a.md; git add .' \
    allow 'a change with a complete entry'

check 'echo x > src.txt
       entry | grep -v "^A test\.$" > ledger/2026-01-01-a.md
       git add .' \
    refuse 'an entry with an empty section'

check 'echo x > src.txt
       entry | grep -v "^## Alternatives rejected$" > ledger/2026-01-01-a.md
       git add .' \
    refuse 'an entry missing a section'

check 'echo x > src.txt
       entry > ledger/2026-01-01-a.md
       echo "One paragraph. The diff has the detail" >> ledger/2026-01-01-a.md
       git add .' \
    refuse 'an entry still carrying the template prose'

check 'echo x > src.txt
       entry > ledger/2026-01-01-a.md
       git add .
       : > .git/MERGE_HEAD' \
    allow 'a merge, which has no reasoning of its own'

check 'echo x > src.txt; git add src.txt
       echo "Revert \"a thing\"" > .git/COMMIT_EDITMSG' \
    allow 'a revert, whose entry already exists'

# --- the size guard --------------------------------------------------------
#
# Both directions matter. The regression that prompted this suite was the
# *under* the limit case: the guard passed and killed the script anyway.

limit=2048

check 'entry > ledger/2026-01-01-a.md
       head -c 4096 /dev/zero | tr "\0" "z" > big.txt
       git add .' \
    refuse 'a file over the limit'

check 'entry > ledger/2026-01-01-a.md
       echo small > small.txt
       git add .' \
    allow 'a file under the limit'

limit=

# A deletion stages a path that is not on disk; the guard must skip it rather
# than fail reading it.
check 'echo x > gone.txt; entry > ledger/2026-01-01-a.md; git add .
       git -c core.hooksPath=/dev/null commit -qm first
       git rm -q gone.txt
       entry > ledger/2026-01-02-b.md; git add .' \
    allow 'a deletion, whose path is not on disk'

echo
printf '%s passed, %s failed\n' "$passed" "$failed"
[ "$failed" -eq 0 ]
