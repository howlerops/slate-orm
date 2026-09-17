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

# `google.rpc.ErrorInfo`: reason = 1, domain = 2. The metadata map is field 3
# and is deliberately not declared — see `reason_of`.
_info = _file.message_type.add(name="ErrorInfo")
_info.field.add(
    name="reason", number=1, type=_Field.TYPE_STRING, label=_Field.LABEL_OPTIONAL
)
_info.field.add(
    name="domain", number=2, type=_Field.TYPE_STRING, label=_Field.LABEL_OPTIONAL
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

    The `ErrorInfo.metadata` map is *not* returned. The server populates it
    with a variant's payload — `index` and `table` on a unique violation,
    `limit` on a predicate write that matched too many — and surfacing it means
    deciding what a client promises about keys that vary per variant. The token
    alone is what `errors.py` needs to branch below a status code, and it is
    what three clients can agree on. Field 3 is therefore left undeclared and
    skipped as an unknown field.
    """
    # `Any`, because these classes are built at import from a descriptor and
    # `message_factory` types them as the base `Message`: the field names below
    # exist at runtime and are invisible to a type checker, which is the price
    # of not importing `google.rpc`. The field names are pinned by the tests
    # rather than by mypy, which is the weaker of the two guarantees and is
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
