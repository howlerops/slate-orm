# Handing this to another agent

Paste the block below as the first message to a fresh agent working in this
repository. Everything in it is either a standing rule or a pointer; the
*state* comes from `scripts/handoff.py`, which the prompt makes the agent run
first.

**That split is the whole design.** A handoff that states the state goes
stale the first time anybody commits —
`ledger/2026-10-03-a-briefing-that-reads-the-tree.md` has this repository's
own record of written summaries going wrong, three times over. So this file
holds only what does not move: how to work here, what is deliberately left,
and what only a person can do.

---

## The prompt

```text
You are picking up slate-orm, a Rust ORM and record layer over SlateDB.

FIRST, before anything else, run this and read the output:

    python3 scripts/handoff.py

It prints the live state: git position, the caveat frame, what is still
`narrowed` and what closes it, the newest ledger entries, and the caveats
those entries stopped at. Do not trust any state described below over what
that prints — this text is standing rules, the script is the truth.

THEN read CLAUDE.md in full. It is not boilerplate: it records specific,
expensive failures this repository has already paid for (CI's clippy and
ruff are newer than the container's; `cargo test --workspace` does not fit
on this disk; a skip reads as green). Working against it wastes a session.

HOW WORK IS JUDGED HERE, in short — CLAUDE.md has the long version:

- Every change outside `ledger/` needs a ledger entry in the SAME commit.
  A pre-commit hook refuses otherwise. `cp ledger/TEMPLATE.md
  ledger/$(date +%F)-a-short-slug.md`. The section that matters most is
  "Alternatives rejected" — what else would have worked and what it cost.
- Mutation-test what you write, with `scripts/mutate.py` (never a hand-rolled
  sed). Break it, confirm a NAMED test fails, restore, re-verify. First check
  the mutation was actually a change: an equivalent mutation survives
  everything and looks exactly like a discovery.
- Measure, do not assert. Report spread. Never report a number you did not
  observe. If a result contradicts something you already wrote, withdraw it
  and say you did.
- Stale documentation is worse than none. If your change makes a doc, a
  comment or a README bullet wrong, fixing it is part of the change.
- Report honestly: what you did not test, what you guessed, dead ends.

BEFORE EVERY COMMIT:

    python3 scripts/reclaim.py     # disk is tight; ENOSPC masquerades as
                                   # linker and LLVM errors
    sh scripts/check.sh            # every static check CI runs

A green check.sh is necessary and NOT sufficient — CI's clippy, rustfmt and
ruff are all newer than this container's and have turned jobs red on code
that was clean locally. After pushing, read the run's conclusion rather than
assuming; `scripts/handoff.py` prints the `gh api` line for it.

BRANCH: develop on `claude/rust-orm-record-layer-gswxlu`, merge to `main`
when green. Do not open a pull request unless asked.

WHAT IS LEFT, scoped and with the design already argued:

T4 — column-level grants.
    The real multi-tenant blocker, named in the views row: a view here
    cannot be a privilege boundary, because a caller needs the grant on
    every base table and having it lets them read the columns the view
    omits. "Give the analysts a narrowed view" does not work.
    Built in the kernel and the daemon: `docs/column-grants.md`, whose
    "Build order" section says which steps landed. What is left is its
    step 3's guard — a check that every row-returning path conceals,
    in the style of `scripts/check_handlers.py` — and step 5, the same
    restricted identity through the three clients' conformance runner.
    The sentinel oracle is `crates/slate-kernel/tests/column_grants.rs`;
    extend it rather than adding a test per path.

DO NOT DO THESE — they are decided, not forgotten. Each has a `deliberate`
verdict with an argument in `docs/caveat-status.json`:

- Cross-compiling was just done; the image build is 4m31s, not 77 minutes.
- Do not add `linux/arm64` to anything beyond CI's image matrix.
- Do not publish to npm or PyPI. Both are off behind repository variables
  on purpose, the names are unclaimed, and a published version number can
  never be reused.
- Do not tag a release. `0.1.0` is out; the next tag is a human's decision.

ONLY THE OWNER CAN DO THESE — if your work needs one, stop and say so:

- Apply the permission block in
  `ledger/2026-10-03-the-briefing-as-a-session-start-hook.md` to
  `.claude/settings.json`. An agent writing its own permissions is refused
  as self-modification, correctly.
- Claim `@slate-orm` on npm and `slate-client` on PyPI, add NPM_TOKEN, add
  PyPI's Trusted Publisher, set PUBLISH_NPM and PUBLISH_PYPI.
  `docs/releasing.md` has the exact fields.
- Push a `v*` tag.

UNVERIFIED, so do not state otherwise: whether a Bash permission allowlist
actually lets an agent push a tag here. The only evidence is the wording of
the denial that blocked the v0.1.0 push. It has not been tested.
```

---

## Keeping this true

The prompt names two tasks and four refusals. Both lists are the kind that
rot. When T3 or T4 lands, or a refusal is reconsidered, this file is part of
that change — the same rule as any other doc here.

What does *not* rot is the first instruction. An agent that runs
`scripts/handoff.py` gets the live frame even if everything below it has gone
stale, which is why that line is first and says so.
