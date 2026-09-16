package main

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

func (s *server) meta(ctx context.Context, session *slate.Session, _ json.RawMessage) (any, error) {
	status, err := s.clients["app"].Leadership(ctx)
	if err != nil {
		return nil, err
	}
	return map[string]any{
		"sdk":    "go",
		"leader": status.Leader,
		"tables": []string{"authors", "books", "sales"},
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

	way, table := slate.Children, "sales"
	if spec.Way == "parents" {
		way, table = slate.Parents, "books"
	}
	through := spec.Through
	if through == "" {
		through = "sale_book"
	}

	keys := make([][]slate.Value, 0, len(spec.Keys))
	for _, raw := range spec.Keys {
		value, err := decode(raw)
		if err != nil {
			return nil, err
		}
		keys = append(keys, []slate.Value{value})
	}

	groups, err := session.Related(ctx, table,
		slate.Relation{On: "sales", Through: through, Way: way}, keys...)
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
