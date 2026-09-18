"""A decimal column, and the conditional update it exists to protect.

Both were built in the kernel and reachable only from Rust until the protocol
grew `Value.decimal_value` and `UpdateRequest.expected`. This is the Python
half.

# What a decimal is here, and what it is not

A count of the column's smallest unit. `Units(1250)` in a column declared
`scale=2` is 12.50, and the identical value in a `scale=0` column is 1250 --
nothing on the wire says which, because the protocol publishes no schema. So
the tests below are written to fail if a decimal ever arrives as an `i64`:
that is the confusion that loses two decimal places without an error anywhere.

A `decimal.Decimal` is refused rather than scaled. It carries a scale of its
own, there is nothing on the wire to reconcile it against, and a client that
guessed would be off by a factor of ten on a column it guessed wrong about.
"""

from __future__ import annotations

import decimal

import pytest

from slate import Client, Query, SlateError, Units, i64, u64
from slate.types import ValueType
from slate.values import ValueTypeError, from_value, to_value

from .conftest import as_str
from .fixture import PRICES

#: Ids of this module's own, so nothing here depends on another module's rows
#: or leaves any behind that another module would see. `prices` is not seeded
#: by the server, so every row in it is one of these.
FIRST = 400_000


@pytest.fixture
def priced(client: Client) -> Client:
    """A client, with three prices freshly seeded, since most of these move them.

    Not `autouse`: the encoding tests below need no server at all, and an
    autouse fixture that starts one turns every one of their assertions into a
    *setup error* under a mutation. That is not a kill by a named test, it is
    the suite falling over, and the difference showed up the first time these
    were mutated.
    """
    client.insert(
        PRICES,
        [
            (u64(FIRST + 1), "tea", Units(250)),
            (u64(FIRST + 2), "coffee", Units(1250)),
            (u64(FIRST + 3), "free", Units(0)),
        ],
        upsert=True,
    )
    return client


def _amount(client: Client, id: int) -> object:
    row = client.get(PRICES, (u64(id),))
    assert row is not None
    return row.get("amount")


# --- encoding, without a server -------------------------------------------


def test_units_encode_to_the_decimal_arm() -> None:
    assert to_value(Units(1250)).WhichOneof("kind") == "decimal_value"
    # And not to the integer arm beside it. The kernel orders values type
    # first, so a decimal sent as an `int64_value` would not compare equal to
    # the stored one and a predicate built from a row read back would select
    # nothing.
    assert to_value(1250, ValueType.I64).WhichOneof("kind") == "int64_value"


def test_a_bare_int_takes_the_declared_type_of_its_slot() -> None:
    assert to_value(1250, ValueType.DECIMAL).WhichOneof("kind") == "decimal_value"


def test_a_decimal_comes_back_as_units_and_not_as_an_int() -> None:
    back = from_value(to_value(Units(1250)))
    assert isinstance(back, Units)
    assert back == 1250


def test_a_python_decimal_is_refused_with_a_reason() -> None:
    with pytest.raises(ValueTypeError) as raised:
        # Deliberately not a `PyValue`: the point of the branch under test is
        # what happens when a caller reaches for the obvious Python type, and
        # the checker is right that they should not. Suppressed here and
        # nowhere else.
        to_value(decimal.Decimal("12.50"), ValueType.DECIMAL)  # ty: ignore[invalid-argument-type]
    # The message has to say what to do instead, because "cannot put Decimal on
    # the wire" leaves a caller guessing at exactly the point where guessing
    # costs two decimal places.
    assert "Units" in str(raised.value)
    assert "scale" in str(raised.value)


@pytest.mark.parametrize(
    ("units", "scale", "rendered"),
    [
        (1250, 2, "12.50"),
        (250, 2, "2.50"),
        (0, 2, "0.00"),
        (-75, 2, "-0.75"),
        (5, 3, "0.005"),
        (1250, 0, "1250"),
        # `i64::MIN`, whose magnitude does not fit in an `i64`: negating it
        # overflows back to itself. The Rust and Go renderers both go through a
        # wider type for this; Python's integers are unbounded, so this case
        # exists to keep the three agreeing rather than to catch a Python bug.
        (-9_223_372_036_854_775_808, 2, "-92233720368547758.08"),
    ],
)
def test_rendering_against_a_scale(units: int, scale: int, rendered: str) -> None:
    assert Units(units).to_string_with_scale(scale) == rendered


def test_the_declared_scale_is_readable_and_only_for_a_decimal() -> None:
    assert PRICES.scale_of("amount") == 2
    # `None` rather than 0 for a column that is not a decimal: a caller must
    # not be able to read a scale off a type that does not have one.
    assert PRICES.scale_of("label") is None
    assert PRICES.scale_of("nonexistent") is None


# --- and through a real server ---------------------------------------------


def test_a_decimal_round_trips_through_the_server(priced: Client) -> None:
    amount = _amount(priced, FIRST + 2)
    assert isinstance(amount, Units), f"a decimal came back as {type(amount).__name__}"
    assert amount == 1250
    assert Units(amount).to_string_with_scale(PRICES.scale_of("amount") or 0) == "12.50"


def test_a_negative_decimal_survives(priced: Client) -> None:
    # A refund is the value most likely to be mangled by an encoding that
    # assumed a price is positive.
    priced.insert(PRICES, [(u64(FIRST + 9), "refund", Units(-75))], upsert=True)
    assert _amount(priced, FIRST + 9) == Units(-75)


def test_a_decimal_filters_as_a_decimal(priced: Client) -> None:
    q = Query(PRICES)
    rows = list(priced.query(q.where(q.c.amount.ge(Units(250)) & q.c.id.ge(u64(FIRST)))))
    assert sorted(as_str(row.get("label")) for row in rows) == ["coffee", "tea"]


def test_an_integer_in_a_decimal_column_is_refused(priced: Client) -> None:
    with pytest.raises(SlateError):
        priced.insert(PRICES, [(u64(FIRST + 8), "wrong", i64(100))], upsert=True)


# --- the conditional update ------------------------------------------------


def test_an_update_naming_the_row_it_read_is_applied(priced: Client) -> None:
    result = priced.update(
        PRICES,
        [(u64(FIRST + 2), "coffee", Units(1400))],
        expected=[(u64(FIRST + 2), "coffee", Units(1250))],
    )
    assert result.affected == 1
    assert _amount(priced, FIRST + 2) == Units(1400)


def test_an_update_naming_a_row_that_moved_is_refused(priced: Client) -> None:
    # Somebody else's write lands between this caller's read and its write.
    priced.update(PRICES, [(u64(FIRST + 2), "coffee", Units(1300))])

    with pytest.raises(SlateError) as raised:
        priced.update(
            PRICES,
            [(u64(FIRST + 2), "coffee", Units(1400))],
            expected=[(u64(FIRST + 2), "coffee", Units(1250))],
        )
    assert "prices" in str(raised.value)
    # And the refusal is total: the write it guarded did not land.
    assert _amount(priced, FIRST + 2) == Units(1300)


def test_one_stale_row_refuses_the_whole_statement(priced: Client) -> None:
    priced.update(PRICES, [(u64(FIRST + 3), "free", Units(5))])

    with pytest.raises(SlateError):
        priced.update(
            PRICES,
            [
                (u64(FIRST + 1), "tea", Units(300)),
                (u64(FIRST + 3), "free", Units(99)),
            ],
            expected=[
                (u64(FIRST + 1), "tea", Units(250)),
                (u64(FIRST + 3), "free", Units(0)),
            ],
        )
    # The stale row is second, so the first row's write was applied and rolled
    # back with the statement. That is what makes this usable for a transfer,
    # where a half-applied pair is worse than a refused one.
    assert _amount(priced, FIRST + 1) == Units(250)


def test_a_stale_first_row_refuses_the_rest(priced: Client) -> None:
    """The other order, which is the one a loop that does not stop gets wrong.

    A server that reported the *last* row's outcome would answer this with a
    success while the first row was stale -- a lost update, reported as a
    successful conditional update, which is worse than not having the feature.
    """
    priced.update(PRICES, [(u64(FIRST + 1), "tea", Units(251))])

    with pytest.raises(SlateError):
        priced.update(
            PRICES,
            [
                (u64(FIRST + 1), "tea", Units(300)),
                (u64(FIRST + 2), "coffee", Units(1400)),
            ],
            expected=[
                (u64(FIRST + 1), "tea", Units(250)),
                (u64(FIRST + 2), "coffee", Units(1250)),
            ],
        )
    assert _amount(priced, FIRST + 2) == Units(1250)


def test_a_short_expected_is_refused_by_the_client(priced: Client) -> None:
    """Before the request leaves, and with both counts named.

    The server refuses this too, and its message is fine. The priced refuses it
    because sending it is never what a caller meant: a short `expected` leaves
    the rows past its end updated *unconditionally*, which is precisely the
    lost update the argument exists to catch.
    """
    with pytest.raises(ValueError) as raised:
        priced.update(
            PRICES,
            [
                (u64(FIRST + 1), "tea", Units(300)),
                (u64(FIRST + 2), "coffee", Units(1400)),
            ],
            expected=[(u64(FIRST + 1), "tea", Units(250))],
        )
    assert "2 row(s) and 1 expected" in str(raised.value)
    assert _amount(priced, FIRST + 2) == Units(1250)


def test_no_expected_is_an_ordinary_update(priced: Client) -> None:
    # `expected=None` and the default must both mean "no condition"; the field
    # is `repeated` on the wire, so there is no third state to confuse this
    # with.
    priced.update(PRICES, [(u64(FIRST + 1), "tea", Units(275))], expected=None)
    assert _amount(priced, FIRST + 1) == Units(275)


def test_a_conditional_update_inside_a_transaction(priced: Client) -> None:
    # The session path is different code from the autocommit one, and a feature
    # wired into one and not the other is this repository's recurring shape.
    with priced.transaction() as txn:
        txn.update(
            PRICES,
            [(u64(FIRST + 2), "coffee", Units(1500))],
            expected=[(u64(FIRST + 2), "coffee", Units(1250))],
        )
    assert _amount(priced, FIRST + 2) == Units(1500)


def test_a_refused_conditional_update_rolls_its_transaction_back(
    priced: Client,
) -> None:
    priced.update(PRICES, [(u64(FIRST + 2), "coffee", Units(1300))])
    with pytest.raises(SlateError), priced.transaction() as txn:
        txn.update(PRICES, [(u64(FIRST + 1), "tea", Units(999))])
        txn.update(
            PRICES,
            [(u64(FIRST + 2), "coffee", Units(1400))],
            expected=[(u64(FIRST + 2), "coffee", Units(1250))],
        )
    # The unconditional write that came first is gone with the transaction.
    assert _amount(priced, FIRST + 1) == Units(250)
