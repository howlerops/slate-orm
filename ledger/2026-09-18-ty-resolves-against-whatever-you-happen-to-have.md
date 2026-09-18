# The new Python checker went red in CI on a file nobody had touched

- **Date:** 2026-09-18
- **Author:** Claude (Opus 5)
- **Touches:** `pyproject.toml`, `site/data/make-trips.py`, `CLAUDE.md`
- **Kind:** fix

## What changed

`site/data/make-trips.py` is excluded from `ty` and still linted by `ruff`, and
`CLAUDE.md` gains the recipe for running `ty` the way CI runs it.

## Why

The `scripts` job added an hour ago failed on its first real run, on a file the
commit did not touch:

```
error[unresolved-import]: Cannot resolve imported module `pyarrow.parquet`
  --> site/data/make-trips.py:60:12
```

`make-trips.py` regenerates `site/data/trips.bin.gz`, which is committed, and
its own docstring records why: rebuilding it in CI "would mean a 50 MB
download on every run, a pyarrow dependency, and a sampling step that has to be
bit-for-bit deterministic". So `pyarrow` is deliberately absent from every
environment CI builds — and it is present in this container, where `ty check`
was green.

That is the repository's own recurring failure with a new face. "Passed locally
for the worst possible reason" has been written up here twice already; this is
the third, and the mechanism is worth naming rather than just fixing: **a type
checker's answer depends on the `site-packages` it can see, so a local run and
a CI run are different checks.** The same is true of clippy's version, which
`CLAUDE.md` already warns about, and the note now sits beside it.

## Alternatives rejected

**Installing `pyarrow` in the `scripts` job.** The straightforward fix: the two
environments agree and the file stays fully checked. It contradicts a decision
this repository has already made and written down — the script exists in the
shape it does *because* pyarrow was kept out of CI — and it puts a 40 MB wheel
into a job whose whole point is being cheap. Rejected on the grounds that the
docstring's argument is still good.

**A per-line `# ty: ignore[unresolved-import]`.** The obvious narrow fix, and it
does not work here, which is the interesting part. The directive is *used* in
CI and *unused* locally, and `ty` reports an unused directive as a diagnostic —
so the suppression is correct in one environment and an error in the other. A
suppression that cannot be right in both is worse than none. Tried, measured,
discarded.

**Dropping `site/data` from the `include` list.** It works and it lies: the
mechanism would be an omission twenty lines from the explanation, and a second
file added to that directory would be silently unchecked. `include` keeps the
directory in scope and `exclude` names the one file, so the comment sits on the
line that does the work. (The first attempt did both, which made the `exclude`
decorative — caught by the reproduction below, which passed in *both* arms.)

**Excluding it from `ruff` too.** No reason to. Ruff resolves no imports, so
every name error, unused import and style rule still applies — which is most of
what this script would get from either tool. The loss is confined to
type-checking the one function that reads the parquet.

## Evidence

A venv holding exactly what the CI job installs, and nothing else:

```sh
python3 -m venv /tmp/ci-env && /tmp/ci-env/bin/pip install -e './clients/python[dev]'
/tmp/ci-env/bin/ty check --python /tmp/ci-env
```

| `pyproject.toml` | result |
| --- | --- |
| as committed (`exclude` set) | exit 0 |
| `exclude = []` — the mutation | exit 1, `Cannot resolve imported module 'pyarrow.parquet'` |

That is CI's failure, reproduced locally and character for character, and it
confirms the `exclude` is what makes the job pass rather than something else in
the environment. The same two runs against this container's interpreter both
pass, which is the whole point.

`ruff check .`, `ty check` and the docs check green afterwards.

## What this does not do

- **`make-trips.py` is not type-checked.** Stated in the config, in the script
  and here. It is eighty lines that run by hand, perhaps once a year.
- **It does not make the two environments agree.** They still differ by
  everything this container happens to have installed, and the only guard is
  the recipe in `CLAUDE.md` — which is a thing somebody has to remember, not a
  check. A `scripts` job that built its own venv and compared would close it;
  that is a bigger change than this and would want its own reasoning.
- **Nothing prevents the next one.** Any script here that imports a module
  present locally and absent in CI will fail the same way. What is different
  now is that the failure is a one-line message in a fast job rather than a
  traceback in a slow one.
