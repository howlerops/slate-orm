package main

import (
	"context"
	"encoding/json"
	"math"
	"strconv"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// renderedCase is one (units, scale) the three clients must render alike.
type renderedCase struct {
	units int64
	scale int
}

// renderedCases is the shared table. See the Python adapter's `RENDERED` for
// why each line is here; the two are the same list and the conformance runner
// is what holds them to it.
//
// A decimal on the wire is a count of the column's smallest unit and the scale
// never travels, so each client has a renderer of its own and each had an
// edge-case table in its *own* suite. Three tables that agree with three
// authors is not three renderers that agree with each other.
var renderedCases = []renderedCase{
	{0, 0},
	{0, 2},
	{7, 1},
	{5, 4},
	{-1, 2},
	{-75, 2},
	{1250, 0},
	{1250, 1},
	{1250, 2},
	{-1250, 3},
	{math.MaxInt64, 2},
	{math.MinInt64, 2},
}

// renderDecimals runs every client's decimal renderer over one shared table.
//
// No server anywhere: three pure functions compared to each other, which is
// the one thing in this contract that needs no database — and the reason it is
// here rather than in three suites is that three suites cannot disagree with
// each other.
func (s *server) renderDecimals(
	_ context.Context, _ *slate.Session, _ json.RawMessage,
) (any, error) {
	out := make([]map[string]any, 0, len(renderedCases))
	for _, one := range renderedCases {
		out = append(out, map[string]any{
			// `units` as a string, by the contract's 64-bit rule: two of these
			// do not survive a JSON number, and they are the two the table
			// exists for.
			"units": strconv.FormatInt(one.units, 10),
			"scale": one.scale,
			"text":  slate.Units(one.units).StringWithScale(one.scale),
		})
	}
	return map[string]any{"rendered": out}, nil
}
