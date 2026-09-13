"""The contract's tagged value encoding, for the Python adapter.

Three adapters encode values identically or the conformance runner is comparing
formatting rather than answers. The rules, and why:

- **Tagged**, because an `i64` and a `u64` of the same magnitude are different
  values to this database, and a bare `1` would let the three adapters disagree
  about what they sent.
- **64-bit integers as strings**, because a JSON number above 2^53 does not
  survive `JSON.parse`, and a primary key is where that shows up latest.
- **Doubles as fixed-precision strings**, because Go, Python and JavaScript each
  have their own shortest-round-trip float formatter and they do not always
  agree on the last digit.

This client works in native Python values — `int`, `str`, `None`, plus `i64`
and `u64` subclasses that remember which of the wire's two integer types they
came from. That memory is what makes the tag possible on the way out.
"""

from __future__ import annotations

import math
from typing import Any

from slate import i64, u64


def format_float(value: float) -> str:
    """A double, rendered the way every adapter renders it."""
    if math.isnan(value) or math.isinf(value):
        return "null"
    return f"{value:.6f}"


def encode(value: Any) -> dict[str, Any]:
    """One decoded Python value in the contract's JSON form."""
    if value is None:
        return {"null": True}
    # Before `int`: `bool` is an `int` in Python and would tag as a number.
    if isinstance(value, bool):
        return {"bool": value}
    if isinstance(value, u64):
        return {"u64": str(int(value))}
    if isinstance(value, i64):
        return {"i64": str(int(value))}
    if isinstance(value, int):
        # A plain `int` reached this without a wire type to remember. Tagged as
        # `i64` because that is what `to_value` would have sent it as.
        return {"i64": str(value)}
    if isinstance(value, float):
        return {"f64": format_float(value)}
    if isinstance(value, str):
        return {"str": value}
    if isinstance(value, (bytes, bytearray)):
        return {"bytes": bytes(value).hex()}
    return {"unknown": type(value).__name__}


def encode_row(row: Any) -> list[dict[str, Any]] | None:
    """A row of decoded values, or `None` for an outer join's absent side."""
    if row is None:
        return None
    return [encode(v) for v in row]


def decode(tagged: dict[str, Any]) -> Any:
    """A tagged value from a request body, as this client wants it."""
    if not isinstance(tagged, dict):
        raise ValueError("a value must be a tagged object")
    if "null" in tagged:
        return None
    if "bool" in tagged:
        return bool(tagged["bool"])
    if "str" in tagged:
        return str(tagged["str"])
    if "i64" in tagged:
        return i64(int(tagged["i64"]))
    if "u64" in tagged:
        return u64(int(tagged["u64"]))
    if "f64" in tagged:
        return float(tagged["f64"])
    raise ValueError(f"a value carried no known kind: {sorted(tagged)}")
