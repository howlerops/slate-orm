"""Freshness: what a caller has to know about read tokens, and what it does not.

# Where the line is

`docs/topology.md` states the model: a commit returns the sequence it landed
at, a read carrying that sequence may only be served by a view that has reached
it, and a session threads the highest token it has seen so that no read is
served by a view behind one the caller already observed. That fixes
read-your-writes and monotonic reads with one mechanism.

The mechanism is also the thing a caller most wants not to think about. So this
package draws the line here:

- **A `Session` threads the watermark by itself.** Write, then read, and the
  read carries the sequence the write returned. A caller doing the ordinary
  thing never types the word "token", and cannot forget to. This is the
  default, because the alternative default — `ANY` — is correct until there is
  a replica, and then it is wrong in a way that only appears under load.
- **A caller who wants to reason about staleness has every part of it.**
  `Freshness` is a first-class argument on every read; `ReadToken` comes back
  from every commit; `ServedBy` comes back on every response and says which
  view answered and at what sequence; `Session.watermark` is readable.

The rejected shape was making freshness implicit and *only* implicit — a client
that always sends `AT_LEAST(watermark)` and offers no way out. It reads well
and it is unusable: it sends every read after the first write to the writer,
which is the scarcest resource in the deployment, and it gives an operator no
way to say "this dashboard may be a minute behind". Freshness is a real
decision and hiding it entirely is not simplification, it is removing the
control.

# The cost of monotonic reads, stated

Observing the sequence a *read* was served at, not just a write, is what makes
reads monotonic — no read may be served by a view behind one already seen. It
also means that one read served by the writer pins every later read in that
session to the writer, because nothing else can be that fresh. That is correct
and it is expensive, so it is a constructor flag (`monotonic_reads`) rather
than a law, and turning it off leaves read-your-writes intact — only the
weaker guarantee goes.
"""

from __future__ import annotations

import dataclasses

from ._proto.slate.v1 import records_pb2 as pb

__all__ = ["Freshness", "ReadToken", "ServedBy"]


@dataclasses.dataclass(frozen=True, order=True)
class ReadToken:
    """The durable sequence a commit landed at.

    A *durable* sequence, which is the limit worth restating: a replica reads
    object storage, so a write acknowledged before its flush is not on any
    replica yet and cannot be. Read-your-writes through a replica requires
    durable commits.
    """

    sequence: int


@dataclasses.dataclass(frozen=True)
class ServedBy:
    """Which view served a read, and how far it had got.

    One per response, including a response that read several tables: a join is
    several reads of *one* snapshot, so there is one replica and one sequence
    to report. Two would be a join across two points in time, which is not a
    state the database was ever in.
    """

    replica: str
    #: The sequence that view is known to include, or zero when it does not
    #: track one.
    sequence: int

    @staticmethod
    def from_proto(wire: pb.ServedBy) -> ServedBy:
        return ServedBy(replica=wire.replica, sequence=wire.sequence)


class Freshness:
    """How fresh a read has to be.

    Constructed through the three factories. There is no public constructor,
    because the wire's three levels are the whole vocabulary and a fourth would
    be this client inventing a guarantee the server does not make.
    """

    __slots__ = ("_proto",)

    def __init__(self, proto: pb.Freshness) -> None:
        self._proto = proto

    @staticmethod
    def any() -> Freshness:
        """Any replica, however far behind. The cheapest read."""
        return Freshness(pb.Freshness(any=True))

    @staticmethod
    def at_least(token: ReadToken | int) -> Freshness:
        """Only a view that has reached this sequence. This is read-your-writes."""
        sequence = token.sequence if isinstance(token, ReadToken) else token
        return Freshness(pb.Freshness(at_least=sequence))

    @staticmethod
    def latest() -> Freshness:
        """The writer, which is the only view that can see an unflushed write.

        A pool without a writer refuses this rather than substituting a
        replica, which is why it is a distinct level rather than a very large
        `at_least`.
        """
        return Freshness(pb.Freshness(latest=True))

    def to_proto(self) -> pb.Freshness:
        return self._proto

    def __repr__(self) -> str:
        level = self._proto.WhichOneof("level")
        if level == "at_least":
            return f"Freshness.at_least({self._proto.at_least})"
        return f"Freshness.{level}()"


class Watermark:
    """The highest sequence a session has observed.

    Internal to `Session`. Kept as a class rather than an `int | None` field so
    that "observe" is one method with one rule in it — take the maximum, never
    go backwards — rather than three call sites each remembering to.
    """

    __slots__ = ("_sequence",)

    def __init__(self) -> None:
        self._sequence: int | None = None

    @property
    def token(self) -> ReadToken | None:
        return None if self._sequence is None else ReadToken(self._sequence)

    def observe(self, sequence: int | None) -> None:
        if sequence is None or sequence == 0:
            # Zero is what a view that does not track a sequence reports, and
            # what a transaction-served read reports. It is not evidence of
            # anything, and recording it would make `token` say "at least 0",
            # which every view satisfies — harmless, but it would make the
            # watermark look set when it is not.
            return
        if self._sequence is None or sequence > self._sequence:
            self._sequence = sequence

    def freshness(self) -> Freshness | None:
        """What a read should ask for, or `None` for the wire's default (`ANY`)."""
        if self._sequence is None:
            return None
        return Freshness.at_least(self._sequence)
