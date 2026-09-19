package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"time"

	"github.com/howlerops/slate-orm/clients/go/slate"
	"github.com/howlerops/slate-orm/examples/explorer/backends/go/schema"
)

func (s *server) meta(ctx context.Context, session *slate.Session, _ json.RawMessage) (any, error) {
	status, err := s.clients["app"].Leadership(ctx)
	if err != nil {
		return nil, err
	}
	return map[string]any{
		"sdk":    "go",
		"leader": status.Leader,
		// Derived from the allowlist the query path enforces, not a second
		// literal beside it. The two were separate lists and drifted the
		// moment a table was added: the query path learned `shipments` and
		// this did not, so `/api/meta` reported three tables while
		// `/api/query` served four — and the conformance `meta` case caught
		// it because the node adapter had already been deriving its list.
		"tables": knownTables(),
	}, nil
}

func (s *server) query(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
	var spec querySpec
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the query: %w", err)
	}
	query, err := spec.build()
	if err != nil {
		return nil, err
	}
	stream, err := session.Query(ctx, query)
	if err != nil {
		return nil, err
	}
	rows, err := stream.Collect()
	if err != nil {
		return nil, err
	}
	out := make([][]tagged, 0, len(rows))
	for _, row := range rows {
		out = append(out, encodeRow(row))
	}
	return map[string]any{"rows": out}, nil
}

func (s *server) join(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
	var spec struct {
		Type  string  `json:"type"`
		Limit *uint64 `json:"limit"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the join: %w", err)
	}
	kinds := map[string]slate.JoinType{
		"inner": slate.Inner, "left": slate.Left,
		"right": slate.Right, "full": slate.Full,
	}
	kind, ok := kinds[spec.Type]
	if !ok {
		return nil, fmt.Errorf("no such join type: %s", spec.Type)
	}

	b := slate.NewJoin()
	authors := b.Add(slate.JoinInput{Table: "authors"})
	b.Add(slate.JoinInput{
		Table: "books",
		Type:  kind,
		On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
	})
	query := b.Query()
	query.Limit = spec.Limit

	stream, err := session.Join(ctx, query)
	if err != nil {
		return nil, err
	}
	rows, err := stream.Collect()
	if err != nil {
		return nil, err
	}

	out := make([]map[string]any, 0, len(rows))
	for _, row := range rows {
		entry := map[string]any{"authors": nil, "books": nil}
		if len(row) > 0 && row[0] != nil {
			entry["authors"] = encodeRow(row[0])
		}
		if len(row) > 1 && row[1] != nil {
			entry["books"] = encodeRow(row[1])
		}
		out = append(out, entry)
	}
	// A join's row order is the plan's business and the plan is the planner's.
	// Sorted so the three adapters are comparable and the table does not
	// reshuffle when a hint changes the algorithm.
	sortJoined(out)
	return map[string]any{"rows": out}, nil
}

// sortJoined orders joined rows for comparison across adapters.
//
// By the row's JSON, not by `fmt.Sprintf("%v")`: Go renders a map differently
// from `json.Marshal`, so the three adapters sorted by three different keys and
// produced three different orders for the same rows. The conformance runner
// caught it on the outer joins, where the unmatched row sorts to a different
// place under each rendering.
func sortJoined(rows []map[string]any) {
	key := func(row map[string]any) string {
		encoded, err := json.Marshal(row)
		if err != nil {
			// Unreachable for these rows, and a panic here would be worse than
			// an unstable order in a demo.
			return fmt.Sprintf("%v", row)
		}
		return string(encoded)
	}
	sort.SliceStable(rows, func(i, j int) bool { return key(rows[i]) < key(rows[j]) })
}

// buildAggregate turns the contract's aggregate body into the join and the
// grouping it names.
//
// Shared by `aggregate` and `explainAggregate` for the same reason the kernel
// shares its narrowing between running a grouped read and explaining one: an
// explanation of a *different* request is worse than none, and two copies of
// this twenty lines would diverge on the first change to either.
func buildAggregate(body json.RawMessage) (slate.JoinQuery, slate.Grouping, error) {
	var none slate.JoinQuery
	var spec struct {
		GroupBy string `json:"groupBy"`
		Having  *struct {
			MinCount uint64 `json:"minCount"`
		} `json:"having"`
		Sort      string  `json:"sort"`
		Direction string  `json:"direction"`
		Limit     *uint64 `json:"limit"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return none, slate.Grouping{}, fmt.Errorf("decoding the aggregate: %w", err)
	}

	b := slate.NewJoin()
	authors := b.Add(slate.JoinInput{Table: "authors"})
	books := b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
	})

	// Which column of the joined schema to group on.
	//
	// `decade` is not a column at all. It used to be refused here, in all three
	// adapters, with "needs a computed column, which this demo does not
	// declare" — true of the clients rather than of the database, since the
	// kernel has had scalar expressions throughout. Returning a hand-bucketed
	// answer would have been the adapter doing the database's job, so it was
	// left refused and recorded.
	//
	// It is declared now: `books.year / 10 * 10`, computed over the *joined*
	// row and named with `JoinComputed(0)`. Integer division truncates toward
	// zero, which is what a decade means for these years.
	var key slate.Column
	var compute []slate.Scalar
	switch spec.GroupBy {
	case "author":
		key = slate.At(0, 1) // authors.name
	case "country":
		key = slate.At(0, 2) // authors.country
	case "decade":
		compute = []slate.Scalar{slate.Mul(
			slate.Div(slate.Ref(slate.At(books, 3)), slate.Lit(slate.Int(10))),
			slate.Lit(slate.Int(10)),
		)}
		key = slate.JoinComputed(0)
	// Everything below is a different *kind* of scalar, not a different
	// column. Arithmetic was the only expression the three SDKs were ever
	// compared on, and division is the one operation every language spells
	// identically — so agreement on it proved much less than it looked.
	case "discounted":
		// Money, and the one arithmetic rule that is not arithmetic: a decimal
		// literal has no scale of its own and takes the column's, so
		// `Units(50)` beside a scale-2 price is fifty *cents*. A client that
		// sent `Int(50)` instead would be refused by the server — a decimal
		// beside a plain number has no unit — which is the difference this
		// case exists to catch, in three languages that each have their own
		// idea of what an integer literal is.
		compute = []slate.Scalar{slate.Sub(
			slate.Ref(slate.At(books, 7)),
			slate.Lit(slate.Units(50)),
		)}
		key = slate.JoinComputed(0)
	case "doubled":
		// The other expressible shape: money times a whole number is still
		// money, at the same scale.
		compute = []slate.Scalar{slate.Mul(
			slate.Ref(slate.At(books, 7)),
			slate.Lit(slate.Int(2)),
		)}
		key = slate.JoinComputed(0)
	case "badPrice":
		// Refused by the *server*, at plan time: a decimal added to a plain
		// integer would be a count of nothing. Sent rather than caught here on
		// purpose — the claim is that all three clients surface the same
		// refusal, which an adapter that validated locally would not test.
		compute = []slate.Scalar{slate.Add(
			slate.Ref(slate.At(books, 7)),
			slate.Ref(slate.At(books, 3)),
		)}
		key = slate.JoinComputed(0)
	case "shout":
		// A string function, on the *left* input. The author's *name* rather
		// than the country, because every country here is already upper case
		// — so an adapter that dropped the `Upper` would have passed.
		compute = []slate.Scalar{slate.Upper(slate.Ref(slate.At(authors, 1)))}
		key = slate.JoinComputed(0)
	case "era":
		// A conditional, whose branches are string literals of a different
		// type from the column they test.
		compute = []slate.Scalar{slate.Case(
			[]slate.CaseBranch{{
				// `Compare` rather than `Lt`, because the condition is over
				// the *joined* row: `Lt` takes a bare ordinal, which on a join
				// would mean the left table's.
				// 1970 rather than 2000, because every book here predates
				// 2000 — so the `otherwise` branch was never taken and the
				// conditional was a constant. Five fall either side of 1970.
				When: slate.Compare(slate.At(books, 3), slate.OpLt, slate.Int(1970)),
				Then: slate.Lit(slate.String("before 1970")),
			}},
			slate.Lit(slate.String("from 1970")),
		)}
		key = slate.JoinComputed(0)
	case "tidy":
		// A regular expression over a lower-cased title, so the pattern
		// dialect and the case folding both have to agree.
		compute = []slate.Scalar{slate.RegexpReplace(
			slate.Lower(slate.Ref(slate.At(books, 2))), "[^a-z]+", "-",
		)}
		key = slate.JoinComputed(0)
	case "releasedYear":
		// A calendar field, over seconds since the epoch. Seven of the eleven
		// books are before 1970, so this runs on negative instants.
		compute = []slate.Scalar{slate.YearOf(slate.Ref(slate.At(books, 5)))}
		key = slate.JoinComputed(0)
	case "releasedMonth":
		// A calendar *truncation*, which is not a division: a month has no
		// fixed number of seconds.
		compute = []slate.Scalar{slate.MonthStartOf(slate.Ref(slate.At(books, 5)))}
		key = slate.JoinComputed(0)
	case "releasedHourNY":
		// A named timezone, resolved through the server's transition table.
		// Some of these dates are in daylight saving and some are not, so this
		// is not a constant shift.
		compute = []slate.Scalar{slate.Extract(
			slate.Hour, slate.InZone("America/New_York", slate.Ref(slate.At(books, 5))),
		)}
		key = slate.JoinComputed(0)
	case "label":
		// Concatenation across *both* inputs, which no input's own compute
		// could express.
		compute = []slate.Scalar{slate.Concat(
			slate.Ref(slate.At(authors, 2)),
			slate.Lit(slate.String("/")),
			slate.Ref(slate.At(books, 2)),
			slate.Lit(slate.String("/")),
			// An i64 spliced into a string, which is where `Concat` was
			// rendering Rust's `Debug` form: `1968` came out as `I64(1968)`.
			slate.Ref(slate.At(books, 3)),
		)}
		key = slate.JoinComputed(0)
	default:
		return none, slate.Grouping{}, fmt.Errorf("no such grouping: %s", spec.GroupBy)
	}

	grouping := slate.Grouping{
		GroupBy:    []slate.Column{key},
		Aggregates: []slate.Aggregate{slate.Count()},
		Limit:      spec.Limit,
	}
	if spec.Having != nil {
		having := slate.GroupGe(slate.Agg(0), slate.Uint(spec.Having.MinCount))
		grouping.Having = &having
	}
	direction := slate.Asc
	if spec.Direction == "desc" {
		direction = slate.Desc
	}
	column := slate.Agg(0)
	if spec.Sort == "key" {
		column = slate.Key(0)
	}
	grouping.Sort = []slate.GroupSortKey{
		{Column: column, Direction: direction},
		// A tie-break on the key, so equal counts do not come back in whatever
		// order the hash produced — which would differ between runs and
		// between adapters.
		{Column: slate.Key(0), Direction: slate.Asc},
	}

	join := b.Query()
	join.Compute = compute
	return join, grouping, nil
}

func (s *server) aggregate(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
	join, grouping, err := buildAggregate(body)
	if err != nil {
		return nil, err
	}
	stream, err := session.AggregateJoin(ctx, join, grouping)
	if err != nil {
		return nil, err
	}
	groups, err := stream.Collect()
	if err != nil {
		return nil, err
	}
	out := make([]map[string]any, 0, len(groups))
	for _, group := range groups {
		entry := map[string]any{"key": encodeRow(group.Key)}
		if len(group.Values) > 0 {
			entry["count"] = encode(group.Values[0])
		}
		out = append(out, entry)
	}
	return map[string]any{"groups": out}, nil
}

// explainAggregate is the plan of the *grouped* read, which is not the plan of
// the join underneath it: grouping narrows each input's projection to the group
// keys and the aggregates' columns. `decodes` is where that shows.
func (s *server) explainAggregate(
	ctx context.Context, session *slate.Session, body json.RawMessage,
) (any, error) {
	join, grouping, err := buildAggregate(body)
	if err != nil {
		return nil, err
	}
	plan, err := session.ExplainAggregateJoin(ctx, join, grouping)
	if err != nil {
		return nil, err
	}
	if plan.Join == nil {
		return nil, fmt.Errorf("a grouped join explained as something other than a join")
	}
	inputs := make([]map[string]any, 0, len(plan.Join.Inputs))
	for _, input := range plan.Join.Inputs {
		inputs = append(inputs, map[string]any{
			"table":     input.Plan.Table,
			"access":    input.Plan.Access,
			"indexOnly": input.Plan.IndexOnly,
			"decodes":   decodesOf(input.Plan.Decodes),
			"algorithm": input.Algorithm,
		})
	}
	return map[string]any{"inputs": inputs, "display": plan.Display}, nil
}

// decodesOf renders a plan's decoded columns as a JSON array of numbers.
//
// `[]uint32(nil)` marshals to `null` and an empty slice to `[]`, and the
// conformance runner compares the text — so a plan that decodes nothing must
// not read as a plan that did not say.
func decodesOf(columns []uint32) []uint32 {
	if columns == nil {
		return []uint32{}
	}
	return columns
}

func (s *server) explain(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
	var spec querySpec
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the query: %w", err)
	}
	query, err := spec.build()
	if err != nil {
		return nil, err
	}
	plan, err := session.Explain(ctx, query)
	if err != nil {
		return nil, err
	}
	// `estimatedCost` is deliberately absent: it is a float the three clients
	// may render differently, and the contract compares text.
	return map[string]any{
		"table":         plan.Table,
		"access":        plan.Access,
		"residual":      plan.Residual,
		"indexOnly":     plan.IndexOnly,
		"sorts":         plan.Sorts,
		"descending":    plan.Descending,
		"estimatedRows": formatFloat(plan.EstimatedRows),
		"display":       plan.Display,
	}, nil
}

// transaction demonstrates the one thing a single request cannot: a write that
// is visible to its own transaction before anyone else can see it.
func (s *server) transaction(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
	var spec struct {
		Commit bool `json:"commit"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the request: %w", err)
	}

	const probe = 9001
	// Start from a clean slate so the demo can be run twice.
	if _, err := session.Delete(ctx, "books", []slate.Value{slate.Uint(probe)}); err != nil {
		var e *slate.Error
		if !isNotFound(err, &e) {
			return nil, err
		}
	}

	tx, err := session.Begin(ctx)
	if err != nil {
		return nil, err
	}
	defer func() { _ = tx.Rollback(ctx) }()

	// Every column, in ordinal order, including the two `books` grew for the
	// conformance corpus. A row on the wire is one value per column, so a
	// five-value row here is a refusal — which is how adding them was caught.
	row := []slate.Value{
		slate.Uint(probe), slate.Uint(1),
		slate.String("A Book In Flight"), slate.Int(2026), slate.Float(5.0),
		slate.Int(1767225600), slate.Vector([]float32{0.4, 0.3, 0.2, 0.1}),
		slate.Units(1000),
	}
	if _, err := tx.Insert(ctx, "books", row); err != nil {
		return nil, err
	}
	_, insideVisible, err := tx.Get(ctx, "books", []slate.Value{slate.Uint(probe)})
	if err != nil {
		return nil, err
	}

	if spec.Commit {
		if err := tx.Commit(ctx); err != nil {
			return nil, err
		}
	} else if err := tx.Rollback(ctx); err != nil {
		return nil, err
	}

	_, afterVisible, err := session.Get(ctx, "books", []slate.Value{slate.Uint(probe)})
	if err != nil {
		return nil, err
	}
	// Leave nothing behind, so a committed run and a rolled-back run start the
	// same way and the demo is idempotent.
	if afterVisible {
		if _, err := session.Delete(ctx, "books", []slate.Value{slate.Uint(probe)}); err != nil {
			return nil, err
		}
	}
	return map[string]any{"visibleInside": insideVisible, "visibleAfter": afterVisible}, nil
}

func isNotFound(err error, into **slate.Error) bool {
	return slate.IsKind(err, slate.KindNotFound)
}

// QUERY_VECTOR is the embedding every `/api/nearest` request measures against.
//
// Fixed rather than taken from the request body, because the point is that
// three SDKs build the same `Distance` scalar and agree on the order it
// produces. A vector from the body would let a caller ask a question the other
// two adapters were not asked, which is the one thing the conformance runner
// cannot tolerate.
var queryVector = []float32{0.1, 0.2, 0.3, 0.4}

// nearest ranks books by cosine distance from `queryVector`.
//
// The last scalar family the three SDKs were never compared on. It is also the
// only one whose *result* cannot be compared: a distance is an f64 and the
// three clients format floats differently, which is why `/api/explain`
// excludes `estimatedCost` for the same reason. So this returns the titles in
// order and not the distances — the order is the claim, and it is a total one
// because the sort breaks ties on the id.
func (s *server) nearest(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
	var spec struct {
		Limit *uint64 `json:"limit"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the search: %w", err)
	}
	query := slate.Query{
		Table: "books",
		// The distance, computed per row and then sorted on. A vector index
		// would change the plan and not the answer; there is none here, so
		// this is an exhaustive scan and says so under `/api/explain`.
		Compute: []slate.Scalar{slate.Distance(
			slate.Col(6), slate.Lit(slate.Vector(queryVector)), slate.Cosine,
		)},
		Columns: []slate.Ordinal{0, 2},
		Sort: []slate.SortKey{
			{Ref: ref(slate.Computed0(0))},
			// A tie-break on the primary key, so two books at the same
			// distance do not come back in whatever order the scan produced.
			// A bare ordinal here, because on a single table that is all a
			// sort key needs — `Ref` exists for the computed slot above.
			{Column: 0},
		},
	}
	if spec.Limit != nil {
		query.Limit = spec.Limit
	}
	stream, err := session.Query(ctx, query)
	if err != nil {
		return nil, err
	}
	rows, err := stream.Collect()
	if err != nil {
		return nil, err
	}
	titles := make([]tagged, 0, len(rows))
	for _, row := range rows {
		titles = append(titles, encode(row[2]))
	}
	return map[string]any{"titles": titles}, nil
}

// ref is `&c` for a Column literal, which Go will not take the address of
// inline.
func ref(c slate.Column) *slate.Column { return &c }

// related loads one relationship for many parents, in one read.
//
// The relationship is `sales.book_id -> books`, the only foreign key in the
// demo's schema: `books.author_id` cannot be one, because `Author Unknown`
// names author 99 on purpose so the outer joins have an unmatched side.
//
// Reading it the `parents` way goes through `books`, which carries the row
// policy — so a `reader` asking for the books behind a page of sales must see
// the same gap in all three SDKs, and a client that resolved the relationship
// itself rather than asking the server would not have that gap at all.
func (s *server) related(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
	var spec struct {
		Way  string            `json:"way"`
		Keys []json.RawMessage `json:"keys"`
		// Overridable only so the conformance corpus can name a key that does
		// not exist and compare the three refusals, which is the one thing
		// about this call the three could spell differently.
		Through string `json:"through"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the relation: %w", err)
	}

	// The key, from the generated declaration rather than from three string
	// literals here. `Answers` is the reason: the table a read decodes as is
	// `sales` one way and `books` the other, and it used to be spelled out
	// twice in this function and once more in `path` below. Making `Answers`
	// return the child either way turns four conformance cases red with a
	// schema-check refusal — the server catches it, so this is a convenience
	// rather than a fix for a silent bug. See the type's own comment.
	key := schema.SalesForeignKeys["sale_book"]
	way := slate.Children
	if spec.Way == "parents" {
		way = slate.Parents
	}
	relation := key.Children()
	if way == slate.Parents {
		relation = key.Parents()
	}
	table := key.Answers(way)
	// The conformance corpus names a key that does not exist, so that the
	// three refusals can be compared. That is the one thing about this call
	// the three could spell differently, and it is why the override survives
	// the generated key above.
	if spec.Through != "" {
		relation.Through = spec.Through
	}

	keys := make([][]slate.Value, 0, len(spec.Keys))
	for _, raw := range spec.Keys {
		value, err := decode(raw)
		if err != nil {
			return nil, err
		}
		keys = append(keys, []slate.Value{value})
	}

	groups, err := session.Related(ctx, table, relation, keys...)
	if err != nil {
		return nil, err
	}

	// A group per key the caller sent, in the caller's order, including the
	// empty ones — which is the shape all three clients promise and the one
	// worth comparing.
	out := make([][][]tagged, 0, len(groups))
	for _, group := range groups {
		rows := make([][]tagged, 0, len(group))
		for _, row := range group {
			rows = append(rows, encodeRow(row))
		}
		out = append(out, rows)
	}
	return map[string]any{"groups": out}, nil
}

// chain reads three tables in one request: authors, their books, and the sales
// of those books.
//
// Separate from `/api/join` because it is the thing worth comparing and not a
// variation on a two-table join. A chain is not a different RPC — `JoinQuery`
// carries `repeated JoinInput` and the kernel picks its chain path when there
// are more than two — so what could differ between the three SDKs is how each
// spells the *third* input's attachment: it joins back to the second, not to
// the first, and a client that got that wrong would produce a cross join with
// the right number of columns.
func (s *server) chain(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
	var spec struct {
		Type  string  `json:"type"`
		Limit *uint64 `json:"limit"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the chain: %w", err)
	}
	kinds := map[string]slate.JoinType{
		"inner": slate.Inner, "left": slate.Left,
		"right": slate.Right, "full": slate.Full,
	}
	kind, ok := kinds[spec.Type]
	if !ok {
		return nil, fmt.Errorf("no such join type: %s", spec.Type)
	}

	b := slate.NewJoin()
	authors := b.Add(slate.JoinInput{Table: "authors"})
	books := b.Add(slate.JoinInput{
		Table: "books",
		Type:  kind,
		On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
	})
	// books.id to sales.book_id: `Earlier` names the *second* input, which is
	// what makes this a chain rather than two joins onto the first.
	b.Add(slate.JoinInput{
		Table: "sales",
		Type:  kind,
		On:    []slate.On{{Earlier: slate.At(books, 0), Own: 1}},
	})
	query := b.Query()
	query.Limit = spec.Limit

	stream, err := session.Join(ctx, query)
	if err != nil {
		return nil, err
	}
	rows, err := stream.Collect()
	if err != nil {
		return nil, err
	}

	out := make([]map[string]any, 0, len(rows))
	for _, row := range rows {
		entry := map[string]any{"authors": nil, "books": nil, "sales": nil}
		for at, name := range []string{"authors", "books", "sales"} {
			if at < len(row) && row[at] != nil {
				entry[name] = encodeRow(row[at])
			}
		}
		out = append(out, entry)
	}
	sortJoined(out)
	return map[string]any{"rows": out}, nil
}

// page reads one page of `books` by keyset, and reports where to resume.
//
// The cursor comes back as a row so the three adapters encode it the same way
// they encode everything else, and so the corpus compares its *type* as well
// as its value — a cursor that arrived as a bare number would agree across
// three clients that had all lost the same distinction.
func (s *server) page(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
	var spec struct {
		Limit  uint64            `json:"limit"`
		After  []json.RawMessage `json:"after"`
		Sort   []sortSpec        `json:"sort"`
		Column []int             `json:"columns"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the page: %w", err)
	}

	query := slate.Query{Table: "books"}
	if spec.Limit > 0 {
		query.Limit = slate.Limit(spec.Limit)
	}
	for _, raw := range spec.After {
		value, err := decode(raw)
		if err != nil {
			return nil, err
		}
		query.After = append(query.After, value)
	}
	for _, c := range spec.Column {
		query.Columns = append(query.Columns, slate.Ordinal(c))
	}
	for _, key := range spec.Sort {
		direction := slate.Asc
		if key.Direction == "desc" {
			direction = slate.Desc
		}
		query.Sort = append(query.Sort, slate.SortKey{
			Column: key.Column, Direction: direction,
		})
	}

	page, err := session.Page(ctx, query)
	if err != nil {
		return nil, err
	}

	rows := make([][]tagged, 0, len(page.Rows))
	for _, row := range page.Rows {
		rows = append(rows, encodeRow(row))
	}
	// `nil` rather than an empty list for the last page, so "there is nothing
	// after this" is one value in all three adapters rather than two.
	var cursor []tagged
	if !page.IsLast() {
		cursor = encodeRow(page.Cursor)
	}
	return map[string]any{"rows": rows, "cursor": cursor, "isLast": page.IsLast()}, nil
}

// The id range this handler owns, clear of the fixture and of the
// transaction probe at 9001.
//
// Predicate writes mutate, and the conformance runner drives all three
// adapters against one database, so each run seeds its own rows first and the
// three see the same four rows. The fixture is never touched: a case that
// deleted from it would make every later case depend on which SDK ran first.
const predicateFirst = 9100

// predicateWrite seeds four rows, writes over them by predicate, and reports
// what came back.
//
// Self-contained and idempotent, like the transaction probe above and for the
// same reason: the demo, and the corpus, must give the same answer run twice.
func (s *server) predicateWrite(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
	var spec struct {
		Kind      string `json:"kind"`
		Returning bool   `json:"returning"`
		NoSet     bool   `json:"noSet"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the request: %w", err)
	}

	// Clean slate. A predicate delete is the tidiest way to say "whatever is
	// left from last time", and it exercises the feature on the way in.
	if _, err := session.DeleteWhere(ctx, slate.DeleteWhere{
		Table:  "books",
		Filter: slate.Filter(slate.Ge(0, slate.Uint(predicateFirst))),
	}); err != nil {
		return nil, err
	}
	rows := make([][]slate.Value, 0, 4)
	for n := uint64(0); n < 4; n++ {
		rows = append(rows, []slate.Value{
			slate.Uint(predicateFirst + n), slate.Uint(1),
			slate.String(fmt.Sprintf("Predicate %d", n)),
			slate.Int(int64(2000 + n)), slate.Float(3.0),
			slate.Int(1767225600), slate.Vector([]float32{0.1, 0.2, 0.3, 0.4}),
			slate.Units(1000),
		})
	}
	if _, err := session.Insert(ctx, "books", rows...); err != nil {
		return nil, err
	}

	// Rows 9102 and 9103: year >= 2002.
	recent := slate.Filter(slate.And(
		slate.Ge(0, slate.Uint(predicateFirst)),
		slate.Ge(3, slate.Int(2002)),
	))

	var result slate.WriteResult
	var err error
	switch spec.Kind {
	case "delete":
		result, err = session.DeleteWhere(ctx, slate.DeleteWhere{
			Table: "books", Filter: recent, Returning: spec.Returning,
		})
	case "update":
		set := []slate.Assignment{
			// rating = rating + 1, read off the row as it was.
			slate.Assign(4, slate.Add(slate.Col(4), slate.Lit(slate.Float(1.0)))),
		}
		if spec.NoSet {
			set = nil
		}
		result, err = session.UpdateWhere(ctx, slate.UpdateWhere{
			Table: "books", Filter: recent, Set: set, Returning: spec.Returning,
		})
	default:
		return nil, fmt.Errorf("unknown predicate write %q", spec.Kind)
	}
	if err != nil {
		return nil, err
	}

	returned := make([][]tagged, 0, len(result.Rows))
	for _, row := range result.Rows {
		returned = append(returned, encodeRow(row))
	}
	// How many of the four are left, which is what makes a delete's effect
	// visible rather than only its report.
	stream, err := session.Query(ctx, slate.Query{
		Table:  "books",
		Filter: slate.Filter(slate.Ge(0, slate.Uint(predicateFirst))),
	})
	if err != nil {
		return nil, err
	}
	defer stream.Close()
	left := 0
	for stream.Next() {
		stream.Row()
		left++
	}
	if err := stream.Err(); err != nil {
		return nil, err
	}
	return map[string]any{
		"affected": result.Affected,
		"rows":     returned,
		"left":     left,
	}, nil
}

// The id the conditional-update handler owns, clear of every other range.
const conditionalID = 9300

// conditionalUpdate shows optimistic concurrency, and the decimal column it
// exists to protect.
//
// Seeds one book at 9300 priced 10.00, reads it back, optionally lets somebody
// else move the price, then tries a conditional update to 12.50 guarded by the
// row as it was read. With `stale: false` it lands; with `stale: true` the
// server refuses it and the price is whatever the other writer left.
//
// It is one endpoint rather than two because the *pair* is the point: an
// unconditional update and a conditional one over an unchanged row do exactly
// the same thing, so only the stale case tells them apart.
func (s *server) conditionalUpdate(
	ctx context.Context, session *slate.Session, body json.RawMessage,
) (any, error) {
	var spec struct {
		Stale bool `json:"stale"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the request: %w", err)
	}

	key := []slate.Value{slate.Uint(conditionalID)}
	seeded := book(conditionalID, "Priced")
	if _, err := session.Upsert(ctx, "books", seeded); err != nil {
		return nil, err
	}

	// Read the row back rather than reusing what was written: a conditional
	// update guards against what is *stored*, and a caller that guards with
	// its own draft is testing its memory rather than the database.
	was, found, err := session.Get(ctx, "books", key)
	if err != nil {
		return nil, err
	}
	if !found {
		return nil, fmt.Errorf("the seeded row is not there")
	}

	if spec.Stale {
		moved := append([]slate.Value(nil), was...)
		moved[7] = slate.Units(1100)
		if _, err := session.Update(ctx, "books", moved); err != nil {
			return nil, err
		}
	}

	next := append([]slate.Value(nil), was...)
	next[7] = slate.Units(1250)
	refused := ""
	if _, err := session.UpdateIfUnchanged(ctx, "books", slate.RowUpdate{
		Row: next, Was: was,
	}); err != nil {
		var e *slate.Error
		if !errors.As(err, &e) {
			return nil, err
		}
		// A field rather than an adapter error, so the corpus compares the two
		// cases as ordinary answers instead of one being a refusal case.
		refused = kindName(e.Kind)
	}

	after, _, err := session.Get(ctx, "books", key)
	if err != nil {
		return nil, err
	}
	price, ok := after[7].(slate.Units)
	if !ok {
		return nil, fmt.Errorf("a decimal came back as %T", after[7])
	}
	return map[string]any{
		"refused": refused,
		"price":   encode(price),
		// The rendering, against the scale this adapter declares. The tagged
		// value above is the units and says nothing about a scale, so this is
		// the only place the three clients' renderers are compared.
		"rendered": price.StringWithScale(2),
	}, nil
}

// The id the conditional-delete handler owns.
const conditionalDeleteID = 9301

// conditionalDelete shows the delete half of optimistic concurrency, and the
// one place it is not simply the update's twin.
//
// `stale` lets somebody else edit the row first; `gone` removes it first.
// Applied, refused-because-moved and refused-because-absent are three
// different answers, and the third is the interesting one: a *plain* delete
// reports an absent key as `affected: 0`, and a conditional one refuses it.
func (s *server) conditionalDelete(
	ctx context.Context, session *slate.Session, body json.RawMessage,
) (any, error) {
	var spec struct {
		Stale bool `json:"stale"`
		Gone  bool `json:"gone"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the request: %w", err)
	}

	key := []slate.Value{slate.Uint(conditionalDeleteID)}
	if _, err := session.Upsert(ctx, "books", book(conditionalDeleteID, "Doomed")); err != nil {
		return nil, err
	}
	was, found, err := session.Get(ctx, "books", key)
	if err != nil {
		return nil, err
	}
	if !found {
		return nil, fmt.Errorf("the seeded row is not there")
	}

	if spec.Stale {
		moved := append([]slate.Value(nil), was...)
		moved[7] = slate.Units(1100)
		if _, err := session.Update(ctx, "books", moved); err != nil {
			return nil, err
		}
	}
	if spec.Gone {
		if _, err := session.Delete(ctx, "books", key); err != nil {
			return nil, err
		}
	}

	refused := ""
	affected := uint64(0)
	result, err := session.DeleteIfUnchanged(ctx, "books", slate.RowDelete{Key: key, Was: was})
	if err != nil {
		var e *slate.Error
		if !errors.As(err, &e) {
			return nil, err
		}
		refused = kindName(e.Kind)
	} else {
		affected = result.Affected
	}

	_, present, err := session.Get(ctx, "books", key)
	if err != nil {
		return nil, err
	}
	// `left` is what the table says, beside `affected` and `refused`, which
	// are what the server said it did. A refusal that removed the row anyway
	// would agree across three clients on a claim none of them checked.
	return map[string]any{"refused": refused, "affected": affected, "left": present}, nil
}

// The id range the batch handler owns, clear of the fixture, of the
// transaction probe at 9001 and of the predicate-write range at 9100.
const batchFirst = 9200

// batchWrite seeds nothing and writes three books, one of which is a
// duplicate, under whichever atomicity was asked for.
//
// The duplicate is the point: it is the operation that makes the two
// guarantees visibly different, and `left` afterwards is how the corpus sees
// which one happened.
func (s *server) batchWrite(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
	var spec struct {
		Atomicity string `json:"atomicity"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the request: %w", err)
	}

	// Clean slate, so the demo and the corpus give the same answer run twice.
	if _, err := session.DeleteWhere(ctx, slate.DeleteWhere{
		Table:  "books",
		Filter: slate.Filter(slate.Ge(0, slate.Uint(batchFirst))),
	}); err != nil {
		return nil, err
	}
	// The row the batch will collide with.
	if _, err := session.Insert(ctx, "books", book(batchFirst+1, "Already There")); err != nil {
		return nil, err
	}

	atomicity := slate.Independent
	if spec.Atomicity == "all-or-nothing" {
		atomicity = slate.AllOrNothing
	}
	b := slate.NewBatch(atomicity)
	b.Insert("books", book(batchFirst, "First"))
	b.Insert("books", book(batchFirst+1, "Collides"))
	b.Insert("books", book(batchFirst+2, "Third"))

	result, err := session.Batch(ctx, b)
	failed := ""
	if err != nil {
		// An atomic batch fails the call. Reported as a field rather than as
		// an adapter error, so the corpus compares the *outcome* of the two
		// atomicities rather than one being a refusal case and one not.
		var e *slate.Error
		if errors.As(err, &e) {
			failed = kindName(e.Kind)
		} else {
			return nil, err
		}
	}

	outcomes := make([]any, 0, len(result.Outcomes))
	for _, one := range result.Outcomes {
		if one.OK() {
			outcomes = append(outcomes, map[string]any{"ok": one.Written.Affected})
			continue
		}
		var e *slate.Error
		if !errors.As(one.Err, &e) {
			return nil, one.Err
		}
		outcomes = append(outcomes, map[string]any{
			"kind": kindName(e.Kind), "reason": e.Reason,
		})
	}

	stream, err := session.Query(ctx, slate.Query{
		Table:  "books",
		Filter: slate.Filter(slate.Ge(0, slate.Uint(batchFirst))),
	})
	if err != nil {
		return nil, err
	}
	defer stream.Close()
	left := 0
	for stream.Next() {
		stream.Row()
		left++
	}
	if err := stream.Err(); err != nil {
		return nil, err
	}
	return map[string]any{"failed": failed, "outcomes": outcomes, "left": left}, nil
}

// book is a whole `books` row, every column in ordinal order.
func book(id uint64, title string) []slate.Value {
	return []slate.Value{
		slate.Uint(id), slate.Uint(1), slate.String(title),
		slate.Int(2020), slate.Float(4.0),
		slate.Int(1767225600), slate.Vector([]float32{0.5, 0.5, 0.5, 0.5}),
		// 10.00, at the column's declared scale of 2.
		slate.Units(1000),
	}
}

// path resolves a relationship path level by level in one request.
//
// `sales -> books -> editions`: up to the book a sale sold, then down to that
// book's editions. Two steps in opposite directions, which is the case worth
// comparing across three SDKs — a path that only ever went one way would agree
// even with the two directions confused.
//
// The keys are `book_id` values read off sale rows, because that is where a
// path starts: at the column of the caller's own rows that relates them to the
// first step.
//
// Both shapes in one answer. `trees` keeps the middle level, which is
// `load_nested`; `through` drops it, which is `load_related_through`. The
// difference between them is one line in each SDK — "the rows at the bottom"
// rather than "the rows with nothing below them" — and that line is worth
// pinning in all three.
func (s *server) path(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
	var spec struct {
		Keys []json.RawMessage `json:"keys"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the path request: %w", err)
	}

	keys := make([][]slate.Value, 0, len(spec.Keys))
	for _, raw := range spec.Keys {
		value, err := decode(raw)
		if err != nil {
			return nil, err
		}
		keys = append(keys, []slate.Value{value})
	}

	// Both steps from the generated declaration, so the `Table` beside each
	// relation is the catalog's answer rather than this file's memory of it.
	// The two steps go in opposite directions — up to the book a sale sold,
	// then down to that book's editions — which is exactly the case where
	// `Answers` earns its keep.
	up := schema.SalesForeignKeys["sale_book"]
	down := schema.EditionsForeignKeys["edition_book"]
	steps := []slate.Step{
		{Relation: up.Parents(), Table: up.Answers(slate.Parents)},
		{Relation: down.Children(), Table: down.Answers(slate.Children)},
	}

	trees, err := session.RelatedPath(ctx, steps, keys...)
	if err != nil {
		return nil, err
	}
	through, err := session.RelatedThrough(ctx, steps, keys...)
	if err != nil {
		return nil, err
	}

	outTrees := make([][]map[string]any, 0, len(trees))
	for _, tree := range trees {
		nodes := make([]map[string]any, 0, len(tree))
		for _, n := range tree {
			related := make([][]tagged, 0, len(n.Related))
			for _, leaf := range n.Related {
				related = append(related, encodeRow(leaf.Row))
			}
			nodes = append(nodes, map[string]any{
				"row":     encodeRow(n.Row),
				"related": related,
			})
		}
		outTrees = append(outTrees, nodes)
	}

	outThrough := make([][][]tagged, 0, len(through))
	for _, rows := range through {
		encoded := make([][]tagged, 0, len(rows))
		for _, row := range rows {
			encoded = append(encoded, encodeRow(row))
		}
		outThrough = append(outThrough, encoded)
	}

	return map[string]any{"trees": outTrees, "through": outThrough}, nil
}

// purgeIDs are the shipments the purge handler owns.
//
// Its own range, and re-seeded on every call, because all three adapters run
// this case against one database in turn: the first purge erases the rows, and
// the second and third would find nothing and disagree. The upsert puts them
// back, which is the same trick conditionalDelete uses.
var purgeIDs = []uint64{9401, 9402, 9403}

// purge seeds three shipments, retires two, and erases what was retired.
//
// The count is the point: a purge answers with how many rows it erased and
// never with the rows, which no longer exist to be returned.
func (s *server) purge(
	ctx context.Context, session *slate.Session, _ json.RawMessage,
) (any, error) {
	// A purge is **table-wide** — it takes an instant, not a predicate — so it
	// also erases the row the demo seeder retired. Left alone that made this
	// case depend on which adapter ran first: the first purged three rows and
	// the other two purged two, and all three were right.
	//
	// So: clear the table of retired rows first, run the experiment against a
	// known state, and put the seeder's row back at the end. The handler
	// leaves the table as it found it, which is what keeps the corpus free of
	// an ordering rule nobody would think to preserve.
	if _, err := session.PurgeDeleted(ctx, "shipments", time.Now().Unix()+3600, 0); err != nil {
		return nil, err
	}

	rows := make([][]slate.Value, 0, len(purgeIDs))
	for _, id := range purgeIDs {
		rows = append(rows, []slate.Value{
			slate.Uint(id), slate.Uint(10), slate.String("pending"), slate.Null{},
		})
	}
	if _, err := session.Upsert(ctx, "shipments", rows...); err != nil {
		return nil, err
	}
	// Retire two. A delete rather than a write of `deleted_at`, because the
	// stamp is the server's clock and this is the only path that sets it.
	retire := [][]slate.Value{{slate.Uint(purgeIDs[0])}, {slate.Uint(purgeIDs[1])}}
	if _, err := session.Delete(ctx, "shipments", retire...); err != nil {
		return nil, err
	}

	// The bound is comfortably after the retirement above. The clock is the
	// server's and this is the client's, so a bound of "now" would be a race
	// on a slow machine.
	before := time.Now().Unix() + 3600
	purged, err := session.PurgeDeleted(ctx, "shipments", before, 0)
	if err != nil {
		return nil, err
	}

	// What is left, retired rows included, so the answer distinguishes
	// "erased" from "still there but hidden".
	lower := slate.Uint(purgeIDs[0])
	stream, err := session.Query(ctx, slate.Query{
		Table:          "shipments",
		Filter:         slate.Filter(slate.Ge(0, lower)),
		Sort:           []slate.SortKey{{Column: 0, Direction: slate.Asc}},
		IncludeDeleted: true,
	})
	if err != nil {
		return nil, err
	}
	left, err := stream.Collect()
	if err != nil {
		return nil, err
	}
	ids := make([]uint64, 0, len(left))
	for _, row := range left {
		id, ok := row[0].(slate.Uint)
		if !ok {
			return nil, fmt.Errorf("id is %T", row[0])
		}
		ids = append(ids, uint64(id))
	}
	// Put the seeder's retired shipment back, so the next adapter to run this
	// case — and any case added later that expects it — finds the database as
	// the seeder left it. Written live and then deleted, because a row cannot
	// be created already retired.
	restored := []slate.Value{
		slate.Uint(603), slate.Uint(13), slate.String("pending"), slate.Null{},
	}
	if _, err := session.Upsert(ctx, "shipments", restored); err != nil {
		return nil, err
	}
	if _, err := session.Delete(ctx, "shipments", []slate.Value{slate.Uint(603)}); err != nil {
		return nil, err
	}
	return map[string]any{"purged": purged.Affected, "left": ids}, nil
}

// badStatus writes a shipment whose status no CHECK admits, and lets it fail.
//
// Three checks would be a better fixture than one, and `shipments` declares
// only `status_known`, so this reaches the single-failure shape. The
// three-failure shape is covered by each client's unit tests against the
// captured blob; what this adds is a *live* server, which those cannot have.
//
// The row is never written, so there is nothing to clean up — which is the one
// convenience a refusal case has over the purge above.
func (s *server) badStatus(
	ctx context.Context, session *slate.Session, _ json.RawMessage,
) (any, error) {
	_, err := session.Upsert(ctx, "shipments", []slate.Value{
		slate.Uint(9499), slate.Uint(10), slate.String("teleported"), slate.Null{},
	})
	if err != nil {
		return nil, err
	}
	// Reached only if the server stopped enforcing the check, which is a
	// disagreement worth failing loudly on rather than reporting as an answer.
	return nil, fmt.Errorf("the server accepted a status no CHECK admits")
}

// typed reads two rows and decodes them with the *generated* decoders.
//
// The gap this closes, recorded when the decoders were first executed: every
// test of them builds `[]slate.Value` by hand, so all three suites agree with
// their own idea of what the server sends. A value arriving as `Int` where the
// schema says `Uint` would pass every one of them and fail here — which is the
// only failure the decoders exist to catch that a hand-built row cannot show.
//
// It is also the first thing that *calls* a generated decoder outside a test.
// They were generated, compiled, vetted and run against fixtures, and no code
// path used one; a decoder nothing calls is a decoder whose contract with the
// server is a hypothesis.
//
// `books` 10 covers u64, str, i64, decimal and vector; `shipments` 600 covers
// the nullable column and the enumerated one. `rating` is left out on purpose:
// a float's spelling is the one thing three languages will not agree on
// without a shared formatter, the corpus pins it elsewhere, and this case is
// about *decoding* rather than about rendering.
func (s *server) typed(ctx context.Context, session *slate.Session, _ json.RawMessage) (any, error) {
	bookRow, found, err := session.Get(ctx, "books", []slate.Value{slate.Uint(10)})
	if err != nil {
		return nil, err
	}
	if !found {
		return nil, fmt.Errorf("the seeded book is not there")
	}
	book, err := schema.ScanBooks(bookRow)
	if err != nil {
		return nil, fmt.Errorf("decoding books: %w", err)
	}

	shipmentRow, found, err := session.Get(ctx, "shipments", []slate.Value{slate.Uint(600)})
	if err != nil {
		return nil, err
	}
	if !found {
		return nil, fmt.Errorf("the seeded shipment is not there")
	}
	shipment, err := schema.ScanShipments(shipmentRow)
	if err != nil {
		return nil, fmt.Errorf("decoding shipments: %w", err)
	}

	// Every integer as a decimal string, because one of the three languages
	// reads them as `bigint` and JSON numbers are doubles. The demo's other
	// handlers spell values the same way for the same reason.
	deleted := "null"
	if shipment.DeletedAt != nil {
		deleted = fmt.Sprintf("%d", *shipment.DeletedAt)
	}
	return map[string]any{
		"book": map[string]any{
			"id":        fmt.Sprintf("%d", book.Id),
			"author_id": fmt.Sprintf("%d", book.AuthorId),
			"title":     book.Title,
			"year":      fmt.Sprintf("%d", book.Year),
			// A decimal is a count of the smallest unit; the scale lives in the
			// schema and the row type does not know it.
			"price":      fmt.Sprintf("%d", int64(book.Price)),
			"dimensions": len(book.Embedding),
		},
		"shipment": map[string]any{
			"id":         fmt.Sprintf("%d", shipment.Id),
			"book_id":    fmt.Sprintf("%d", shipment.BookId),
			"status":     shipment.Status,
			"deleted_at": deleted,
		},
	}, nil
}
