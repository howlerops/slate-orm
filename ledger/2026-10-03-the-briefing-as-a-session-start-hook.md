# `handoff.py --hook`, and the permission change I was refused

- **Date:** 2026-10-03
- **Author:** Claude Code (session: what's next, after the release)
- **Touches:** `scripts/handoff.py`, `scripts/test_handoff.py`
- **Kind:** feature

## What changed

`scripts/handoff.py --hook` emits the briefing as a `SessionStart` hook
result, so a session is briefed before anybody thinks to ask. One flag, 5.6 KB
of context at `--entries 5`.

**The settings change it exists for is not in this commit**, because the
permission classifier refused it. That half is below, written out, for a
person to apply.

## Why

`.claude/settings.json` already runs a `SessionStart` hook — it points
`core.hooksPath` at `.githooks` so the ledger hook is live in a fresh clone.
Adding the briefing beside it means a cold session, local or cloud, starts
knowing where the tree is rather than reading `CLAUDE.md` and guessing. That
is the whole value of
`ledger/2026-10-03-a-briefing-that-reads-the-tree.md` actually being
delivered rather than merely available.

**The JSON wrapper is a flag rather than a shell pipeline** because the
alternative is building JSON inside a JSON string inside a settings file:
three levels of quoting, no test, and a failure mode — a hook that emits
something unparseable — that is silent. `--hook` is one flag with two cases in
the suite, one of which pins the field name.

**`additionalContext` and not bare stdout.** That is the field a
`SessionStart` hook result is read from. A briefing printed somewhere nothing
reads it is the never-fires failure this repository has met repeatedly, and
it would be invisible: the session would simply be uninformed, which looks
exactly like no hook at all.

## The refusal, and why it is right

The settings change was denied by the auto-mode permission classifier with
the reason **`[Self-Modification]`**.

It is correct, and worth recording rather than grumbling about. The change
would have granted this agent authority to push `v*` tags unattended, written
by the agent, into a file the agent is the main consumer of. An agent that
can widen its own permissions has no permissions. That the classifier catches
it in a repository where the user had *just agreed* to the widening is the
system working: consent given in conversation is not the same artifact as
consent committed to a file, and only a person can supply the second.

So the file is unchanged and the content is below.

## Alternatives rejected

**`.claude/settings.local.json` instead.** Gitignored, personal, and it would
have avoided committing agent authority to a shared file. It does not work
for half of what was asked: a cloud session clones the repository fresh, so a
gitignored file does not exist there. Committed settings are the only kind
that reach both places, and that is a real cost of "both" rather than a
detail — anyone who clones this repository gets these rules.

**Allowing `Bash(git push *)` and denying the dangerous shapes.** One line
instead of thirteen. Rejected because the allowlist is the safer direction to
be wrong in: an unforeseen `git push` spelling should stop and ask, not
proceed. With a blanket allow plus denials, anything the deny list failed to
imagine is permitted.

**`deny` rather than `ask` for force-pushes.** Stronger, and it would stop the
user too — a denial cannot be cleared in-session, so a legitimate recovery
would mean editing settings under pressure. `ask` keeps the human in the loop
without making them fight the file. It matches what was chosen.

**Writing the settings to some other path and telling the user to move it.**
Would have produced the same outcome the refusal names, one step removed.
The content goes in this entry and in chat; applying it is a person's act.

## What to apply

Replacing `.claude/settings.json` with this adds the permissions and keeps
the existing hook:

```json
{
  "permissions": {
    "allow": [
      "Bash(git push -u origin claude/*)",
      "Bash(git push origin claude/*)",
      "Bash(git push -u origin main)",
      "Bash(git push origin main)",
      "Bash(git push origin v*)",
      "Bash(git tag -a v*)",
      "Bash(git tag v*)",
      "Bash(git merge --ff-only *)",
      "Bash(git merge --no-ff *)",
      "Bash(git fetch *)",
      "Bash(python3 scripts/handoff.py*)",
      "Bash(python3 scripts/reclaim.py*)",
      "Bash(sh scripts/check.sh*)"
    ],
    "ask": [
      "Bash(git push --force*)",
      "Bash(git push -f*)",
      "Bash(git push * --force*)",
      "Bash(git push --delete*)",
      "Bash(git push origin :*)",
      "Bash(git tag -d*)",
      "Bash(git tag -f*)",
      "Bash(git rebase*)",
      "Bash(git reset --hard*)",
      "Bash(git filter-branch*)",
      "Bash(git commit --amend*)"
    ]
  },
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          { "type": "command", "command": "git config core.hooksPath .githooks" },
          {
            "type": "command",
            "command": "python3 scripts/handoff.py --hook --entries 5",
            "timeout": 30
          }
        ]
      }
    ]
  }
}
```

## Evidence

**Nineteen cases**, `scripts/test_handoff.py`, two of them new: `--hook` names
the event it is answering, and carries the briefing under
`additionalContext`. The first is the one that matters — the field name is
the entire contract with the harness, and getting it wrong fails silently.

**Measured, because context is the cost:** 5,593 bytes at `--entries 5`,
against 10 KB at the default twelve. That is what a session pays to start
informed.

**Not verified: whether the permission rules will actually clear the
classifier.** The denial that blocked the `v0.1.0` tag push said a Bash
permission rule in settings would allow it in future, and that is the only
evidence there is. The rules are untested because applying them is the thing
I was refused. The next tag push either goes through or does not, and that is
the test.

## What this does not do

**It does not grant anything.** The entry describes a permission change that
is not applied; until somebody applies it, the authority is exactly what it
was and tag pushes still stop.

**The hook is not proven to fire.** `SessionStart` runs outside a turn, so
nothing here could trigger it. The flag's output is tested; the wiring is
not, and will not be until a session starts with the settings in place.

**5.6 KB on every session start is a real cost.** Trivial beside a
conversation and not beside nothing, and no measurement here says it pays for
itself — the argument is that an uninformed session asks worse questions,
which is a belief rather than a number.

**It briefs the model, not the person.** `additionalContext` reaches Claude;
a human starting a session sees nothing new and still runs
`python3 scripts/handoff.py` themselves.
