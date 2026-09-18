// Package main is the Go adapter of the explorer demo.
//
// It speaks the HTTP contract in ../../CONTRACT.md against a slate head node,
// using the Go client. Two more adapters do the same through the Python and
// TypeScript clients, and `conformance/` requires all three to answer
// identically — which is the first thing in this repository that compares the
// three clients to each other rather than each to the server.
package main

import (
	"encoding/json"
	"fmt"
	"math"
	"strconv"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// tagged is a value in the contract's JSON form.
//
// Tagged rather than bare, and 64-bit integers as strings rather than numbers,
// for the reason the contract gives: `1` as an i64 and `1` as a u64 are
// different values to this database, and a JSON number above 2^53 does not
// survive `JSON.parse`.
type tagged map[string]any

func encode(v slate.Value) tagged {
	switch value := v.(type) {
	case slate.Null:
		return tagged{"null": true}
	case slate.Bool:
		return tagged{"bool": bool(value)}
	case slate.String:
		return tagged{"str": string(value)}
	case slate.Int:
		return tagged{"i64": strconv.FormatInt(int64(value), 10)}
	case slate.Uint:
		return tagged{"u64": strconv.FormatUint(uint64(value), 10)}
	case slate.Float:
		return tagged{"f64": formatFloat(float64(value))}
	case slate.Units:
		// The units, as a string, exactly like the two integer arms — a
		// decimal is 64 bits and a JSON number would round it above 2^53, and
		// a currency total in cents is where that is reached first.
		//
		// *Not* the rendered "12.50": the scale is the column's and this
		// function has only a value. `/api/conditional-update` renders one,
		// against a scale it is given, which is where the three clients'
		// renderers are compared.
		return tagged{"decimal": strconv.FormatInt(int64(value), 10)}
	case slate.Bytes:
		return tagged{"bytes": fmt.Sprintf("%x", []byte(value))}
	case slate.UUID:
		return tagged{"uuid": value.String()}
	case slate.Vector:
		// Elements as formatted strings, for the reason a float is: the three
		// languages print `0.9` differently and the runner compares text.
		//
		// This arm was missing until `books` grew an embedding, and the
		// conformance runner found it on the first run — Go and Python sent
		// `{"unknown": "slate.Vector"}` and `{"unknown": "Vector"}` while Node
		// sent the real thing. A tag nobody had ever produced is a tag nobody
		// had ever checked.
		out := make([]string, 0, len(value))
		for _, element := range value {
			out = append(out, formatFloat(float64(element)))
		}
		return tagged{"vector": out}
	default:
		return tagged{"unknown": fmt.Sprintf("%T", v)}
	}
}

// formatFloat renders a double the same way in all three adapters.
//
// A string, not a JSON number: Go, Python and JavaScript each have their own
// shortest-round-trip float formatter and they do not always agree on the last
// digit. The conformance runner compares text, so the format has to be pinned
// somewhere, and pinning it here costs one function per adapter.
func formatFloat(f float64) string {
	if math.IsNaN(f) || math.IsInf(f, 0) {
		return "null"
	}
	return strconv.FormatFloat(f, 'f', 6, 64)
}

func encodeRow(row []slate.Value) []tagged {
	if row == nil {
		return nil
	}
	out := make([]tagged, 0, len(row))
	for _, v := range row {
		out = append(out, encode(v))
	}
	return out
}

// decode reads a tagged value from a request body.
func decode(raw json.RawMessage) (slate.Value, error) {
	var t map[string]any
	if err := json.Unmarshal(raw, &t); err != nil {
		return nil, fmt.Errorf("a value must be a tagged object: %w", err)
	}
	for kind, v := range t {
		text, _ := v.(string)
		switch kind {
		case "null":
			return slate.Null{}, nil
		case "bool":
			b, _ := v.(bool)
			return slate.Bool(b), nil
		case "str":
			return slate.String(text), nil
		case "i64":
			n, err := strconv.ParseInt(text, 10, 64)
			if err != nil {
				return nil, fmt.Errorf("i64 %q: %w", text, err)
			}
			return slate.Int(n), nil
		case "u64":
			n, err := strconv.ParseUint(text, 10, 64)
			if err != nil {
				return nil, fmt.Errorf("u64 %q: %w", text, err)
			}
			return slate.Uint(n), nil
		case "f64":
			n, err := strconv.ParseFloat(text, 64)
			if err != nil {
				return nil, fmt.Errorf("f64 %q: %w", text, err)
			}
			return slate.Float(n), nil
		case "decimal":
			n, err := strconv.ParseInt(text, 10, 64)
			if err != nil {
				return nil, fmt.Errorf("decimal %q: %w", text, err)
			}
			return slate.Units(n), nil
		}
	}
	return nil, fmt.Errorf("a value carried no known kind")
}
