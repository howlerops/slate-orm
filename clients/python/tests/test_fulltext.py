"""`contains`, through a real server and onto a real inverted index.

The kernel's own suite proves that a text index holds one entry per term and
that a search over it agrees with a scan. This file proves the other half: that
a Python caller can express the search at all, that the *server* does the
splitting, and that the index is reachable from here rather than merely
present.

The last of those is the one worth naming. A text index is not chosen by cost
at this scale -- `docs/full-text.md` measures the crossover at roughly one row
in 24,000 -- so a test that simply searched and got the right rows would be
testing a table scan and would pass with the index deleted. Every search below
that claims the index says so with a hint, and
`test_the_index_and_a_scan_find_the_same_rows` checks the two plans differ
before it compares their rows.
"""

from __future__ import annotations

import pytest

from slate import Array, Client, Query, asc, u64

from .conftest import as_int
from .fixture import POSTS

#: Ids of this module's own. `posts` is not seeded, and `test_array` writes at
#: 500_000, so nothing here can see a row it did not write.
FIRST = 1_200_000

#: Prose chosen so that the terms overlap in three different ways: "rust" and
#: "the" appear in several, "storage" in two, "engine" in one. A corpus where
#: each term picked out exactly one row would make a conjunction and a
#: disjunction indistinguishable.
POSTED: list[tuple[int, str]] = [
    (1, "Rust and the storage engine"),
    (2, "The storage layer, revisited"),
    (3, "rust: a retrospective"),
    (4, "Nothing to do with either"),
]


@pytest.fixture
def posted(client: Client) -> Client:
    client.insert(
        POSTS,
        [
            (u64(FIRST + i), title, Array([]), None)
            for i, title in POSTED
        ],
        upsert=True,
    )
    return client


def search(text: str, *, hinted: bool = True) -> Query:
    """A `contains` over this module's rows, on the text index by default."""
    q = Query(POSTS)
    q = q.where(q.c.id.ge(u64(FIRST)) & q.c.title.contains(text)).sort(asc(q.c.id))
    return q.using_index("by_title_text") if hinted else q.using_table_scan()


def ids(client: Client, query: Query) -> list[int]:
    return [as_int(row.values[0]) for row in client.query(query)]


def test_one_term_finds_every_row_holding_it(posted: Client) -> None:
    assert ids(posted, search("rust")) == [FIRST + 1, FIRST + 3]


def test_the_server_lowercases_the_search(posted: Client) -> None:
    """`RUST` finds the row written as `rust`, and the client sent `RUST`.

    The tokenizer folds case on both sides. This client does no splitting and
    no folding of its own -- see `ColumnRef.contains` -- so the fold observed
    here happened on the server.
    """
    assert ids(posted, search("RUST")) == [FIRST + 1, FIRST + 3]


def test_two_terms_are_a_conjunction_not_a_disjunction(posted: Client) -> None:
    """Both rows hold "rust"; both hold "the"; only one holds both plus
    "storage"."""
    assert ids(posted, search("rust storage")) == [FIRST + 1]


def test_punctuation_is_a_separator_and_not_part_of_a_term(posted: Client) -> None:
    """Row 3 is written `rust: a retrospective`, so its first term is `rust`.

    A search for `rust:` must find it: the colon is a separator on both sides.
    If the client sent terms rather than text, this is the case where its
    splitting and the server's would have to agree by luck.
    """
    assert ids(posted, search("rust:")) == [FIRST + 1, FIRST + 3]


def test_a_substring_of_a_term_is_not_a_term(posted: Client) -> None:
    """`stor` is a prefix of `storage` and matches nothing.

    This is the line between `contains` and `like('%stor%')`, and it is the
    reason both exist.
    """
    assert ids(posted, search("stor")) == []


def test_a_term_in_no_row_finds_nothing(posted: Client) -> None:
    assert ids(posted, search("cephalopod")) == []


def test_the_index_and_a_scan_find_the_same_rows(posted: Client) -> None:
    """The oracle: two access paths, one answer.

    The plans are compared *first*. Without that this test would pass with the
    hint ignored and both sides scanning, which is the failure mode that made
    the kernel's version of this test vacuous when it was first written.
    """
    for text in ("rust", "the storage", "rust storage engine", "nothing"):
        indexed = search(text)
        scanned = search(text, hinted=False)

        by_index = posted.explain(indexed).access
        by_scan = posted.explain(scanned).access
        assert by_index != by_scan, (
            f"both paths planned the same way for {text!r} ({by_index}), so this "
            "compares a scan against a scan"
        )
        assert "by_title_text" in by_index, by_index

        assert ids(posted, indexed) == ids(posted, scanned), text
