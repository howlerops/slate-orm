# A slow reader makes a response later, not larger — so the body is weighed while the clock still stops at the head

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `crates/slate-serverd` (`src/observe.rs`, `tests/observing.rs`),
  `site/docs/deployment.html`, `docs/caveat-status.json`
- **Kind:** feature

## What changed

`slate-serverd`'s request layer now weighs every response body. The `Trailing`
wrapper, which existed to catch a failure raised in a trailer, adds each data
frame's length and counts the frame; two new families,
`slate_response_bytes_total` and `slate_response_frames_total`, appear on
`/metrics`, and `resp_bytes` / `resp_frames` appear at the end of the summary
line. The recorded *duration* is unchanged and still stops at the response
head.

The wrapper's `late: Option<Late>` became a `row: Arc<Method>` and a
`late: bool`. A field that is `take`n cannot hold what the per-frame path
needs, and two `Arc`s to one `Method` would have been the alternative.

## Why

Four open caveats across three entries said the same thing in different words:
a streamed read's rows are in no number this node reports. They were read as
one problem and they are two, which is why both stayed open.

The *time* half has a real defence, written in the module doc since it was
built: timing to the last row measures how fast the client reads, so a slow
consumer would be reported as a slow server. That argument is correct and it is
why the head duration is the right duration.

**It is also an argument about time, and nobody noticed it does not reach
bytes.** A slow reader makes a response arrive later. It does not make the
response larger. The size of a body has no consumer confound at all — it is a
property of the message, exactly as
`ledger/2026-09-29-bytes-do-not-need-a-network.md` argued about the request
side — so it could have been counted at any point in the last two months and
was not, because it was filed behind a caveat whose reason was about something
else.

That is the shape worth remembering: a good reason for not measuring one thing
kept a second thing unmeasured for as long as the two were named together.

The second reason is that this is the missing half of a comparison. Each of the
three clients weighs the request it sends, and
`ledger/2026-09-29-the-same-framing-cost-in-three-clients.md` used that to find
that a batch of twenty costs *more* bytes than twenty singles. Nothing weighed
what came back, in any language, so no request could be compared with its
response. Both sides are now the same unit: a protobuf payload.

## Alternatives rejected

**Time to the last frame, as a second duration beside the head.** The obvious
reading of the four caveats, and the one they ask for. Rejected because it
reintroduces exactly the confound the module doc rejects: a scan draining
slowly against an inattentive client would land in the histogram as server
latency, and a histogram that mixes the two is worse than one that admits it
measures the head — a number with a stated limit can be reasoned about, a
number that silently means two things cannot. The caveats stay open, now with
this as the reason rather than "nobody has done it".

**Time to the *first* frame.** Genuinely unconfounded — the consumer has not
had a chance to apply backpressure before the first frame — and it would say
what the head duration hides for a streamed read. Not taken here because it is
a duration and this change is deliberately not about durations: it needs its
own histogram (26 more series a method), and it needs first establishing
whether tonic sends this daemon's head before or after the handler produces a
row, which is a measurement nobody has made. Named as the next thing rather
than done badly.

**Accumulate in the wrapper and flush on `Drop`.** Fewer atomics — one pair per
response rather than per frame. Rejected because a scan that streams for a
minute would contribute nothing to a scrape taken during it, and a scrape taken
while the node is busy is the one that matters. The cost avoided is two relaxed
`fetch_add`s against a wrapper already measured at 4.7 ns a frame, on a path
consulted once per `rows_per_message` rows rather than once per row.

**A histogram of response sizes rather than counters.** Would answer "how big
is a typical response" instead of only "how much is this node sending". Not
taken: it is 26 series a method against 2, and the question actually being
asked is a rate. A reader wanting a mean divides by `calls`, which is exported
beside it.

**Count the HTTP/2 framing and the headers too.** That is what "bytes on the
wire" literally means, and it is not what this counts. Rejected because the
value here is comparability with the clients' own request weights, which are
`ByteSize()` / `proto.Size()` / the channel's serializer — all protobuf
payloads. A server number that included framing and a client number that did
not would invite a subtraction that means nothing. The help strings and the
module doc both say "protobuf payload" rather than "wire".

## Evidence

`cargo test -p slate-serverd --bins --test observing --no-fail-fast`: **203
passed** in the binary suite and **14 passed** in `observing`, up from 200 and
13. Four new cases, three of them unit and one end to end.

Five mutations, over three runs because the container ran out of disk partway
through the first (recorded as `outcome: problems`, scoring nothing — the
failure mode `mutate.py`'s second lie is about, caught by the script rather
than read as five survivors):

| mutation | outcome |
| --- | --- |
| the weight is a frame count, not a length | caught, all four cases |
| a frame counts twice | caught, the three unit cases |
| the summary prints bytes and frames the other way round | caught, two cases |
| the exported bytes family reads the frame count | caught, two cases |
| only an already-ended body is wrapped, so nothing is weighed | **caught by the end-to-end case alone** |

The last row is the one worth reading. The three unit cases build a body by
hand and hand it to `Trailing` directly, so they pass unchanged when the layer
stops wrapping anything — the mutation makes the counters unreachable in
production and invisible to every test that does not go through a real node.
`a_scrape_weighs_what_the_node_answered` was written for that risk before the
mutation confirmed it, which is the one order in which this evidence is worth
anything.

Its assertions are `frames >= 1` and `bytes > frames`, not a literal. A literal
would pin the encoded size of the `docs` fixture — a fact about the fixture,
not about the instrument — and would go red on a column added to it.
`bytes > frames` is what separates a weight from a count, and rows are tens of
bytes, so the margin is not close.

The records:
`ledger/mutations/20260929T182849-crates-slate-serverd-src-observe-rs.json`
is the run that scored nothing, and its `restored_clean` is `null` because it
never got that far — the tree was put back and checked by hand. The three that
scored are
`ledger/mutations/20260929T183301-crates-slate-serverd-src-observe-rs.json`,
`ledger/mutations/20260929T183415-crates-slate-serverd-src-observe-rs.json`
and
`ledger/mutations/20260929T183651-crates-slate-serverd-src-observe-rs.json`,
each reporting `restored_clean: true`.

## What this does not do

**It measures no latency, and that is now a decision rather than a gap.** The
four caveats about the head duration stay open. The reason recorded against
them is the confound above, not an absence of effort.

**It does not count the framing, the headers or the trailers.** `data_ref`
answers `None` for a trailers frame, which is what makes the frame count a
message count; everything below the protobuf payload is invisible here. A
response's real cost on a link is larger than this number by an amount nothing
measures.

**A head failure's body is not weighed.** The layer wraps only a response that
succeeded at the head and has a body left, so a refusal contributes zero. That
is arithmetically right — there is nothing to count — but it means the counter
is "bytes sent by calls that answered" rather than "bytes sent", and a node
whose traffic is mostly refusals will show a response weight far below what it
actually put on the socket.

**Nothing compares a request with its response.** Both are measured now, in the
same unit, in different processes, and no test or harness puts the two numbers
side by side. That comparison is the reason this was worth doing and it has not
been done.

**The per-frame cost is inherited, not re-measured.** The 4.7 ns a frame is the
figure `ledger/2026-09-18-the-failure-that-arrives-after-the-answer-has-started.md`
measured
for the wrapper as it was, before two atomics were added to it. Two relaxed
`fetch_add`s are a few nanoseconds by any account and the conclusion does not
turn on the exact number, but the number in this entry is the old one and the
new path has not been timed.
