"""Does this package install as it says it does?

The TypeScript client shipped a `package.json` pointing at a path its build
never produced, so `npm install` yielded a package that threw on import — and
all 37 of its tests passed, because they import from `../src/` by relative path
and never go through the package entry point.

Python has the same shape of hole. `[tool.setuptools.packages.find]` decides
what gets installed, `package-data` decides what non-`.py` files come with it,
and the test suite imports from a source checkout where both are irrelevant. A
missing subpackage or a missing data glob is invisible until someone installs
the wheel.

So this builds the distribution and looks inside it, rather than trusting the
manifest to agree with the tree.
"""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys
import sysconfig
import tarfile
import tempfile
import zipfile

import pytest

HERE = pathlib.Path(__file__).resolve().parent
PACKAGE_ROOT = HERE.parent
SOURCE = PACKAGE_ROOT / "src" / "slate"


def _build() -> tuple[pathlib.Path, pathlib.Path]:
    """A wheel and an sdist, built from a clean copy of the tree.

    A *copy*, and this is the whole reason the test is trustworthy: a
    development checkout carries `src/slate_client.egg-info`, and setuptools
    reuses its recorded file list instead of re-reading `pyproject.toml`. Built
    in place, this test passed with `packages.find` narrowed to a single
    package and with `py.typed` removed from `package-data` — the stale
    metadata answered for both. A test of the packaging that reads cached
    packaging output is not a test of anything.
    """
    # Imported for its side effect of failing loudly. `build` is in this
    # package's `[dev]` extra, which is what `pytest` itself comes from, so a
    # checkout that can run this suite at all has it — and its absence is a
    # broken environment rather than a configuration somebody chose.
    #
    # This was a `pytest.skip`, and that is how these four tests came to skip
    # in CI from the day they were written: `build` was not declared, so the
    # skip fired on every run and read as a passing suite. The thing being
    # skipped is the point.
    import build  # noqa: F401

    clean = pathlib.Path(tempfile.mkdtemp()) / "package"
    shutil.copytree(
        PACKAGE_ROOT,
        clean,
        ignore=shutil.ignore_patterns(
            "*.egg-info", "__pycache__", "dist", "build", "testserver", ".pytest_cache"
        ),
    )

    out = pathlib.Path(tempfile.mkdtemp())
    result = subprocess.run(
        [sys.executable, "-m", "build", "--outdir", str(out), str(clean)],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        pytest.fail(f"building the distribution failed:\n{result.stdout}\n{result.stderr}")
    wheels = list(out.glob("*.whl"))
    sdists = list(out.glob("*.tar.gz"))
    assert wheels and sdists, f"build produced {sorted(p.name for p in out.iterdir())}"
    return wheels[0], sdists[0]


def test_the_wheel_carries_every_module_the_source_tree_has() -> None:
    """A subpackage `packages.find` misses is a subpackage that is not installed.

    `_proto` is the one that would hurt: it is three levels deep, it is
    generated rather than hand-written, and without it the client cannot import
    at all.
    """
    wheel, _ = _build()
    with zipfile.ZipFile(wheel) as archive:
        shipped = {
            name for name in archive.namelist() if name.startswith("slate/")
        }

    for path in sorted(SOURCE.rglob("*.py")):
        if "__pycache__" in path.parts:
            continue
        expected = "slate/" + path.relative_to(SOURCE).as_posix()
        assert expected in shipped, (
            f"{expected} is in the source tree and not in the wheel — "
            f"check `[tool.setuptools.packages.find]`"
        )


def test_the_wheel_carries_the_type_stubs() -> None:
    """`.pyi` files and `py.typed` are data, not modules.

    Without them the package installs and imports fine and every downstream
    type check silently loses the client's types — the kind of regression
    nobody notices for a release or two.

    A note on what this does and does not prove. Removing `py.typed` from
    `[tool.setuptools.package-data]` does *not* break it: modern setuptools
    ships files under the package directory anyway, so that entry is
    belt-and-braces and the mutation is equivalent. What does break it is a
    `MANIFEST.in` exclusion, and this test catches that. It guards the
    property, not one config line's spelling.
    """
    wheel, _ = _build()
    with zipfile.ZipFile(wheel) as archive:
        shipped = set(archive.namelist())

    stubs = [p for p in SOURCE.rglob("*.pyi") if "__pycache__" not in p.parts]
    assert stubs, "premise: the source tree has stubs to ship"
    for path in stubs:
        expected = "slate/" + path.relative_to(SOURCE).as_posix()
        assert expected in shipped, f"{expected} is missing from the wheel"

    assert "slate/py.typed" in shipped, (
        "without `py.typed`, a type checker ignores the stubs entirely"
    )


def test_the_installed_package_imports() -> None:
    """The check the TypeScript client did not have.

    A wheel whose files are all present and whose entry point raises on import
    is a wheel that passes every other test here. This installs it into a
    throwaway prefix and imports it in a subprocess, so nothing on this
    interpreter's path can answer for it.
    """
    wheel, _ = _build()
    target = pathlib.Path(tempfile.mkdtemp())
    install = subprocess.run(
        [sys.executable, "-m", "pip", "install", "--quiet", "--target", str(target), str(wheel)],
        capture_output=True,
        text=True,
    )
    if install.returncode != 0:
        pytest.skip(f"pip could not install into a target directory:\n{install.stderr}")

    check = subprocess.run(
        [sys.executable, "-c", "import slate; print(len(slate.__all__))"],
        capture_output=True,
        text=True,
        env={"PYTHONPATH": str(target), "PATH": sysconfig.get_path("scripts")},
    )
    assert check.returncode == 0, (
        f"the installed package does not import:\n{check.stdout}\n{check.stderr}"
    )
    assert int(check.stdout.strip()) > 0, "the package exports nothing"


def test_the_sdist_carries_what_a_maintainer_needs() -> None:
    """An sdist that cannot rebuild its own generated code is a dead end.

    It installs, it imports, and it is unmaintainable — which nothing else
    here notices. A consumer never regenerates, because the stubs are
    committed; a maintainer picking up the sdist alone does.

    This failed when first written: `scripts/` is not a package, so setuptools
    left it out and `MANIFEST.in` did not exist. The `.proto` is deliberately
    still absent — it lives in the server crate and this package holds no copy,
    because a copy could drift from the server's.
    """
    _, sdist = _build()
    with tarfile.open(sdist) as archive:
        names = {pathlib.Path(name).as_posix() for name in archive.getnames()}

    def present(suffix: str) -> bool:
        return any(name.endswith(suffix) for name in names)

    assert present("scripts/generate_proto.py"), "the generator is not in the sdist"
    assert present("src/slate/_proto/slate/v1/records_pb2.py"), (
        "the generated stubs are not in the sdist, so it cannot even be used as-is"
    )
