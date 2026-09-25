#!/usr/bin/env python3
"""Generate a client's schema declaration from the server's own catalog.

# Why this exists

A slate-orm client in another language holds its own copy of the schema. The
protocol publishes none — "the catalog is the server's, and a request names a
table and its columns by the names and ordinals that catalog already has" — so
`Table`/`TableDef` is the client restating it, and the server checks that
restatement with a fingerprint on every request.

That restatement was typed by hand in every client, which is the gap the
comparison against Drizzle and Prisma records: they generate it. Hand-typing it
is not merely tedious, it is the *one* piece of client code where being wrong
is quiet. A wrong ordinal points every reference past it at a different column
and the server cannot tell — the reference is legitimate. The fingerprint
catches it on the first request, which is a good backstop and still a backstop:
the failure has already been written, committed and shipped.

# What it reads

`slate-serverd --config <file> --print-schema`, which resolves the same TOML
the node serves and prints the catalog as JSON. Reading the *resolved* catalog
rather than the TOML is the point: the TOML is a source that the schema layer
interprets — ordinals come from declaration order, a primary key is named and
resolved to ordinals, a decimal's scale is validated — and a generator that
re-interpreted it would be a second implementation of that resolution, free to
disagree with the first. Exactly the disagreement this is meant to end.

# What it writes

One module per target language, each a complete replacement for the file it
overwrites. Generated files carry a header saying so and are checked in,
because a check-in plus `--check` is what makes drift a *CI failure* rather
than something a developer discovers when a request is refused. The
alternative — generating at build time — was rejected for every client here:
Go and TypeScript would need the Rust toolchain present at build time, and the
Python package would need it at install time.

# What it deliberately does not generate

Only the columns, their types and scales, and the primary key — the fields the
fingerprint hashes, which is exactly the set where being wrong is silent. Not
indexes: a client never names one on the wire (`Hint` takes a name, and a wrong
name is an error the server reports). Not `added_in`/`dropped_in`: a client
that pins a schema version is a client that stops working on the next
migration. Generating the fingerprint itself was considered and rejected — the
whole value of the check is that the client computes it from the declaration it
actually holds, so a generated constant would make the check tautological.

Usage:

    python3 scripts/codegen.py --config examples/explorer/head.toml \\
        --python examples/explorer/backends/python/adapter/schema.py
    python3 scripts/codegen.py --config examples/explorer/head.toml \\
        --python <path> --check
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import typing
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: A catalog type name, as `--print-schema` spells it, mapped onto what each
#: client calls it.
#:
#: Three separate tables rather than one with three columns, because the sets
#: are not guaranteed to stay in step: a type this generator does not know
#: about is a hard error below, and it should be possible to add one to a
#: single client without inventing spellings for the other two.
PYTHON_TYPES = {
    "bool": "ValueType.BOOL",
    "bytes": "ValueType.BYTES",
    "string": "ValueType.STR",
    "i64": "ValueType.I64",
    "u64": "ValueType.U64",
    "f64": "ValueType.F64",
    "uuid": "ValueType.UUID",
    "vector": "ValueType.VECTOR",
    "decimal": "ValueType.DECIMAL",
    "array": "ValueType.ARRAY",
}

GO_TYPES = {
    "bool": "slate.TypeBool",
    "bytes": "slate.TypeBytes",
    "string": "slate.TypeString",
    "i64": "slate.TypeInt",
    "u64": "slate.TypeUint",
    "f64": "slate.TypeFloat",
    "uuid": "slate.TypeUUID",
    "vector": "slate.TypeVector",
    "decimal": "slate.TypeDecimal",
    "array": "slate.TypeArray",
}

TYPESCRIPT_TYPES = {
    "bool": '"bool"',
    "bytes": '"bytes"',
    "string": '"string"',
    "i64": '"i64"',
    "u64": '"u64"',
    "f64": '"f64"',
    "uuid": '"uuid"',
    "vector": '"vector"',
    "decimal": '"decimal"',
    "array": '"array"',
}

#: An array column's element type is not in `type` — it is a second field on
#: the column, the way a decimal's scale is, for the reason `docs/arrays.md`
#: gives: `ValueType` stays fieldless so that it stays `Copy`, `const`, and
#: finitely enumerable. So every table above is keyed by a *scalar* type name
#: and an array is resolved through here instead.
#:
#: This is what the `UNSUPPORTED` entry used to say could not be expressed, and
#: the resolution is that it cannot be expressed *in the tables* — one more
#: level of indirection, applied per column rather than per type, is enough.


def element_of(column: dict) -> str:
    """The element type of an array column, as a scalar type name.

    # Errors

    Refuses an array column with no element type, and an array of arrays.
    Neither can reach here through a catalog `slate-serverd` printed — the
    schema layer refuses both at build time — so this is the same shape as the
    unreachable refusals elsewhere: the guarantee is one edit away from
    weakening, and the failure then would be generated code that compiles and
    decodes every element as the wrong type.
    """
    element = column.get("element_type")
    if element is None:
        raise Unknown(
            f"column `{column['name']}` is an array and the catalog gives it no "
            f"element type; the fingerprint hashes that, so a declaration "
            f"without it would be refused by the server"
        )
    if element == "array":
        raise Unknown(
            f"column `{column['name']}` is an array of arrays, which no schema "
            f"this generator can be given contains: an element type is a single "
            f"type and cannot name an element type of its own"
        )
    return element


def declared(column: dict, types: dict[str, str], element_keyword: str) -> str:
    """The declaration a client's `Column` takes: the type, plus what it needs.

    `element_keyword` is how the language spells the second argument — Python
    takes `element=`, Go `Element:`, TypeScript `element:` — and the empty
    string means the caller assembles it itself.
    """
    kind = types.get(column["type"])
    if kind is None:
        raise Unknown(f"no spelling for `{column['type']}`")
    if column["type"] != "array":
        return kind
    return f"{kind}{element_keyword}{types[element_of(column)]}"


def field_of(column: dict, fields: dict[str, tuple[str, str]], shape: str) -> tuple[str, str]:
    """The decoded field's native type and its runtime check, per column.

    `shape` is how the language spells "a list of these" — `Sequence[{}]`,
    `[]{}`, `{}[]` — and the runtime half of the pair becomes the *element's*
    check for an array, because an array's own check is the same in every case
    and the element's is what differs.
    """
    if column["type"] != "array":
        return fields[column["type"]]
    native, runtime = fields[element_of(column)]
    return shape.format(native), runtime


def python_encode(column: dict, expression: str) -> str:
    """The Python expression that turns a decoded field back into a wire value.

    An array needs its *elements* wrapped, not itself: `u64` and `i64` are both
    a plain `int` after decoding, so a list of them carries no tag and the
    server would be the first to notice. Where the element needs no wrapping —
    a `str`, a `bool` — the comprehension would be noise, so the list passes
    through `Array` alone.
    """
    if column["type"] != "array":
        return PYTHON_ENCODE[column["type"]].format(expression)
    inner = PYTHON_ENCODE[element_of(column)]
    if inner == "{}":
        return f"Array({expression})"
    # A list comprehension rather than a generator expression: `Array.__new__`
    # takes a `Sequence`, and a generator is not one. It works at run time —
    # `tuple.__new__` consumes any iterable — and `ty` refuses it, which is the
    # check finding something the tests could not.
    return f"Array([{inner.format('_e')} for _e in {expression}])"


class GoElement(typing.NamedTuple):
    """What Go needs to build one array column's elements."""

    #: The local variable the loop fills. Suffixed rather than bare, because a
    #: column called `out`, `row`, `v` or `i` would otherwise shadow something
    #: the generated function already uses.
    var: str
    #: The native element type, for the conversion out of the wire type.
    native: str


def go_element(column: dict) -> GoElement:
    native, _ = GO_FIELDS[element_of(column)]
    lowered = go_field(column["name"])
    return GoElement(var=lowered[:1].lower() + lowered[1:] + "Elements", native=native)


BANNER = "Generated by scripts/codegen.py. Do not edit."

#: The catalog type mapped onto the *native* type a decoded row field holds,
#: as distinct from the `ValueType` spellings above. Those name the type for
#: the declaration the fingerprint hashes; these name what the caller gets
#: after decoding, which is a different question with different answers —
#: `decimal` declares as `DECIMAL` and decodes to a count of units.
PYTHON_FIELDS = {
    "bool": ("bool", "bool"),
    "bytes": ("bytes", "bytes"),
    "string": ("str", "str"),
    "i64": ("int", "int"),
    "u64": ("int", "int"),
    "f64": ("float", "float"),
    "uuid": ("bytes", "bytes"),
    "vector": ("Sequence[float]", "tuple"),
    "decimal": ("Units", "Units"),
}

GO_FIELDS = {
    "bool": ("bool", "slate.Bool"),
    "bytes": ("[]byte", "slate.Bytes"),
    "string": ("string", "slate.String"),
    "i64": ("int64", "slate.Int"),
    "u64": ("uint64", "slate.Uint"),
    "f64": ("float64", "slate.Float"),
    "uuid": ("[16]byte", "slate.UUID"),
    "vector": ("[]float32", "slate.Vector"),
    "decimal": ("slate.Units", "slate.Units"),
}

TYPESCRIPT_FIELDS = {
    "bool": ("boolean", "bool"),
    "bytes": ("Uint8Array", "bytes"),
    "string": ("string", "string"),
    "i64": ("bigint", "int"),
    "u64": ("bigint", "uint"),
    "f64": ("number", "float"),
    "uuid": ("Uint8Array", "uuid"),
    "vector": ("number[]", "vector"),
    "decimal": ("bigint", "units"),
}


#: How a decoded field is turned back into a wire value, per language.
#:
#: The *reason* this is generated rather than left to the caller is sharpest in
#: Python: `u64` and `i64` both decode to a plain `int`, so a caller assembling
#: a row by hand has to remember which columns carry which tag, and getting it
#: wrong is refused by the server rather than by anything nearer. That is the
#: write-side twin of a wrong ordinal, and the decoders have had a guard
#: against their version of it since they shipped.
#:
#: `{}` is the field expression. A type whose decoded form is already the wire
#: form passes through unwrapped.
PYTHON_ENCODE = {
    "bool": "{}",
    "bytes": "{}",
    "string": "{}",
    "i64": "i64({})",
    "u64": "u64({})",
    "f64": "{}",
    "uuid": "{}",
    "vector": "Vector({})",
    "decimal": "{}",
}

GO_ENCODE = {
    "bool": "slate.Bool({})",
    "bytes": "slate.Bytes({})",
    "string": "slate.String({})",
    "i64": "slate.Int({})",
    "u64": "slate.Uint({})",
    "f64": "slate.Float({})",
    "uuid": "slate.UUID({})",
    "vector": "slate.Vector({})",
    "decimal": "{}",
}


#: A check whose predicate is exactly `column in ('a', 'b', …)` over a string
#: column describes an enumeration, and an enumeration is a *type* in two of
#: the three target languages. This matches that shape and nothing else.
#:
#: Deliberately narrow. A general predicate parser in Python would be a second
#: implementation of `lang/pred.rs` — the thing this repository has spent two
#: tasks proving is expensive to keep in step — so anything this does not
#: recognise in full is left alone and the field keeps its plain type. A
#: narrowing that is sometimes right and sometimes silently absent is worse
#: than one that is obviously all-or-nothing.
ENUM_CHECK = re.compile(
    r"^\s*(?P<column>[A-Za-z_][A-Za-z0-9_]*)\s+in\s*\((?P<values>[^()]*)\)\s*$",
    re.IGNORECASE,
)
#: One single-quoted SQL string literal, with `''` as the escape.
ENUM_VALUE = re.compile(r"^\s*'((?:[^']|'')*)'\s*$")


def enumerations(table: dict) -> dict[str, list[str]]:
    """Column name -> the values a check restricts it to.

    Only for a `str` column: `status in (1, 2)` over an integer is a range
    somebody wrote as a set, and turning it into a union of numeric literals
    would be a type that says less than `int` does about arithmetic.
    """
    by_name = {column["name"]: column for column in live_columns(table)}
    found: dict[str, list[str]] = {}
    for check in table.get("checks", []):
        predicate = check.get("predicate")
        if not predicate:
            continue
        match = ENUM_CHECK.match(predicate)
        if not match:
            continue
        column = by_name.get(match.group("column"))
        if column is None or column["type"] != "string":
            continue
        values = []
        for piece in match.group("values").split(","):
            literal = ENUM_VALUE.match(piece)
            if literal is None:
                values = []
                break
            values.append(literal.group(1).replace("''", "'"))
        if values:
            # Last one wins if two checks restrict the same column, which is a
            # schema nobody should write; the declaration still lists both.
            found[column["name"]] = values
    return found


def quoted(values: list[str], quote: str = '"') -> str:
    """Values as a comma-separated list of quoted literals."""
    return ", ".join(f"{quote}{value}{quote}" for value in values)


def row_columns(table: dict) -> list[tuple[int, dict]]:
    """The columns a row *type* carries, with the ordinal each sits at.

    Dropped columns are skipped as fields and their ordinals are not reused:
    the slot is still in the row and every later column still sits where the
    catalog says. `live_columns` keeps them because the *declaration* must —
    the fingerprint hashes which columns are dropped — and a row type must not,
    because a field nobody can read is noise. The two differ on purpose.
    """
    return [
        (at, column)
        for at, column in enumerate(live_columns(table))
        if column.get("dropped_in") is None
    ]


def type_name(table: dict) -> str:
    """A table's row type: the table name, CamelCased, not de-pluralised.

    `books` becomes `Books`, not `Book`. Guessing the singular is guessing
    about English, and it is wrong on `data`, `series`, `status` and every
    domain noun that is already plural — a generator that renames a table
    behind the caller's back is worse than one whose names are dull.
    """
    return "".join(part.capitalize() for part in table["name"].replace("-", "_").split("_"))


def go_field(name: str) -> str:
    """A column name as an exported Go field.

    Mechanically Title-cased per underscore-separated part, so `created_at`
    becomes `CreatedAt` and `id` becomes `Id` — not `ID`, which is what Go
    style would want. Deliberate: an initialism list is a second thing to keep
    in step with, it is never complete, and a field whose name depends on
    whether `api` made the list is worse than one that is uniformly dull.
    """
    return "".join(part.capitalize() for part in name.split("_"))


class Unknown(Exception):
    """A catalog type no target language has a spelling for.

    Raised rather than passed through or guessed at. A generator that emitted
    an unknown type as a string would produce a file that fails to compile,
    which is survivable; one that guessed would produce a declaration that
    compiles and hashes to the wrong fingerprint, which is the failure this
    whole tool exists to prevent.
    """


#: Catalog types no generator emits yet, and what each one would need.
#:
#: Separate from "no spelling for this type", which is what happens to a type
#: nobody has thought about: these are types the *rest* of the system supports
#: and this tool does not, so the message says what is missing rather than
#: implying the type does not exist.
#:
#: An array needs more than a row in the six type tables below. Its declaration
#: has to carry the element type — the fingerprint hashes it, so a declaration
#: without it is refused by the server — and its decoded form is element-typed
#: in all three languages (`Sequence[str]`, `[]string`, `string[]`), which the
#: tables cannot express because they are keyed by the column's type alone.
#: Each language also needs a per-element encode, since `u64` and `i64` are
#: different wire arms and a list of them cannot pass through unwrapped.
UNSUPPORTED: dict[str, str] = {}


def refuse_unsupported(tables: list[dict]) -> None:
    """Refuse a catalog this tool cannot generate, before emitting anything.

    Up front and in one place, because the alternative is what was here: the
    declaration emitters refuse an unknown type with a readable `Unknown`, and
    the *row* emitters index the same tables directly and raise `KeyError`. Two
    failures for one cause, one of them unreadable, and which one you got
    depended on the order the emitters ran in.
    """
    for table in tables:
        for column in table.get("columns", ()):
            reason = UNSUPPORTED.get(column.get("type", ""))
            if reason is not None:
                raise Unknown(
                    f"table `{table['name']}` column `{column['name']}` is "
                    f"{column['type']}, which this generator does not emit: {reason}"
                )


def catalog(config: Path, serverd: str) -> dict:
    """The resolved catalog, as the node would serve it."""
    printed = subprocess.run(
        [serverd, "--config", str(config), "--print-schema"],
        capture_output=True,
        text=True,
        check=False,
    )
    if printed.returncode != 0:
        raise SystemExit(
            f"`{serverd} --config {config} --print-schema` failed "
            f"({printed.returncode}):\n{printed.stderr}"
        )
    return json.loads(printed.stdout)


def declared_views(printed: dict, tables: list[dict]) -> list[tuple[str, str]]:
    """Each view as `(name, base table)`, in the order the catalog lists them.

    A view's *declaration* is its base table's, under the view's name. That is
    the whole of what a client needs and it is not a simplification: a view may
    not narrow columns — `docs/views.md` refuses a projection, because it would
    make the caller's ordinals view ordinals — so a view's ordinals are its base
    table's, and the server checks a claim about a view under the view's name
    against exactly those columns (`fingerprint::check_named`).

    So nothing per-column is emitted for a view, and there is nothing here that
    could disagree with the table: each emitter builds the view's declaration
    from the table's own, in the generated file, rather than writing a second
    column list beside it.

    The base table is required to be one this file emits. It cannot fail today
    — `slate-serverd` resolves every view against the catalog before it starts
    — but "cannot fail" is a property of the server's wiring, and a generator
    that indexed a missing name would emit a file that does not compile with no
    idea why.
    """
    known = {table["name"] for table in tables}
    out = []
    for view in printed.get("views", []):
        base = view["table"]
        if base not in known:
            raise Unknown(
                f"view `{view['name']}` reads table `{base}`, which is not in "
                f"the catalog this file declares"
            )
        out.append((view["name"], base))
    return out


def live_columns(table: dict) -> list[dict]:
    """The columns a client declares: every one, in ordinal order.

    Dropped columns are *kept*. A dropped column holds its ordinal — that is
    the whole design of the schema layer's `dropped_in`, so that a column
    removed from the middle does not re-point everything after it — and a
    declaration that skipped it would shift every later ordinal by one and
    silently read the wrong column, which is the exact failure being generated
    away from. The ordinals are asserted contiguous rather than trusted.
    """
    columns = sorted(table["columns"], key=lambda column: column["ordinal"])
    for at, column in enumerate(columns):
        if column["ordinal"] != at:
            raise SystemExit(
                f"table `{table['name']}` has a gap at ordinal {at}: "
                f"`{column['name']}` claims {column['ordinal']}. "
                "A declaration built from this would be wrong."
            )
    return columns


def key_names(table: dict, columns: list[dict]) -> list[str]:
    """The primary key, as names rather than the ordinals the catalog holds.

    Every client's declaration names its key columns, because a key given as
    ordinals would be a second place to get an ordinal wrong.
    """
    return [columns[ordinal]["name"] for ordinal in table["primary_key"]]


def spelling(table: dict, types: dict[str, str]) -> str:
    """A table's constant name: the table name, upper-cased.

    `books` becomes `BOOKS`. Collisions are impossible because a catalog cannot
    hold two tables with the same name, and case-folding cannot merge two
    distinct names that differ only in case — the schema layer refuses those
    too.
    """
    del types
    return table["name"].upper().replace("-", "_")


def parent_names(tables: list[dict]) -> dict[int, str]:
    """Table id to table name, for resolving a foreign key's parent.

    `--print-schema` publishes a key's parent as an **id**, because `TableDef`
    holds it that way and resolving it server-side would mean searching the
    catalog for something the reader can look up in the same document. This is
    that look-up, done once.
    """
    return {table["id"]: table["name"] for table in tables}


def foreign_keys(table: dict, parents: dict[int, str]) -> list[tuple[str, str, str, str]]:
    """A table's foreign keys as (name, child, parent, on_delete).

    Resolved rather than passed through. A parent id in a generated file would
    make the caller do the look-up the generator is here to do, and the id is
    the one part of a catalog that means nothing outside it.
    """
    out = []
    for key in table.get("foreign_keys", []):
        parent = parents.get(key["parent"])
        if parent is None:
            # Reachable only from a catalog the server would itself refuse, so
            # this is a bug in the generator or in `--print-schema` rather
            # than in anybody's schema. Loud either way: a generated file with
            # a wrong parent name reads a row of the wrong table.
            raise SystemExit(
                f"table `{table['name']}` has a foreign key `{key['name']}` whose "
                f"parent is table {key['parent']}, which is not in this catalog."
            )
        out.append((key["name"], table["name"], parent, key.get("on_delete", "restrict")))
    return out


def go_foreign_keys(table: dict, name: str, parents: dict[int, str]) -> list[str]:
    """A table's foreign keys, as Go data."""
    keys = foreign_keys(table, parents)
    if not keys:
        return []
    out = [
        f"// {name}ForeignKeys is every foreign key on `{table['name']}`, by name.",
        "//",
        "// The parent is the point. A `Relation` names a relationship by the",
        "// child table and the key, which is all the server needs — but",
        "// `Related` also wants the table its rows decode as, and for a",
        "// `Parents` read that is the parent, which no client can derive. It",
        "// was a string the caller typed; now it is generated. The wrong one",
        "// is refused by the schema check rather than mis-decoded, which was",
        "// measured and is why this is a convenience and not a bug fix.",
        f"var {name}ForeignKeys = map[string]slate.ForeignKey{{",
    ]
    for key, child, parent, on_delete in keys:
        out.append(
            f'\t"{key}": {{Name: "{key}", Child: "{child}", '
            f'Parent: "{parent}", OnDelete: "{on_delete}"}},'
        )
    out.extend(["}", ""])
    return out


def typescript_foreign_keys(table: dict, name: str, parents: dict[int, str]) -> list[str]:
    """A table's foreign keys, as a TypeScript record."""
    keys = foreign_keys(table, parents)
    if not keys:
        return []
    out = [
        "/**",
        f" * Every foreign key on `{table['name']}`, by name.",
        " *",
        " * The parent is the point. A `Relation` names a relationship by the",
        " * child table and the key, which is all the server needs — but",
        " * `related` also wants the table its rows decode as, and for a",
        ' * `"parents"` read that is the parent, which no client can derive.',
        " * The wrong one is refused by the schema check rather than",
        " * mis-decoded, which was measured; this is a convenience, not a fix.",
        " */",
        f"export const {name}ForeignKeys: Record<string, ForeignKey> = {{",
    ]
    for key, child, parent, on_delete in keys:
        out.append(
            f'  "{key}": {{ name: "{key}", child: "{child}", '
            f'parent: "{parent}", onDelete: "{on_delete}" }},'
        )
    out.extend(["};", ""])
    return out


def go_checks(table: dict, name: str) -> list[str]:
    """A table's `CHECK` constraints, as Go data.

    A `map[string]CheckRule` rather than a generated struct per check: the set
    is data the server owns and a caller looks up by name, which is what the
    `check` key in a refusal's details already hands them.
    """
    checks = table.get("checks", [])
    if not checks:
        return []
    out = [
        f"// {name}Checks is every `CHECK` on `{table['name']}`, by name.",
        "//",
        "// Published rather than restated: checks are outside the schema",
        "// fingerprint, so a client cannot derive them and would otherwise learn",
        "// each rule from a refusal. The `check` key in a violation's details is",
        "// the key here.",
        f"var {name}Checks = map[string]slate.CheckRule{{",
    ]
    for check in checks:
        parts = []
        for field, key in (("column", "Column"), ("message", "Message"), ("predicate", "Predicate")):
            value = check.get(field)
            spelled = '""' if value is None else '"' + value.replace('"', '\\"') + '"'
            parts.append(f"{key}: {spelled}")
        out.append(f'\t"{check["name"]}": {{{", ".join(parts)}}},')
    out.extend(["}", ""])
    return out


def typescript_checks(table: dict, name: str) -> list[str]:
    """A table's `CHECK` constraints, as a TypeScript record."""
    checks = table.get("checks", [])
    if not checks:
        return []
    out = [
        "/**",
        f" * Every `CHECK` on `{table['name']}`, by name.",
        " *",
        " * Published rather than restated: checks are outside the schema",
        " * fingerprint, so a client cannot derive them and would otherwise learn",
        " * each rule from a refusal.",
        " */",
        f"export const {name}Checks: Record<string, CheckRule> = {{",
    ]
    for check in checks:
        parts = []
        for field, key in (("column", "column"), ("message", "message"), ("predicate", "predicate")):
            value = check.get(field)
            spelled = "null" if value is None else '"' + value.replace('"', '\\"') + '"'
            parts.append(f"{key}: {spelled}")
        out.append(f'  "{check["name"]}": {{ {", ".join(parts)} }},')
    out.extend(["};", ""])
    return out


def python_rows(tables: list[dict]) -> list[str]:
    """Typed rows for Python: a frozen dataclass and a decoder per table."""
    out = [
        "",
        "",
        "def _field(values: Sequence[object], at: int, table: str, column: str,",
        "           kind: type | tuple[type, ...], nullable: bool) -> object:",
        '    """One column of a row, checked.',
        "",
        "    The check is the point. Python would happily hand back whatever sat",
        "    at the ordinal, and a declaration that is one column out reads the",
        "    neighbour and looks plausible — which is the failure this whole",
        "    generator exists to make impossible. Raising here names the table",
        "    and the column, so the error says where to look.",
        '    """',
        "    value = values[at]",
        "    if value is None or isinstance(value, Null):",
        "        if nullable:",
        "            return None",
        "        raise ValueError(",
        '            f"{table}.{column} is not nullable and came back null"',
        "        )",
        "    if not isinstance(value, kind):",
        "        raise TypeError(",
        '            f"{table}.{column} is {type(value).__name__}, "',
        '            f"not the declared {kind}"',
        "        )",
        "    return value",
    ]
    # Emitted only for a catalog that has an array column, because it refers to
    # `Array` and the import list is derived from the same predicate — see
    # `has_array_column`, and the defect that made it one function.
    if has_array_column(tables):
        out.extend([
        "",
        "",
        "def _elements(values: Sequence[object], at: int, table: str, column: str,",
        "              kind: type | tuple[type, ...], nullable: bool) -> object:",
        '    """One array column, with every element checked.',
        "",
        "    `_field` stops at the list. Its elements arrive already decoded to",
        "    native Python — an `Array` of `str`, not of tagged values — so a",
        "    caller reading one *looks* right whatever the column declared, and a",
        "    declaration naming the wrong element type would be found by the",
        "    server rather than here. The element type is not on the wire, so",
        "    this is the only place on this side that can notice.",
        '    """',
        '    value = _field(values, at, table, column, Array, nullable)',
        "    if not isinstance(value, Array):",
        "        # `_field` returns `None` exactly when the column was null and",
        "        # nullable; it has already refused anything that is neither.",
        "        # Spelled as the `isinstance` rather than `is None` because a",
        "        # checker cannot narrow `object` minus `None` to something",
        "        # iterable, and the loop below needs it to.",
        "        return value",
        "    for index, element in enumerate(value):",
        "        if not isinstance(element, kind):",
        "            raise TypeError(",
        '                f"{table}.{column}[{index}] is {type(element).__name__}, "',
        '                f"not the declared {kind}"',
        "            )",
        "    return value",
        ])
    out.append("")
    for table in tables:
        name = type_name(table)
        fields = row_columns(table)
        enums = enumerations(table)
        out.extend(
            [
                "",
                "@dataclass(frozen=True)",
                f"class {name}:",
                f'    """A row of `{table["name"]}`, decoded."""',
                "",
            ]
        )
        for _, column in fields:
            native, _ = field_of(column, PYTHON_FIELDS, "Sequence[{}]")
            # A `CHECK` restricting the column to a set of strings is an
            # enumeration, and the server already refuses anything outside it,
            # so the narrower type states a rule rather than inventing one.
            if column["name"] in enums:
                native = f"Literal[{quoted(enums[column['name']], chr(34))}]"
            hint = f"{native} | None" if column["nullable"] else native
            out.append(f"    {column['name']}: {hint}")
        width = len(live_columns(table))
        out.extend(
            [
                "",
                "    @classmethod",
                f"    def from_row(cls, values: Sequence[object]) -> {name}:",
                f'        """Decode a row of `{table["name"]}`, by ordinal."""',
                f"        if len(values) != {width}:",
                "            raise ValueError(",
                f'                f"{table["name"]} has {width} columns, got {{len(values)}}"',
                "            )",
                "        return cls(",
            ]
        )
        for at, column in fields:
            native, runtime = field_of(column, PYTHON_FIELDS, "Sequence[{}]")
            nullable = "True" if column["nullable"] else "False"
            # An array goes through `_elements`, which checks every element
            # against the *element* type rather than the column's own; see the
            # helper's docstring for why that is the only place it can happen.
            reader = "_elements" if column["type"] == "array" else "_field"
            if column["name"] in enums:
                # **Single** quotes, because this hint goes inside the
                # double-quoted cast target below and a `Literal["a"]` closes
                # it early. The annotation on the dataclass field above uses
                # double quotes, which is what ruff wants in real code; here
                # the type is the contents of a string and the inner quoting
                # is ours to pick.
                #
                # Found by running the demo: the generated module was a
                # `SyntaxError`, and every check in CI passed on it. `ruff`
                # does not parse a file it is not given, the codegen tests
                # asserted on the *annotation* line and never the cast, and
                # the one test that executes generated Python used a catalog
                # with no enumerated column in it.
                native = f"Literal[{quoted(enums[column['name']], chr(39))}]"
            hint = f"{native} | None" if column["nullable"] else native
            # The cast target is a *string*. `cast(str | None, …)` builds a
            # union object at run time on every row, and importing `Optional`
            # for the alternative leaves it unused on any catalog with no
            # nullable column — which ruff then strips out of the generated
            # file, and a generated file a linter edits is drift by Tuesday.
            out.append(
                f'            {column["name"]}=cast("{hint}", '
                f'{reader}(values, {at}, "{table["name"]}", "{column["name"]}", '
                f"{runtime}, {nullable})),"
            )
        out.extend(["        )", ""])

        # The write side. `u64` and `i64` both decode to a plain `int`, so a
        # caller assembling a row by hand has to remember which tag each column
        # wants, and getting it wrong is refused by the *server* — a long way
        # from the mistake, and with nothing naming the column.
        out.extend(
            [
                "    def to_row(self) -> list[object]:",
                '        """Encode this row in the column order of '
                + f'`{table["name"]}`."""',
                "        return [",
            ]
        )
        for _, column in fields:
            wrapped = python_encode(column, f"self.{column['name']}")
            if column["nullable"]:
                out.append(
                    f"            NULL if self.{column['name']} is None "
                    f"else {wrapped},"
                )
            else:
                out.append(f"            {wrapped},")
        out.extend(["        ]", ""])

        # `soft_delete` is published as an ordinal, so the generator knows
        # which column carries the retirement stamp and the row type can say
        # so. Without this a caller reads a nullable `i64` and has to know, out
        # of band, that *this* one means "gone" — which is the knowledge the
        # catalog holds and the client was not being told.
        stamp = table.get("soft_delete")
        if stamp is not None:
            column = next(c for at, c in fields if at == stamp)
            out.extend(
                [
                    "    @property",
                    "    def retired(self) -> bool:",
                    '        """Whether this row has been soft-deleted."""',
                    f"        return self.{column['name']} is not None",
                    "",
                    # The write-side twin. It was generated once before and
                    # reverted, because at the time the server refused every
                    # write at a retired row's key and a helper for an
                    # operation the database cannot perform is worse than none:
                    # a caller reaches for it, writes the row back and gets
                    # `NotFound` from a row they are holding. That is fixed;
                    # see `ledger/2026-09-20-the-write-that-names-a-key.md`.
                    f"    def restored(self) -> {name}:",
                    '        """This row with its soft delete cleared, ready to write back.',
                    "",
                    "        Restoring is an ordinary `update` or `upsert` — there is no",
                    "        restore verb — so this only clears the column; sending it is",
                    "        the caller's. It needs the `read_deleted` action, the same",
                    "        grant `include_deleted` needs, because a write that names a",
                    "        retired row's key reaches it only for a caller who may see it.",
                    "",
                    "        Writing the row back *unchanged* does not work and is not",
                    "        meant to: the column is the server's, and a row carrying a",
                    "        timestamp is refused naming that column.",
                    '        """',
                    f"        return replace(self, {column['name']}=None)",
                    "",
                ]
            )
    return out


def go_rows(tables: list[dict]) -> list[str]:
    """Typed rows for Go: a struct and a scanner per table."""
    parents = parent_names(tables)
    # No leading blank: the declaration block above already ends with one, and
    # two in a row is the only thing `gofmt -l` had to say about this file.
    out: list[str] = []
    for table in tables:
        name = type_name(table)
        fields = row_columns(table)
        # Go has no union of string literals, so the field stays `string` and
        # the allowed values are emitted beside it. A named type with a const
        # block was the alternative and buys nothing a plain `string` does not
        # already have: Go would still let any string be converted into it, so
        # it would look like a check and not be one.
        for column, values in enumerations(table).items():
            out.extend(
                [
                    f"// {name}{go_field(column)}Values is every value the `{table['name']}`",
                    f"// check allows in `{column}`. The server enforces it; this is here so a",
                    "// caller can offer the choices without asking, and is not a type because",
                    "// Go has no union of string literals.",
                    f"var {name}{go_field(column)}Values = []string{{{quoted(values)}}}",
                    "",
                ]
            )
        for line in go_checks(table, name):
            out.append(line)
        for line in go_foreign_keys(table, name, parents):
            out.append(line)
        out.extend([f"// {name} is a row of `{table['name']}`, decoded.", f"type {name} struct {{"])
        # Padded to the longest field name, because that is what `gofmt` does
        # to a struct and a generator whose output `gofmt -l` lists is a
        # generator someone will reformat by hand and then regenerate over.
        widest = max((len(go_field(c["name"])) for _, c in fields), default=0)
        for _, column in fields:
            native, _ = field_of(column, GO_FIELDS, "[]{}")
            # A nullable column is a pointer: Go has no other shape that can
            # hold "absent" for an int64, and a zero would be a real value.
            hint = f"*{native}" if column["nullable"] else native
            out.append(f"\t{go_field(column['name']).ljust(widest)} {hint}")
        out.extend(["}", ""])

        width = len(live_columns(table))
        out.extend(
            [
                f"// Scan{name} decodes one row of `{table['name']}`, by ordinal.",
                "//",
                "// Every column is type-asserted rather than cast. A declaration one",
                "// column out would otherwise read the neighbour and return it, which",
                "// compiles and is wrong; this returns an error naming the column.",
                f"func Scan{name}(row []slate.Value) ({name}, error) {{",
                f"\tvar out {name}",
                f"\tif len(row) != {width} {{",
                f'\t\treturn out, fmt.Errorf("{table["name"]} has {width} columns, got %d", len(row))',
                "\t}",
            ]
        )
        for at, column in fields:
            native, element_wire = field_of(column, GO_FIELDS, "[]{}")
            # An array asserts `slate.Array` and then each element separately;
            # `field_of` gave the *element's* wire type for that second step.
            array = column["type"] == "array"
            wire = "slate.Array" if array else element_wire
            field = go_field(column["name"])
            out.append(f"\tif _, null := row[{at}].(slate.Null); !null {{")
            out.append(f"\t\tv, ok := row[{at}].({wire})")
            out.append("\t\tif !ok {")
            out.append(
                f'\t\t\treturn out, fmt.Errorf("{table["name"]}.{column["name"]}: '
                f'expected {wire}, got %T", row[{at}])'
            )
            out.append("\t\t}")
            if array:
                # A `slate.Array` is a `[]slate.Value`, so its members still
                # carry their own types and a cast to `[]string` would be a
                # lie the compiler cannot see. Checked one at a time, and the
                # error names the position: "tags[2]" is findable, "tags" is
                # not.
                element = go_element(column)
                out.append(f"\t\t{element.var} := make({native}, len(v))")
                out.append("\t\tfor i, e := range v {")
                out.append(f"\t\t\tev, ok := e.({element_wire})")
                out.append("\t\t\tif !ok {")
                out.append(
                    f'\t\t\t\treturn out, fmt.Errorf("{table["name"]}.{column["name"]}[%d]: '
                    f'expected {element_wire}, got %T", i, e)'
                )
                out.append("\t\t\t}")
                out.append(f"\t\t\t{element.var}[i] = {element.native}(ev)")
                out.append("\t\t}")
                if column["nullable"]:
                    out.append(f"\t\tout.{field} = &{element.var}")
                else:
                    out.append(f"\t\tout.{field} = {element.var}")
            elif column["nullable"]:
                out.append(f"\t\tvalue := {native}(v)")
                out.append(f"\t\tout.{field} = &value")
            else:
                out.append(f"\t\tout.{field} = {native}(v)")
            out.append("\t} else {")
            if column["nullable"]:
                out.append(f"\t\tout.{field} = nil")
            else:
                out.append(
                    f'\t\treturn out, fmt.Errorf("{table["name"]}.{column["name"]} '
                    'is not nullable and came back null")'
                )
            out.append("\t}")
        out.extend(["\treturn out, nil", "}", ""])

        # The write side. Go keeps `uint64` and `int64` apart, so the tag
        # cannot be confused the way it can in Python or TypeScript; what this
        # buys here is the *order*, which is what the decoder buys from the
        # other end.
        out.extend(
            [
                f"// Row encodes r in the column order of `{table['name']}`.",
                "//",
                "// The twin of the decoder above. A caller building this slice by hand",
                "// gets no help with the order, and a transposition the server happens",
                "// to accept is a row written wrong with nothing to say so.",
                f"func (r {name}) Row() []slate.Value {{",
                f"\tout := make([]slate.Value, 0, {width})",
            ]
        )
        for _, column in fields:
            field = go_field(column["name"])
            if column["type"] == "array":
                # A loop rather than a format string, because `slate.Array` is
                # a `[]slate.Value` and every element has to be wrapped in its
                # own type on the way out. Written inline instead of as a
                # generated helper per element type: one helper would be
                # shared by two columns of different element types and would
                # need a type parameter for no gain, and `gofmt` is happy with
                # either.
                element = go_element(column)
                inner = GO_ENCODE[element_of(column)]
                source = f"*r.{field}" if column["nullable"] else f"r.{field}"
                body = [
                    f"\t{element.var} := make(slate.Array, 0, len({source}))",
                    f"\tfor _, e := range {source} {{",
                    f"\t\t{element.var} = append({element.var}, {inner.format('e')})",
                    "\t}",
                    f"\tout = append(out, {element.var})",
                ]
                if column["nullable"]:
                    out.append(f"\tif r.{field} == nil {{")
                    out.append("\t\tout = append(out, slate.Null{})")
                    out.append("\t} else {")
                    out.extend("\t" + line for line in body)
                    out.append("\t}")
                else:
                    out.extend(body)
            elif column["nullable"]:
                encoded = GO_ENCODE[column["type"]].format(f"*r.{field}")
                out.extend(
                    [
                        f"\tif r.{field} == nil {{",
                        "\t\tout = append(out, slate.Null{})",
                        "\t} else {",
                        f"\t\tout = append(out, {encoded})",
                        "\t}",
                    ]
                )
            else:
                encoded = GO_ENCODE[column["type"]].format(f"r.{field}")
                out.append(f"\tout = append(out, {encoded})")
        out.extend(["\treturn out", "}", ""])

        # See the Python emitter: the catalog knows which column is the
        # retirement stamp and the row type should not make the caller know it
        # too.
        stamp = table.get("soft_delete")
        if stamp is not None:
            column = next(c for at, c in fields if at == stamp)
            field = go_field(column["name"])
            out.extend(
                [
                    f"// Retired reports whether this row of `{table['name']}` has been",
                    "// soft-deleted.",
                    f"func (r {name}) Retired() bool {{",
                    f"\treturn r.{field} != nil",
                    "}",
                    "",
                    # See the Python emitter for why this exists now and did
                    # not before. A value receiver, so the copy is the point:
                    # the caller's row is not mutated by asking for a restored
                    # one.
                    "// Restored returns this row with its soft delete cleared, ready to",
                    "// write back with Update or Upsert. There is no restore verb; this",
                    "// only clears the column. It needs the `read_deleted` action, the",
                    "// same grant include_deleted needs. Writing the row back unchanged",
                    "// is refused naming the column.",
                    f"func (r {name}) Restored() {name} {{",
                    f"\tr.{field} = nil",
                    "\treturn r",
                    "}",
                    "",
                ]
            )
    return out


def typescript_rows(tables: list[dict]) -> list[str]:
    """Typed rows for TypeScript: an interface and a decoder per table."""
    parents = parent_names(tables)
    out = [""]
    for table in tables:
        name = type_name(table)
        fields = row_columns(table)
        enums = enumerations(table)
        out.extend(typescript_checks(table, name))
        out.extend(typescript_foreign_keys(table, name, parents))
        out.extend([f"/** A row of `{table['name']}`, decoded. */", f"export interface {name} {{"])
        for _, column in fields:
            native, _ = field_of(column, TYPESCRIPT_FIELDS, "{}[]")
            if column["name"] in enums:
                native = " | ".join(f'"{value}"' for value in enums[column["name"]])
            hint = f"{native} | null" if column["nullable"] else native
            out.append(f"  {column['name']}: {hint};")
        out.extend(["}", ""])

        width = len(live_columns(table))
        out.extend(
            [
                "/**",
                f" * Decode one row of `{table['name']}`, by ordinal.",
                " *",
                " * Every column's tag is checked rather than assumed. A declaration one",
                " * column out would otherwise return the neighbour, which type-checks",
                " * and is wrong; this throws naming the column.",
                " */",
                f"export function decode{name}(row: Value[]): {name} {{",
                f"  if (row.length !== {width}) {{",
                f"    throw new Error(`{table['name']} has {width} columns, got ${{row.length}}`);",
                "  }",
                "  return {",
            ]
        )
        for at, column in fields:
            native, tag = field_of(column, TYPESCRIPT_FIELDS, "{}[]")
            if column["name"] in enums:
                native = " | ".join(f'"{value}"' for value in enums[column["name"]])
            nullable = "true" if column["nullable"] else "false"
            # `elements` for an array, and the tag it is given is the
            # *element's* — the array's own is `"array"` and is spelled inside
            # the helper, because there is only one thing it can be.
            reader = "elements" if column["type"] == "array" else "field"
            out.append(
                f'    {column["name"]}: {reader}(row, {at}, "{table["name"]}", '
                f'"{column["name"]}", "{tag}", {nullable}) as {native}'
                + (" | null," if column["nullable"] else ",")
            )
        out.extend(["  };", "}", ""])

        # The write side, the twin of the decoder above. It buys the order and
        # the tag together: `int` and `uint` are both `bigint` here, so a
        # hand-built row can carry the wrong one and typecheck perfectly.
        out.extend(
            [
                "/**",
                f" * Encode one row of `{table['name']}` in the catalog column order.",
                " *",
                " * `int` and `uint` are both `bigint` on this side, so a hand-built row",
                " * can carry the wrong tag and still typecheck. This cannot.",
                " */",
                f"export function encode{name}(row: {name}): Value[] {{",
                "  return [",
            ]
        )
        for _, column in fields:
            _, tag = field_of(column, TYPESCRIPT_FIELDS, "{}[]")
            ref = f"row.{column['name']}"
            if column["type"] == "array":
                # Each element wrapped in its own tag, which is what the
                # decoder's `elements` unwrapped. `int` and `uint` are both
                # `bigint` here, so a list of them carries nothing to tell
                # them apart until the server refuses it.
                live = (
                    f'{{ kind: "array", value: {ref}.map((e) => '
                    f'({{ kind: "{tag}", value: e }})) }}'
                )
            else:
                live = f'{{ kind: "{tag}", value: {ref} }}'
            if column["nullable"]:
                out.append(f'    {ref} === null ? {{ kind: "null" }} : {live},')
            else:
                out.append(f"    {live},")
        out.extend(["  ];", "}", ""])

        # See the Python emitter.
        stamp = table.get("soft_delete")
        if stamp is not None:
            column = next(c for at, c in fields if at == stamp)
            out.extend(
                [
                    "/**",
                    f" * Whether this row of `{table['name']}` has been soft-deleted.",
                    " */",
                    f"export function isRetired{name}(row: {name}): boolean {{",
                    f"  return row.{column['name']} !== null;",
                    "}",
                    "",
                    # See the Python emitter.
                    "/**",
                    f" * This row of `{table['name']}` with its soft delete cleared, ready to",
                    " * write back with `update` or `upsert`. There is no restore verb; this",
                    " * only clears the column. It needs the `read_deleted` action, the same",
                    " * grant `includeDeleted` needs. Writing the row back unchanged is",
                    " * refused naming the column.",
                    " */",
                    f"export function restored{name}(row: {name}): {name} {{",
                    f"  return {{ ...row, {column['name']}: null }};",
                    "}",
                    "",
                ]
            )
    return out


def has_array_column(tables: list[dict]) -> bool:
    """Whether any live column in this catalog is an array.

    One predicate, consulted by both the import list and the helper that needs
    the import, because gating them separately is what went wrong: `_elements`
    was emitted unconditionally and `Array` was imported only when a catalog
    had an array column. The demo has one, so the two conditions coincided
    there and its generated file was fine; the retention example has none, and
    its file referred to a name it had not imported. `ruff` found it, in CI, on
    a file nobody had touched — which is the whole argument for one predicate.
    """
    return any(column["type"] == "array" for table in tables for _, column in row_columns(table))


def python_value_imports(tables: list[dict]) -> str:
    """The `slate.values` names this catalog's generated code refers to.

    `Null` is used by the decoder's null test and `NULL` by the encoder, so a
    catalog with any column at all needs both; the rest follow the column
    types actually present.
    """
    needed = {"Null", "NULL"}

    def wants(kind: str) -> None:
        if kind in ("i64", "u64"):
            needed.add(kind)
        elif kind == "decimal":
            needed.add("Units")
        elif kind == "vector":
            needed.add("Vector")

    for table in tables:
        for _, column in row_columns(table):
            kind = column["type"]
            if kind == "array":
                # The list itself and, separately, whatever its elements need:
                # an `array<i64>` encodes as `Array(i64(e) for e in …)` and so
                # refers to both. Missed at first, and the generated module was
                # a `NameError` at import — which `ty` and `ruff` do not see,
                # because neither is given a file that does not exist yet.
                needed.add("Array")
                wants(element_of(column))
            else:
                wants(kind)
    return ", ".join(sorted(needed, key=import_order))


def import_order(name: str) -> tuple[int, str]:
    """`ruff`'s isort order for a `from … import a, b, c` list.

    Not plain alphabetical: with `order-by-type` — ruff's default — the names
    are grouped by what their *spelling* says they are, constants first, then
    classes, then everything else. Sorting flat happened to agree while the
    list was `NULL, Null, Units, Vector, i64, u64`, and stopped agreeing the
    moment `Array` was added, because `Array` sorts before `NULL` and is a
    class rather than a constant. `ruff check` on the generated file said so;
    nothing else would have, since a generated file is not read by hand.
    """
    if name.isupper():
        return (0, name)
    if name[:1].isupper():
        return (1, name)
    return (2, name)


def python_module(tables: list[dict], views: list[tuple[str, str]]) -> str:
    """The Python client's declaration."""
    parents = parent_names(tables)
    out = [
        '"""The server\'s tables, as this client must declare them.',
        "",
        f"{BANNER}",
        "",
        "A client holds its own copy of the schema and the server checks it:",
        "every request carries a fingerprint, and a declaration that disagrees",
        "with the catalog is refused rather than answered positionally-wrong.",
        "This file is that declaration, produced from the catalog itself so the",
        "two cannot drift.",
        '"""',
        "",
        "from __future__ import annotations",
        "",
        "from collections.abc import Sequence",
        # `replace` only when some table soft-deletes, for the same reason
        # `Literal` is conditional: ruff strips an unused import out of the
        # generated file, and a generated file a linter edits has drifted by
        # the next `--check`.
        (
            "from dataclasses import dataclass, replace"
            if any(table.get("soft_delete") is not None for table in tables)
            else "from dataclasses import dataclass"
        ),
        # `Literal` only when a check actually narrows a field. An unused
        # import is what ruff strips out of the generated file, and a
        # generated file a linter edits has drifted by the next `--check` —
        # the same failure the `Optional` import had, caught the same way.
        (
            "from typing import Literal, cast"
            if any(enumerations(table) for table in tables)
            else "from typing import cast"
        ),
        "",
        "from slate import Column, Table, ValueType",
        # Exactly the value names this catalog's encoders and decoders use, and
        # no others. An unused import is what ruff strips out of a generated
        # file, and a generated file a linter edits has drifted by the next
        # `--check`; a *missing* one is worse and was how this was found — the
        # first version of the encoders imported nothing new and emitted
        # `u64(...)`, which every static check in CI passed and which raised
        # `NameError` the moment a row was encoded.
        f"from slate.values import {python_value_imports(tables)}",
        "",
        "__all__ = [",
    ]
    names = [spelling(table, PYTHON_TYPES) for table in tables]
    rows = [type_name(table) for table in tables]
    # `sorted` alone interleaves `AUTHORS` and `Authors`, because `U` sorts
    # before `u`. Ruff's RUF022 wants the all-caps names as one group and the
    # CamelCase ones as another, and a generated file a linter wants to edit is
    # a generated file that drifts.
    exported = sorted(
        [*names, *rows, "BY_NAME"], key=lambda name: (0 if name.isupper() else 1, name)
    )
    out.extend(f'    "{name}",' for name in exported)
    out.extend(["]", ""])

    for table in tables:
        columns = live_columns(table)
        out.append(f"{spelling(table, PYTHON_TYPES)} = Table(")
        out.append(f'    "{table["name"]}",')
        out.append("    [")
        for column in columns:
            kind = declared(column, PYTHON_TYPES, ", element=")
            scale = "" if column["scale"] is None else f", scale={column['scale']}"
            out.append(f'        Column("{column["name"]}", {kind}{scale}),')
        out.append("    ],")
        key = ", ".join(f'"{name}"' for name in key_names(table, columns))
        out.append(f"    primary_key=[{key}],")
        out.extend([")", ""])
        # The constraints, as data. Not part of the declaration the fingerprint
        # hashes — checks are excluded from it — so this is published rather
        # than restated: a form can show the rule beside the field before the
        # person submits, instead of learning it from a refusal.
        checks = table.get("checks", [])
        if checks:
            out.append(f"{spelling(table, PYTHON_TYPES)}_CHECKS = {{")
            for check in checks:
                out.append(f'    "{check["name"]}": {{')
                for field in ("column", "message", "predicate"):
                    value = check.get(field)
                    spelled = "None" if value is None else '"' + value.replace('"', '\\"') + '"'
                    out.append(f'        "{field}": {spelled},')
                out.append("    },")
            out.extend(["}", ""])
        # And the foreign keys, for the reason the Go and TypeScript files
        # give: the parent table is what a `parents` read decodes as, and it
        # is the one thing a client holding only a `Relation` cannot work out.
        keys = foreign_keys(table, parents)
        if keys:
            out.append(f"{spelling(table, PYTHON_TYPES)}_FOREIGN_KEYS = {{")
            for key, child, parent, on_delete in keys:
                out.append(f'    "{key}": {{')
                out.append(f'        "name": "{key}",')
                out.append(f'        "child": "{child}",')
                out.append(f'        "parent": "{parent}",')
                out.append(f'        "on_delete": "{on_delete}",')
                out.append("    },")
            out.extend(["}", ""])

    joined = ", ".join(names)
    out.append(f"BY_NAME = {{table.name: table for table in ({joined},)}}")
    if views:
        out.extend(["", *python_views(views)])
    out.extend(python_rows(tables))
    return "\n".join(out)


def python_views(views: list[tuple[str, str]]) -> list[str]:
    """The view declarations, each built from its base table's above."""
    out = [
        "#: The views the catalog declares, for `Query(VIEWS_BY_NAME[name])`.",
        "#:",
        "#: Each is its base table's columns under the view's name, built from",
        "#: the declaration above rather than written out again. A view may not",
        "#: narrow columns, so its ordinals are its base table's and there is",
        "#: nothing here that could disagree; the server checks a claim about a",
        "#: view under the view's own name against exactly those columns.",
        "#:",
        "#: Separate from `BY_NAME` because a view is not a table: only a plain",
        "#: query reads through one, and every other request naming it is",
        "#: refused.",
    ]
    made = []
    for name, base in views:
        constant = name.upper()
        table = base.upper()
        out.append(
            f'{constant} = Table("{name}", {table}.columns, {table}.primary_key)'
        )
        made.append(constant)
    joined = ", ".join(made)
    out.append(f"VIEWS_BY_NAME = {{view.name: view for view in ({joined},)}}")
    return out


def go_views(views: list[tuple[str, str]]) -> list[str]:
    """The Go view declarations, and the one helper they need."""
    out = [
        "// named is a table's declaration under a different name, which is",
        "// exactly what a view's is: a view may not narrow columns, so its",
        "// ordinals are its base table's and only the name differs.",
        "//",
        "// It copies the struct and reassigns one field. The Columns slice is",
        "// shared with the table's declaration, which is correct — neither is",
        "// written after this file is loaded — and is why this is not a deep",
        "// copy.",
        "func named(name string, base slate.TableDef) slate.TableDef {",
        "\tbase.Name = name",
        "\treturn base",
        "}",
        "",
        "// Views is every view the catalog declares, ready for",
        "// `client.Declaring(schema.Views)` beside the tables.",
        "//",
        "// Separate from `Tables` because a view is not a table: only a plain",
        "// query reads through one, and every other request naming it is",
        "// refused.",
        "var Views = slate.Schemas{",
    ]
    for name, base in views:
        out.append(f'\t"{name}": named("{name}", Tables["{base}"]),')
    out.append("}")
    return out


def go_file(tables: list[dict], views: list[tuple[str, str]], package: str) -> str:
    """The Go client's declaration."""
    out = [
        f"// {BANNER}",
        "",
        "// A client holds its own copy of the schema and the server checks it:",
        "// every request carries a fingerprint, and a declaration that",
        "// disagrees with the catalog is refused rather than answered",
        "// positionally-wrong. This file is that declaration, produced from the",
        "// catalog itself so the two cannot drift.",
        f"package {package}",
        "",
        "import (",
        '\t"fmt"',
        "",
        '\t"github.com/howlerops/slate-orm/clients/go/slate"',
        ")",
        "",
        "// Tables is every table the catalog declares, ready for",
        "// `client.Declaring(schema.Tables)`.",
        "var Tables = slate.Schemas{",
    ]
    for table in tables:
        columns = live_columns(table)
        out.append(f'\t"{table["name"]}": {{')
        out.append(f'\t\tName: "{table["name"]}",')
        out.append("\t\tColumns: []slate.ColumnDef{")
        for column in columns:
            kind = declared(column, GO_TYPES, ", Element: ")
            scale = "" if column["scale"] is None else f", Scale: {column['scale']}"
            out.append(f'\t\t\t{{Name: "{column["name"]}", Type: {kind}{scale}}},')
        out.append("\t\t},")
        key = ", ".join(f'"{name}"' for name in key_names(table, columns))
        out.append(f"\t\tPrimaryKey: []string{{{key}}},")
        out.append("\t},")
    out.extend(["}", ""])
    if views:
        out.extend([*go_views(views), ""])
    out.extend(go_rows(tables))
    return "\n".join(out)


def typescript_views(views: list[tuple[str, str]]) -> list[str]:
    """The TypeScript view declarations, each spread from its base table's."""
    out = [
        "/** Every view the catalog declares, for",
        " * `client.declaring({ ...TABLES, ...VIEWS })`.",
        " *",
        " * Each is its base table's declaration with the view's name, spread",
        " * from the constant above rather than written out again. A view may",
        " * not narrow columns, so its ordinals are its base table's and there",
        " * is nothing here that could disagree.",
        " *",
        " * Separate from `TABLES` because a view is not a table: only a plain",
        " * query reads through one, and every other request naming it is",
        " * refused.",
        " */",
        "export const VIEWS: Schemas = {",
    ]
    for name, base in views:
        out.append(f'  "{name}": {{ ...{base.upper()}, name: "{name}" }},')
    out.append("};")
    return out


def typescript_module(tables: list[dict], views: list[tuple[str, str]]) -> str:
    """The TypeScript client's declaration."""
    out = [
        "// " + BANNER,
        "//",
        "// A client holds its own copy of the schema and the server checks it:",
        "// every request carries a fingerprint, and a declaration that",
        "// disagrees with the catalog is refused rather than answered",
        "// positionally-wrong. This file is that declaration, produced from the",
        "// catalog itself so the two cannot drift.",
        "",
        'import type { CheckRule, ForeignKey, Schemas, TableDef, Value } from "@slate-orm/client";',
        "",
        "/**",
        " * One column of a row, with its tag checked.",
        " *",
        " * The check is the point. A declaration one column out would otherwise",
        " * hand back the neighbour, which type-checks at the call site and is",
        " * wrong; this throws naming the table and the column.",
        " */",
        "function field(",
        "  row: Value[],",
        "  at: number,",
        "  table: string,",
        "  column: string,",
        "  kind: string,",
        "  nullable: boolean,",
        "): unknown {",
        "  const value = row[at];",
        '  if (value === undefined || value.kind === "null") {',
        "    if (nullable) return null;",
        "    throw new Error(`${table}.${column} is not nullable and came back null`);",
        "  }",
        "  if (value.kind !== kind) {",
        "    throw new Error(",
        "      `${table}.${column} is ${value.kind}, not the declared ${kind}`,",
        "    );",
        "  }",
        '  return "value" in value ? value.value : undefined;',
        "}",
        "",
    ]
    # Same gate as Python's `_elements`, and for a weaker reason that is still
    # a reason: an unused function is not a TypeScript error today, and a
    # generated file full of code no catalog needs is one `noUnusedLocals`
    # away from being one.
    if has_array_column(tables):
        out.extend([
            "/**",
            " * One array column, with every element checked against its declared type.",
            " *",
            " * `field` above stops at the array: its value is a `Value[]`, whose members",
            " * still carry their own tags, so a cast to `string[]` at the call site would",
            " * be a lie nothing can see. The element type is not on the wire either, so",
            " * this is the only place on this side that can notice — and the error names",
            " * the position, because `tags[2]` is findable and `tags` is not.",
            " */",
            "function elements(",
            "  row: Value[],",
            "  at: number,",
            "  table: string,",
            "  column: string,",
            "  kind: string,",
            "  nullable: boolean,",
            "): unknown {",
            '  const value = field(row, at, table, column, "array", nullable);',
            "  if (value === null) return null;",
            "  return (value as Value[]).map((element, index) => {",
            "    if (element.kind !== kind) {",
            "      throw new Error(",
            "        `${table}.${column}[${index}] is ${element.kind}, not the declared ${kind}`,",
            "      );",
            "    }",
            '    return "value" in element ? element.value : undefined;',
            "  });",
        "}",
        ])
    out.append("")
    names = []
    for table in tables:
        columns = live_columns(table)
        name = spelling(table, TYPESCRIPT_TYPES)
        names.append(name)
        out.append(f"export const {name}: TableDef = {{")
        out.append(f'  name: "{table["name"]}",')
        out.append("  columns: [")
        for column in columns:
            kind = declared(column, TYPESCRIPT_TYPES, ", element: ")
            scale = "" if column["scale"] is None else f", scale: {column['scale']}"
            out.append(f'    {{ name: "{column["name"]}", type: {kind}{scale} }},')
        out.append("  ],")
        key = ", ".join(f'"{name}"' for name in key_names(table, columns))
        out.append(f"  primaryKey: [{key}],")
        out.extend(["};", ""])

    out.append("/** Every table the catalog declares, for `client.declaring(TABLES)`. */")
    out.append("export const TABLES: Schemas = {")
    out.extend(f"  [{name}.name]: {name}," for name in names)
    out.extend(["};", ""])
    if views:
        out.extend([*typescript_views(views), ""])
    out.extend(typescript_rows(tables))
    return "\n".join(out)


def web_module(tables: list[dict], views: list[tuple[str, str]]) -> str:
    """A browser module: table and view names, mapped to their column names.

    # Why this is not the TypeScript target above

    `typescript_module` emits a *client declaration* — `TableDef`s that a
    `Client` sends, importing `@slate-orm/client`. A browser app that only
    labels columns cannot have that: the SDK is a gRPC client, and pulling it
    into a bundle to read a list of strings would be absurd. So this emits
    plain data and imports nothing.

    # Why it is generated rather than written

    `examples/explorer/web/src/api.ts` held these two maps by hand, guarded by
    a test that re-parsed `head.toml` with a regex. That guard worked, and it
    was the *second implementation of resolution* this generator's own
    docstring warns about four paragraphs in: ordinals come from declaration
    order, a primary key is named and resolved, a decimal's scale is validated,
    and a regex over the TOML knows none of it. It happened to agree because
    the demo's schema is simple.

    Reading the resolved catalog removes the second implementation. What stays
    hand-written in the app is which tables the UI *shows*, which is a UI
    decision with reasons and not a copy of anything.

    # Dropped columns

    Included, unlike a row type's fields. A dropped column still occupies its
    ordinal, and this module's consumer indexes a row by position to put a
    header over it — so skipping one would shift every later label by one,
    which is precisely the quiet wrong this generator exists to prevent.
    """
    out = [
        "// " + BANNER,
        "//",
        "// Names only: no import, no `TableDef`, nothing from the SDK. A browser",
        "// app uses this to label columns, and the SDK is a gRPC client.",
        "//",
        "// Every column the catalog carries, in ordinal order, dropped ones",
        "// included — a dropped column keeps its slot in the row, so leaving it",
        "// out would shift every later header onto the wrong value.",
        "",
        "/** Every table the catalog declares, as name -> column names in ordinal order. */",
        "export const CATALOG_TABLES: Record<string, string[]> = {",
    ]
    for table in tables:
        names = ", ".join(f'"{column["name"]}"' for column in live_columns(table))
        out.append(f'  {table["name"]}: [{names}],')
    out.extend([
        "};",
        "",
        "/** Every view, as name -> its base table's column names.",
        " *",
        " * The same array object as the base table's, not a copy: a view may not",
        " * narrow columns, so its ordinals *are* the table's and a second list is",
        " * a second thing to get wrong.",
        " */",
        "export const CATALOG_VIEWS: Record<string, string[]> = {",
    ])
    for name, base in views:
        out.append(f'  {name}: CATALOG_TABLES[\"{base}\"]!,')
    out.append("};")
    return "\n".join(out) + "\n"


def emit(path: Path, body: str, check: bool) -> bool:
    """Write `body` to `path`, or compare and report. True means agreement."""
    if check:
        if not path.exists():
            print(f"miss  {path} does not exist", file=sys.stderr)
            return False
        if path.read_text(encoding="utf-8") != body:
            print(f"drift {path} is not what the catalog says", file=sys.stderr)
            return False
        print(f"ok    {path}")
        return True
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")
    print(f"wrote {path}")
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--python", type=Path)
    parser.add_argument("--go", type=Path)
    parser.add_argument("--go-package", default="schema")
    parser.add_argument("--typescript", type=Path)
    parser.add_argument(
        "--web",
        type=Path,
        help="a plain-data TypeScript module of names, importing nothing",
    )
    parser.add_argument(
        "--serverd",
        # The same variable the client harnesses use, so a run with a prebuilt
        # binary needs no second thing to set. A path that is set and missing is
        # a hard error in `catalog`, never a silent fall back to building.
        default=os.environ.get("SLATE_SERVERD", "target/debug/slate-serverd"),
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="compare instead of writing; non-zero if anything has drifted",
    )
    arguments = parser.parse_args()

    if not (
        arguments.python or arguments.go or arguments.typescript or arguments.web
    ):
        parser.error("name at least one of --python, --go, --typescript, --web")

    printed = catalog(arguments.config, arguments.serverd)
    tables = printed["tables"]
    refuse_unsupported(tables)
    views = declared_views(printed, tables)
    agreed = True
    if arguments.python:
        agreed &= emit(arguments.python, python_module(tables, views), arguments.check)
    if arguments.go:
        body = go_file(tables, views, arguments.go_package)
        agreed &= emit(arguments.go, body, arguments.check)
    if arguments.typescript:
        agreed &= emit(
            arguments.typescript, typescript_module(tables, views), arguments.check
        )
    if arguments.web:
        agreed &= emit(arguments.web, web_module(tables, views), arguments.check)

    if not agreed:
        print(
            "\nthe checked-in declarations disagree with the catalog; "
            "re-run without --check",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
