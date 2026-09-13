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

func (s *server) aggregate(ctx context.Context, session *slate.Session, body json.RawMessage) (any, error) {
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
		return nil, fmt.Errorf("decoding the aggregate: %w", err)
	}

	// Which column of the joined schema to group on. `decade` is not a column
	// at all and is refused rather than faked: the kernel can compute one with
	// a scalar expression, this client cannot yet declare one, and returning a
	// hand-bucketed answer would be the adapter doing the database's job.
	var key slate.Column
	switch spec.GroupBy {
	case "author":
		key = slate.At(0, 1) // authors.name
	case "country":
		key = slate.At(0, 2) // authors.country
	case "decade":
		return nil, fmt.Errorf(
			"grouping by decade needs a computed column, which this demo does not declare")
	default:
		return nil, fmt.Errorf("no such grouping: %s", spec.GroupBy)
	}

	b := slate.NewJoin()
	authors := b.Add(slate.JoinInput{Table: "authors"})
	b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
	})

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

	stream, err := session.AggregateJoin(ctx, b.Query(), grouping)
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

	row := []slate.Value{
		slate.Uint(probe), slate.Uint(1),
		slate.String("A Book In Flight"), slate.Int(2026), slate.Float(5.0),
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
