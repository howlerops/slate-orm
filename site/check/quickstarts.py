#!/usr/bin/env python3
"""Run the quickstarts on the landing page, exactly as they are printed.

The site's README used to say the snippets "were extracted from the page and
executed" and then, honestly, that nothing re-ran them. That is the shape of
staleness that matters most: a landing page's code sample is the first thing a
reader tries and the last thing anybody edits when an API changes.

So this extracts the four `<pre><code>` panels out of `site/index.html`, starts
a head node from the TOML panel, and runs the three snippets against it.

What "exactly as they are printed" costs, and where it is spent:

* The TOML panel names an S3 bucket, which cannot be started here. It is
  therefore checked twice — `slate-serverd --check` validates the page's text
  verbatim, and a copy with only the `[storage]` stanza swapped for `memory` is
  what actually runs. The swap is asserted to touch nothing else, so the rest
  of the page's config is the config under test.
* The snippets name `127.0.0.1:7421`. Each run picks a free port instead, and
  the substitution is asserted to fire exactly once per snippet, so a snippet
  that stops mentioning the address fails rather than silently connecting
  somewhere else.
* Go's snippet is a fragment: no package clause, no imports, no `ctx`. It is
  spliced verbatim into a wrapper this file owns. The wrapper reads `err` and
  prints `rows` — Go requires that, and it also means the assertion below is
  about the snippet's own result and not about anything the wrapper computed.

Usage: python3 site/check/quickstarts.py [--keep]
"""

from __future__ import annotations

import argparse
import html
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
INDEX = ROOT / "site" / "index.html"

# The row every snippet inserts and reads back. Asserting on the title rather
# than on an exit status is the difference between "the program ran" and "the
# program talked to the database": a snippet whose query silently returned
# nothing would still exit 0.
EXPECTED = "A Wizard of Earthsea"

PANEL = re.compile(
    r'<div data-panel="(?P<name>\w+)"[^>]*>\s*<pre><code>(?P<code>.*?)</code></pre>',
    re.DOTALL,
)


def panels() -> dict[str, str]:
    found = {
        match["name"]: html.unescape(match["code"])
        for match in PANEL.finditer(INDEX.read_text())
    }
    missing = {"py", "go", "ts", "toml"} - found.keys()
    if missing:
        raise SystemExit(f"index.html has no panel for {', '.join(sorted(missing))}")
    return found


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def substitute(code: str, port: int, what: str) -> str:
    """Point a snippet at this run's port, and refuse to do it silently."""
    replaced, count = re.subn(r"127\.0\.0\.1:7421", f"127.0.0.1:{port}", code)
    if count != 1:
        raise SystemExit(
            f"the {what} snippet mentions 127.0.0.1:7421 {count} times, expected once — "
            "either it no longer connects to the head node, or it connects twice and "
            "this check would rewrite both"
        )
    return replaced


def runnable_toml(page: str, port: int) -> str:
    """The page's TOML with `[storage]` swapped for one that can start here.

    Done by splitting rather than by substituting, so that everything outside
    the storage stanza is unchanged *by construction*. A regex rewrite of a
    config file, checked by eye, is how a check ends up validating a config the
    page does not contain.
    """
    stanza = re.search(
        r"^\[storage\]\n.*?(?=^\[(?!storage\.))", page, re.DOTALL | re.MULTILINE
    )
    if stanza is None:
        raise SystemExit("the TOML panel has no `[storage]` stanza")
    if 's3' not in stanza.group():
        raise SystemExit(
            "the TOML panel's `[storage]` stanza no longer names s3; this check swaps it "
            "for an in-memory one precisely because the page's cannot be started here, "
            "so if the page now shows something runnable, run it instead of swapping"
        )

    before, after = page[: stanza.start()], page[stanza.end() :]
    swapped = f'{before}[storage]\nbackend = "memory"\n\n{after}'
    return substitute(swapped, port, "TOML")


def serverd() -> Path:
    """The daemon to run: `SLATE_SERVERD`, or the one this tree has built.

    The same variable the three client suites and the demo honour, so CI builds
    the binary once. Set and missing is a hard error, as it is there.
    """
    named = os.environ.get("SLATE_SERVERD")
    if named:
        path = Path(named)
        if not path.exists():
            raise SystemExit(f"SLATE_SERVERD={named} does not exist")
        return path
    built = ROOT / "target" / "debug" / "slate-serverd"
    if not built.exists():
        raise SystemExit(f"{built} is not built; cargo build -p slate-serverd")
    return built


class Node:
    """A head node started from the page's own configuration."""

    def __init__(self, config: Path, log: Path, port: int) -> None:
        self.port = port
        self.log = log
        binary = serverd()
        self.handle = subprocess.Popen(
            [str(binary), "--config", str(config)],
            stdout=log.open("w"),
            stderr=subprocess.STDOUT,
        )

    def wait(self, seconds: int = 30) -> None:
        # `LISTENING` is printed once the listener is bound, which closes the
        # window a port poll leaves open between bind and accept.
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if "LISTENING" in self.log.read_text(errors="replace"):
                return
            if self.handle.poll() is not None:
                raise SystemExit(
                    f"the head node exited {self.handle.returncode}:\n{self.log.read_text()}"
                )
            time.sleep(0.2)
        raise SystemExit(f"the head node never listened:\n{self.log.read_text()}")

    def stop(self) -> None:
        self.handle.terminate()
        try:
            self.handle.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.handle.kill()


def run(command: list[str], cwd: Path, env: dict[str, str] | None = None) -> subprocess.CompletedProcess:
    merged = dict(os.environ)
    merged.update(env or {})
    return subprocess.run(
        command, cwd=cwd, env=merged, capture_output=True, text=True, timeout=900
    )


# --- one function per snippet ----------------------------------------------
#
# Each returns the process that ran it. The wrappers differ because the three
# ecosystems differ, and pretending otherwise would mean a lowest-common
# harness that runs none of them the way a reader would.


def check_python(code: str, work: Path) -> subprocess.CompletedProcess:
    script = work / "quickstart.py"
    script.write_text(code)
    return run(
        [sys.executable, str(script)],
        cwd=work,
        env={"PYTHONPATH": str(ROOT / "clients" / "python" / "src")},
    )


GO_WRAPPER = """package main

// Everything outside the SNIPPET markers belongs to site/check/quickstarts.py,
// not to the page. The page's Go panel is a fragment on purpose — a reader
// pastes it into a function they already have — so the wrapper supplies the
// package clause, the imports and the context, and then reads `err` and prints
// `rows` because Go insists and because the assertion should be about the
// snippet's own result.

import (
\t"context"
\t"fmt"
\t"log"

\t"github.com/howlerops/slate-orm/clients/go/slate"
)

func main() {
\tctx := context.Background()
\t_ = ctx

\t// --- SNIPPET ---
%s
\t// --- END SNIPPET ---

\tif err != nil {
\t\tlog.Fatal(err)
\t}
\tdefer rows.Close()
\tfor rows.Next() {
\t\tfmt.Println(rows.Row())
\t}
\tif err := rows.Err(); err != nil {
\t\tlog.Fatal(err)
\t}
}
"""


def check_go(code: str, work: Path) -> subprocess.CompletedProcess:
    module = work / "go"
    module.mkdir()
    indented = "\n".join(("\t" + line if line.strip() else "") for line in code.splitlines())
    (module / "main.go").write_text(GO_WRAPPER % indented)
    (module / "go.mod").write_text(
        "module quickstart\n\n"
        "go 1.24\n\n"
        "require github.com/howlerops/slate-orm/clients/go v0.0.0\n\n"
        f"replace github.com/howlerops/slate-orm/clients/go => {ROOT / 'clients' / 'go'}\n"
    )
    tidy = run(["go", "mod", "tidy"], cwd=module)
    if tidy.returncode != 0:
        return tidy
    return run(["go", "run", "."], cwd=module)


def check_typescript(code: str, work: Path) -> subprocess.CompletedProcess:
    package = work / "ts"
    package.mkdir()
    (package / "quickstart.ts").write_text(code)
    (package / "package.json").write_text(
        json.dumps(
            {
                "name": "quickstart",
                "private": True,
                "type": "module",
                "dependencies": {
                    "@slate-orm/client": f"file:{ROOT / 'clients' / 'typescript'}"
                },
                "devDependencies": {"@types/node": "^22.10.0", "typescript": "^5.7.2"},
            },
            indent=2,
        )
    )
    (package / "tsconfig.json").write_text(
        json.dumps(
            {
                "compilerOptions": {
                    "target": "ES2022",
                    "module": "NodeNext",
                    "moduleResolution": "NodeNext",
                    "strict": True,
                    "outDir": "dist",
                    "skipLibCheck": True,
                },
                "include": ["quickstart.ts"],
            },
            indent=2,
        )
    )
    install = run(["npm", "install", "--silent", "--no-audit", "--no-fund"], cwd=package)
    if install.returncode != 0:
        return install
    compiled = run(["npx", "tsc", "-p", "tsconfig.json"], cwd=package)
    if compiled.returncode != 0:
        return compiled
    return run(["node", "dist/quickstart.js"], cwd=package)


CHECKS = {
    "python": check_python,
    "go": check_go,
    "typescript": check_typescript,
}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--keep", action="store_true", help="leave the scratch tree in place")
    parser.add_argument(
        "--only",
        choices=sorted(CHECKS),
        action="append",
        help="run one snippet (repeatable); the TOML is always checked",
    )
    arguments = parser.parse_args()

    page = panels()
    port = free_port()
    work = Path(tempfile.mkdtemp(prefix="slate-quickstart-"))
    failures: list[str] = []

    try:
        # 1. The page's TOML, verbatim, through the validator.
        verbatim = work / "page.toml"
        verbatim.write_text(page["toml"])
        checked = run([str(serverd()), "--config", str(verbatim), "--check"], cwd=work)
        if checked.returncode != 0:
            failures.append("toml")
            print("FAIL  the TOML on the page does not validate")
            print((checked.stdout + checked.stderr).rstrip())
        else:
            print(f"ok    the TOML on the page validates — {checked.stdout.strip()}")

        # 2. The same TOML, storage swapped, actually started.
        config = work / "run.toml"
        config.write_text(runnable_toml(page["toml"], port))
        node = Node(config, work / "head.log", port)
        node.wait()
        print(f"ok    a head node started from it on 127.0.0.1:{port}")

        try:
            wanted = arguments.only or sorted(CHECKS)
            for name in wanted:
                key = {"python": "py", "go": "go", "typescript": "ts"}[name]
                snippet = substitute(page[key], port, name)
                result = CHECKS[name](snippet, work)
                output = result.stdout + result.stderr
                if result.returncode != 0:
                    failures.append(name)
                    print(f"FAIL  the {name} snippet exited {result.returncode}")
                    print("\n".join("        " + line for line in output.rstrip().splitlines()))
                elif EXPECTED not in output:
                    failures.append(name)
                    print(f"FAIL  the {name} snippet ran but never printed {EXPECTED!r}")
                    print("\n".join("        " + line for line in output.rstrip().splitlines()))
                else:
                    print(f"ok    the {name} snippet inserts a row and reads it back")
        finally:
            node.stop()
    finally:
        if arguments.keep:
            print(f"\nscratch tree kept at {work}")
        else:
            shutil.rmtree(work, ignore_errors=True)

    print()
    if failures:
        print(f"{len(failures)} failed: {', '.join(failures)}")
        return 1
    print("the quickstarts on the landing page run")
    return 0


if __name__ == "__main__":
    sys.exit(main())
