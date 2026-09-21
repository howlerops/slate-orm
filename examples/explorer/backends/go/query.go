package main

import (
	"encoding/json"
	"fmt"
	"sort"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// filterSpec is the contract's filter tree.
//
// A closed grammar rather than a string the adapter parses: three adapters
// parsing the same little language is three parsers to keep in agreement, and
// the first divergence would look like a database bug.
type filterSpec struct {
	Op      string            `json:"op"`
	Column  slate.Ordinal     `json:"column"`
	Value   json.RawMessage   `json:"value"`
	Values  []json.RawMessage `json:"values"`
	Pattern string            `json:"pattern"`
	Parts   []filterSpec      `json:"parts"`
	Part    *filterSpec       `json:"part"`
}

func (f *filterSpec) build() (slate.Expr, error) {
	if f == nil {
		return slate.True(), nil
	}
	simple := map[string]func(slate.Ordinal, slate.Value) slate.Expr{
		"eq": slate.Eq, "ne": slate.Ne,
		"lt": slate.Lt, "le": slate.Le,
		"gt": slate.Gt, "ge": slate.Ge,
	}
	if make, ok := simple[f.Op]; ok {
		value, err := decode(f.Value)
		if err != nil {
			return slate.Expr{}, err
		}
		return make(f.Column, value), nil
	}

	switch f.Op {
	case "like":
		return slate.Like(f.Column, f.Pattern), nil
	case "ilike":
		return slate.ILike(f.Column, f.Pattern), nil
	case "isNull":
		return slate.IsNull(f.Column), nil
	case "isNotNull":
		return slate.IsNotNull(f.Column), nil
	case "in":
		values := make([]slate.Value, 0, len(f.Values))
		for _, raw := range f.Values {
			value, err := decode(raw)
			if err != nil {
				return slate.Expr{}, err
			}
			values = append(values, value)
		}
		return slate.In(f.Column, values...), nil
	case "and", "or":
		parts := make([]slate.Expr, 0, len(f.Parts))
		for i := range f.Parts {
			part, err := f.Parts[i].build()
			if err != nil {
				return slate.Expr{}, err
			}
			parts = append(parts, part)
		}
		if f.Op == "and" {
			return slate.And(parts...), nil
		}
		return slate.Or(parts...), nil
	case "not":
		inner, err := f.Part.build()
		if err != nil {
			return slate.Expr{}, err
		}
		return slate.Not(inner), nil
	default:
		return slate.Expr{}, fmt.Errorf("no such filter operator: %s", f.Op)
	}
}

type sortSpec struct {
	Column    slate.Ordinal `json:"column"`
	Direction string        `json:"direction"`
}

type querySpec struct {
	Table   string          `json:"table"`
	Filter  *filterSpec     `json:"filter"`
	Sort    []sortSpec      `json:"sort"`
	Limit   *uint64         `json:"limit"`
	Offset  uint64          `json:"offset"`
	Columns []slate.Ordinal `json:"columns"`
	// IncludeDeleted asks for rows a soft delete has retired, which needs the
	// `read_deleted` grant the demo gives `app` and withholds from `reader`.
	IncludeDeleted bool `json:"includeDeleted"`
}

// tables the demo serves.
//
// This client needs no catalog, so it could pass an unknown name straight to
// the server and let it answer `not-found`. It does not, because the other two
// clients hold a schema and *cannot* build a request without one — so the
// server never sees their bad name. One of the three refusing differently is a
// contract divergence, and the conformance runner found exactly that.
var known = map[string]bool{
	"authors": true, "books": true, "sales": true, "shipments": true,
}

// views the demo serves, held apart from `known` rather than added to it.
//
// A view is not a table and `/api/meta` must not say it is: the other two
// adapters hold a `Table` per name and a generator reading the catalog emits a
// row type per table, so a view listed among them would be described as
// something it has no id, no index and no write path to be.
//
// Only `/api/query` accepts one. Every other endpoint here names its tables
// itself, which matches the server: `query` is the single handler that reads
// through a view, and the rest answer "`classics` is a view over `books`, and
// only a plain query can read through one".
var views = map[string]bool{"classics": true}

// viewNames is `views` sorted, for `/api/meta`, for the reason knownTables is.
func viewNames() []string {
	out := make([]string, 0, len(views))
	for name := range views {
		out = append(out, name)
	}
	sort.Strings(out)
	return out
}

// knownTables is `known` as a sorted slice, for `/api/meta`.
//
// Sorted because a map's iteration order is deliberately random in Go, and
// the conformance runner compares the three adapters' answers as JSON: an
// unsorted list would disagree with itself between two runs of the same
// binary, which is a far more confusing failure than a missing table.
func knownTables() []string {
	out := make([]string, 0, len(known))
	for name := range known {
		out = append(out, name)
	}
	sort.Strings(out)
	return out
}

func (q *querySpec) build() (slate.Query, error) {
	if !known[q.Table] && !views[q.Table] {
		return slate.Query{}, fmt.Errorf("no such table: %s", q.Table)
	}
	out := slate.Query{
		Table: q.Table, Offset: q.Offset, Columns: q.Columns,
		IncludeDeleted: q.IncludeDeleted,
	}
	if q.Filter != nil {
		filter, err := q.Filter.build()
		if err != nil {
			return out, err
		}
		out.Filter = &filter
	}
	if q.Limit != nil {
		out.Limit = q.Limit
	}
	for _, key := range q.Sort {
		direction := slate.Asc
		if key.Direction == "desc" {
			direction = slate.Desc
		}
		out.Sort = append(out.Sort, slate.SortKey{Column: key.Column, Direction: direction})
	}
	return out, nil
}
