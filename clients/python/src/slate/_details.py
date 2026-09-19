"""The stable reason token out of a status's `grpc-status-details-bin`.

The head node puts a `google.rpc.ErrorInfo` in every status's details —
`crates/slate-server/src/status.rs` — because the status code alone is
many-to-one: `UNAVAILABLE` covers a fenced writer, a stale replica, no replica
and a failed object-store call, and `RESOURCE_EXHAUSTED` covers a join build,
a group count, a distinct count, a sort and a predicate write that matched more
rows than it may return. The token is how a caller tells them apart without
matching on prose.

# Why this builds its own descriptors instead of importing `google.rpc`

The obvious implementation is to generate `google/rpc/status.proto` and
`error_details.proto` into this package the way `slate/v1/records.proto` is
generated, and import the result. **That breaks `import slate` outright** for
anyone who also has `googleapis-common-protos` installed — which is most
people, since `grpcio-status`, every `google-cloud-*` package and the OTLP
exporter all depend on it.

Generated modules register themselves in protobuf's *default* descriptor pool
under the proto's own path, and two files with that path whose descriptors are
not identical are a hard error at import time:

    TypeError: Couldn't build proto file into descriptor pool:
    duplicate file name google/rpc/error_details.proto

Identical copies are tolerated, which is what makes this trap quiet: this
repository's `error_details.proto` declares `ErrorInfo` alone, upstream's
declares ten messages, so the descriptors differ and the collision is certain
rather than possible. It was demonstrated before this module was written.

So the three messages are declared here, in a pool of this module's own, under
a file name nothing else can claim. Protobuf is structural — field numbers and
wire types are the contract, names are not — so these decode the head node's
bytes exactly as generated `google.rpc` classes would. The cost is that the
declarations below must stay in step with `proto/google/rpc/` by hand; they are
seven fields that have not changed since 2015, and `test_details.py` decodes a
blob built from the real thing.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Final

from google.protobuf import descriptor_pb2, descriptor_pool, message_factory

#: The `Any.type_url` the head node packs its `ErrorInfo` under.
#: `status.rs::ERROR_INFO_URL`.
ERROR_INFO_URL: Final = "type.googleapis.com/google.rpc.ErrorInfo"

_Field = descriptor_pb2.FieldDescriptorProto

_pool = descriptor_pool.DescriptorPool()
_file = descriptor_pb2.FileDescriptorProto()
# Neither the file name nor the package may be one anything else could declare,
# which is the whole point of the module docstring above.
_file.name = "slate/_status_details.proto"
_file.package = "slate._rpc"
_file.syntax = "proto3"

# `google.protobuf.Any`: type_url = 1, value = 2.
_any = _file.message_type.add(name="Any")
_any.field.add(
    name="type_url", number=1, type=_Field.TYPE_STRING, label=_Field.LABEL_OPTIONAL
)
_any.field.add(
    name="value", number=2, type=_Field.TYPE_BYTES, label=_Field.LABEL_OPTIONAL
)

# `google.rpc.Status`: code = 1, message = 2, details = 3 (repeated Any).
_status = _file.message_type.add(name="Status")
_status.field.add(
    name="code", number=1, type=_Field.TYPE_INT32, label=_Field.LABEL_OPTIONAL
)
_status.field.add(
    name="message", number=2, type=_Field.TYPE_STRING, label=_Field.LABEL_OPTIONAL
)
_status.field.add(
    name="details",
    number=3,
    type=_Field.TYPE_MESSAGE,
    label=_Field.LABEL_REPEATED,
    type_name=".slate._rpc.Any",
)

# `google.rpc.ErrorInfo`: reason = 1, domain = 2, metadata = 3.
_info = _file.message_type.add(name="ErrorInfo")
_info.field.add(
    name="reason", number=1, type=_Field.TYPE_STRING, label=_Field.LABEL_OPTIONAL
)
_info.field.add(
    name="domain", number=2, type=_Field.TYPE_STRING, label=_Field.LABEL_OPTIONAL
)
# The metadata map, which used to be left undeclared on the grounds that
# surfacing it "means deciding what a client promises about keys that vary per
# variant". That was right when every variant invented its own keys. It is no
# longer the whole story: the check-violation keys are specified — `violations`
# is a count and `check.N`, `column.N`, `message.N` are indexed from zero — so
# there is exactly one shape a client can promise something about.
#
# So the map is parsed and *not* exposed raw. `check_failures_of` reads the one
# documented shape into typed values and nothing hands a caller the dictionary,
# which keeps the original objection answered rather than overruled: a key this
# client has no contract for still reaches nobody.
#
# A `map<string, string>` on the wire is a repeated message of key/value pairs
# with the `map_entry` option set, which is what this builds by hand.
_entry = _info.nested_type.add(name="MetadataEntry")
_entry.field.add(
    name="key", number=1, type=_Field.TYPE_STRING, label=_Field.LABEL_OPTIONAL
)
_entry.field.add(
    name="value", number=2, type=_Field.TYPE_STRING, label=_Field.LABEL_OPTIONAL
)
_entry.options.map_entry = True
_info.field.add(
    name="metadata",
    number=3,
    type=_Field.TYPE_MESSAGE,
    label=_Field.LABEL_REPEATED,
    type_name=".slate._rpc.ErrorInfo.MetadataEntry",
)

_pool.Add(_file)
_Status = message_factory.GetMessageClass(_pool.FindMessageTypeByName("slate._rpc.Status"))
_ErrorInfo = message_factory.GetMessageClass(
    _pool.FindMessageTypeByName("slate._rpc.ErrorInfo")
)


def reason_of(blob: bytes) -> str:
    """The `ErrorInfo.reason` in a `grpc-status-details-bin` blob, or `""`.

    `""` for a blob that carries no `ErrorInfo`, and for one that does not
    parse. A client that raised while building an error message would replace
    the server's failure with its own, which is strictly worse than losing a
    token: the caller would no longer know why the call failed at all.

    The `ErrorInfo.metadata` map is not returned *here*. The token alone is
    what `errors.py` needs to branch below a status code. See
    `check_failures_of` for the one part of that map this client reads.
    """
    # `Any`, because these classes are built at import from a descriptor and
    # `message_factory` types them as the base `Message`: the field names below
    # exist at runtime and are invisible to a type checker, which is the price
    # of not importing `google.rpc`. The field names are pinned by the tests
    # rather than by the type checker, which is the weaker of the two guarantees and is
    # said here rather than left for a reader to discover.
    try:
        status: Any = _Status()
        status.ParseFromString(blob)
        for detail in status.details:
            if detail.type_url == ERROR_INFO_URL:
                info: Any = _ErrorInfo()
                info.ParseFromString(detail.value)
                return str(info.reason)
    except Exception:  # pragma: no cover - see the docstring
        return ""
    return ""


@dataclass(frozen=True)
class CheckFailure:
    """One `CHECK` a refused row violated.

    The point of the whole thing: a form can put `message` next to `column`
    without parsing it out of the status text, which is the contract a caller
    would otherwise have to invent — and which would last until somebody
    reworded a sentence.
    """

    #: The constraint's name, as the schema declares it.
    check: str
    #: The column it is about, or `None` for one spanning several. A caller
    #: with nowhere to put it shows it beside the form rather than a field.
    column: str | None
    #: The sentence to show, or `None` where the schema wrote none.
    message: str | None


def check_failures_of(blob: bytes) -> list[CheckFailure]:
    """Every check a refused write violated, in declaration order.

    Empty for any failure that is not a check violation, and for a blob that
    does not parse — a client that raised while building an error message
    would replace the server's failure with its own, which is the reasoning
    `reason_of` already gives.

    Reads the indexed keys (`check.0`, `column.0`, `message.0`, …) rather than
    the unindexed pair, because the unindexed one is only the *first* failure
    and this is the call that exists to return all of them. A server old
    enough to send only the unindexed pair yields one failure here, which is
    the honest reading of what it said.
    """
    try:
        status: Any = _Status()
        status.ParseFromString(blob)
        for detail in status.details:
            if detail.type_url != ERROR_INFO_URL:
                continue
            info: Any = _ErrorInfo()
            info.ParseFromString(detail.value)
            if str(info.reason) != "CHECK_VIOLATION":
                return []
            data = dict(info.metadata)
            # The count, not the key set: a message or column may legitimately
            # be absent, so counting `check.N` keys would be right and
            # counting the map's size would not. The server sends `violations`
            # for exactly this.
            try:
                total = int(data.get("violations", ""))
            except ValueError:
                # Old enough to send the unindexed pair and no count. One
                # failure is what that means.
                name = data.get("check")
                if name is None:
                    return []
                return [
                    CheckFailure(name, data.get("column"), data.get("message"))
                ]
            out = []
            for at in range(total):
                name = data.get(f"check.{at}")
                if name is None:
                    # A gap means the server and this reader disagree about
                    # the shape. Returning the prefix would be a quiet lie
                    # about how many rules the row broke.
                    return []
                out.append(
                    CheckFailure(name, data.get(f"column.{at}"), data.get(f"message.{at}"))
                )
            return out
    except Exception:  # pragma: no cover - see the docstring
        return []
    return []
