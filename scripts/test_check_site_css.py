#!/usr/bin/env python3
"""`check_site_css.py`, over trees this file writes.

Against a written tree rather than against `slate-orm`, for the reason the
other guards here give: a check tested only by running it over this repository
asserts that today's tree is clean, which is also what a check that does
nothing asserts. Every case below makes one thing wrong and expects the guard
to say which.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_site_css as guard

CSS = """\
:root { --line: #ddd; }
.card { border: 1px solid var(--line); }
.badge[data-tone="warn"] { color: gold; }
#editor { font-family: monospace; }
/* .retired was dropped when the panel became the workbench. */
"""

PAGE = """<html><body>
<div class="card">a card</div>
<textarea id="editor"></textarea>
</body></html>
"""

#: The dynamic case the docstring promises to accept: the class is built by
#: concatenation and never appears as a whole attribute value.
SCRIPT = 'const badge = (tone) => `<span class="badge" data-tone="${tone}"></span>`;\n'

#: The demo's stylesheet and its one source, so the second sheet in `SHEETS`
#: is present and clean in every case. Without it every case would carry a
#: "does not exist" failure and `failing` would have to name it each time.
DEMO_CSS = ".panel { padding: 8px; }\n"
DEMO_SOURCE = 'export const Panel = () => <div class="panel" />;\n'

CLEAN: dict[str, str] = {
    "site/style.css": CSS,
    "site/index.html": PAGE,
    "site/workbench.js": SCRIPT,
    "site/docs/features.html": "<html><body><p>nothing styled here</p></body></html>\n",
    "examples/explorer/web/src/styles.css": DEMO_CSS,
    "examples/explorer/web/src/panels.tsx": DEMO_SOURCE,
}


def write(root: Path, files: dict[str, str]) -> None:
    for name, body in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")


def case(name: str, changes: dict[str, str | None], failing: set[str]) -> bool:
    files = dict(CLEAN)
    for path, body in changes.items():
        if body is None:
            files.pop(path, None)
        else:
            files[path] = body
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        write(root, files)
        report = guard.check(root)
        failed = [what for what, ok, _ in report if not ok]
        unexpected = [f for f in failed if not any(frag in f for frag in failing)]
        unseen = [frag for frag in failing if not any(frag in f for f in failed)]
        if unexpected or unseen:
            print(f"FAIL  {name}")
            for f in unexpected:
                print(f"        unexpected failure: {f}")
            for frag in unseen:
                print(f"        expected a failure mentioning {frag!r}, got {failed}")
            return False
    print(f"ok    {name}")
    return True


def main() -> int:
    passed = [
        case("a stylesheet whose every selector is used passes", {}, set()),
        case(
            "a class no page or script names is reported",
            {"site/style.css": CSS + ".ghost { display: none; }\n"},
            {"every class site/style.css defines"},
        ),
        case(
            "an id no page or script names is reported",
            {"site/style.css": CSS + "#gone { color: red; }\n"},
            {"every id site/style.css defines"},
        ),
        case(
            # The whole reason `selectors()` strips comments first. Without it
            # the note about `.retired` in CSS reads as a definition, and the
            # guard fails on a rule nobody wrote — reporting on its own input.
            "a class named only inside a CSS comment is not treated as defined",
            {"site/style.css": CSS + "/* .also-retired, and .this-one-too */\n"},
            set(),
        ),
        case(
            # A class attached by string concatenation. Accepted deliberately;
            # the docstring says why, and this case is what stops someone
            # "tightening" the match to whole attribute values.
            "a class only ever built dynamically still counts as used",
            {"site/index.html": PAGE.replace('<div class="card">a card</div>', "")},
            {"every class site/style.css defines"},
        ),
        case(
            "the docs pages count as sources",
            {
                "site/style.css": CSS + ".only-in-docs { margin: 0; }\n",
                "site/docs/features.html": '<html><body><p class="only-in-docs">x</p></body></html>\n',
            },
            set(),
        ),
        case(
            # `slate_wasm.js` is a megabyte of generated glue; matching against
            # it would make almost any short name look used, which is a guard
            # that passes on anything.
            "the generated wasm glue is not a source",
            {
                "site/style.css": CSS + ".ghost { display: none; }\n",
                "site/slate_wasm.js": "// ghost\n",
            },
            {"every class site/style.css defines"},
        ),
        case(
            # The caveat the widening closed: the demo has a stylesheet of its
            # own and the first version of this guard read only `site/`.
            "a dead class in the demo's own stylesheet is reported",
            {"examples/explorer/web/src/styles.css": DEMO_CSS + ".ghost { display: none; }\n"},
            {"every class examples/explorer/web/src/styles.css defines"},
        ),
        case(
            # Each sheet is checked against its *own* sources. A class defined
            # in the demo's sheet and mentioned only in `site/` is dead in the
            # demo, and reading one pooled corpus would have missed it.
            "a demo class named only by a site page is still dead",
            {
                "examples/explorer/web/src/styles.css": DEMO_CSS + ".card { border: 0; }\n",
            },
            {"every class examples/explorer/web/src/styles.css defines"},
        ),
        case(
            "a stylesheet with no sources at all is reported, not passed",
            {"examples/explorer/web/src/panels.tsx": None},
            # Both: with no sources the class check also fails, which is the
            # right answer — every class is dead when nothing can name one —
            # and the sources check is what says *why*.
            {
                "examples/explorer/web/src/styles.css has sources",
                "every class examples/explorer/web/src/styles.css defines",
            },
        ),
        case(
            "a missing stylesheet is reported rather than passing empty",
            {"site/style.css": None},
            {"site/style.css exists"},
        ),
        case(
            "a stylesheet with no classes at all is reported",
            {"site/style.css": ":root { --line: #ddd; }\n"},
            {"site/style.css defines some classes"},
        ),
    ]
    print(f"\n{sum(passed)} passed, {len(passed) - sum(passed)} failed")
    return 0 if all(passed) else 1


if __name__ == "__main__":
    sys.exit(main())
