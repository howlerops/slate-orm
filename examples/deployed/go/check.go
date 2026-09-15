// Ask the deployed head node the same questions the Python check asks, through
// the Go SDK, and require the same answers.
//
// The oracle stays in Python. `check.py --expect` writes what its fold
// computed and this reads it, rather than decoding the packed trip file again:
// three decoders of one file would be three places to be wrong, and a
// common-mode error in them would agree with itself. What is being checked
// here is the *client* — that a Go caller building the same request over the
// same socket gets the same answer as a Python one.
//
// Which matters because the two build requests differently. The Python client
// declares a `Table` and carries a schema fingerprint; this one names a table
// and an ordinal. A join's computed value is `join.computed(0)` there and
// `slate.JoinComputed(0)` here. Every one of those is a place a client could
// send something subtly different, and until this file the deployed example
// exercised exactly one of them.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"os"
	"sort"
	"time"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// The ordinals `head.toml` declares. Written out rather than fetched, which is
// the Go client's arrangement: it names a table and an ordinal, and the server
// has no way to tell it that `3` is `pickup_time` — which is why the Python
// client's fingerprint check exists and why these constants are worth naming.
const (
	tripID         slate.Ordinal = 0
	tripPickupZone slate.Ordinal = 1
	tripPickupTime slate.Ordinal = 3
	tripFare       slate.Ordinal = 7
	zoneID         slate.Ordinal = 0
	zoneBorough    slate.Ordinal = 1
)

// expected is what `check.py --expect` wrote.
type expected struct {
	Trips       int                  `json:"trips"`
	ByHour      map[string]int       `json:"byHour"`
	ByBorough   map[string]int       `json:"byBorough"`
	JoinedHours map[string][]float64 `json:"joinedHours"`
	Zone132     int                  `json:"zone132"`
	Replicas    []string             `json:"replicas"`
}

var failures []string

func check(name string, ok bool, detail string) {
	if ok {
		fmt.Printf("ok    %s\n", name)
		return
	}
	fmt.Printf("FAIL  %s   %s\n", name, detail)
	failures = append(failures, name)
}

func main() {
	address := flag.String("address", "", "the head node's address")
	expect := flag.String("expect", "", "the JSON `check.py --expect` wrote")
	flag.Parse()
	if *address == "" || *expect == "" {
		fmt.Fprintln(os.Stderr, "usage: check --address HOST:PORT --expect FILE")
		os.Exit(2)
	}

	raw, err := os.ReadFile(*expect)
	if err != nil {
		fmt.Fprintf(os.Stderr, "reading %s: %v\n", *expect, err)
		os.Exit(1)
	}
	var want expected
	if err := json.Unmarshal(raw, &want); err != nil {
		fmt.Fprintf(os.Stderr, "decoding %s: %v\n", *expect, err)
		os.Exit(1)
	}

	client, err := slate.Dial(*address, slate.Identity{
		Principal: "u64:1", Tenant: "u64:1", Roles: []string{"app"},
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "dialing %s: %v\n", *address, err)
		os.Exit(1)
	}
	defer client.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()
	session := client.Session()

	// --- every row is there ------------------------------------------------
	total, served, err := countAll(ctx, session)
	if err != nil {
		fmt.Fprintf(os.Stderr, "counting: %v\n", err)
		os.Exit(1)
	}
	check("every trip is visible to the Go client too",
		total == want.Trips, fmt.Sprintf("%d against %d", total, want.Trips))
	check("and the response names the view that served it",
		served != nil && contains(want.Replicas, served.Replica),
		fmt.Sprintf("%+v", served))

	// --- a computed column, grouped ---------------------------------------
	hours := slate.Query{
		Table:   "trips",
		Compute: []slate.Scalar{slate.Extract(slate.Hour, slate.Col(tripPickupTime))},
	}
	byHour, err := groupCounts(ctx, session, hours, slate.Grouping{
		GroupBy:    []slate.Column{slate.Computed0(0)},
		Aggregates: []slate.Aggregate{slate.Count()},
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "grouping by hour: %v\n", err)
		os.Exit(1)
	}
	check("trips per hour of day agree with the Python fold",
		sameCounts(byHour, want.ByHour), diffCounts(byHour, want.ByHour))

	// --- a join, with the key and an aggregate both on the left -----------
	b := slate.NewJoin()
	trips := b.Add(slate.JoinInput{Table: "trips"})
	b.Add(slate.JoinInput{
		Table: "zones",
		On:    []slate.On{{Earlier: slate.At(trips, tripPickupZone), Own: zoneID}},
	})
	join := b.Query()
	join = slate.JoinQuery{
		Inputs: join.Inputs,
		Compute: []slate.Scalar{
			slate.Extract(slate.Hour, slate.Ref(slate.At(trips, tripPickupTime))),
		},
	}
	stream, err := session.AggregateJoin(ctx, join, slate.Grouping{
		GroupBy: []slate.Column{slate.JoinComputed(0)},
		Aggregates: []slate.Aggregate{
			slate.Count(), slate.AvgOf(slate.At(trips, tripFare)),
		},
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "grouping the join: %v\n", err)
		os.Exit(1)
	}
	groups, err := stream.Collect()
	if err != nil {
		fmt.Fprintf(os.Stderr, "draining the join: %v\n", err)
		os.Exit(1)
	}
	joined := map[string][]float64{}
	for _, group := range groups {
		hour, ok := group.Key[0].(slate.Int)
		if !ok {
			fmt.Fprintf(os.Stderr, "an hour came back as %v\n", group.Key[0])
			os.Exit(1)
		}
		count, okCount := group.Values[0].(slate.Uint)
		average, okAvg := group.Values[1].(slate.Float)
		if !okCount || !okAvg {
			fmt.Fprintf(os.Stderr, "aggregates came back as %v\n", group.Values)
			os.Exit(1)
		}
		joined[fmt.Sprint(int64(hour))] = []float64{float64(count), float64(average)}
	}
	check("the hour and the average fare, both from `trips`, across a join",
		sameJoined(joined, want.JoinedHours), diffJoined(joined, want.JoinedHours))

	// --- a group key on the right side of the join ------------------------
	rb := slate.NewJoin()
	rtrips := rb.Add(slate.JoinInput{Table: "trips"})
	rzones := rb.Add(slate.JoinInput{
		Table: "zones",
		On:    []slate.On{{Earlier: slate.At(rtrips, tripPickupZone), Own: zoneID}},
	})
	boroughStream, err := session.AggregateJoin(ctx, rb.Query(), slate.Grouping{
		GroupBy:    []slate.Column{slate.At(rzones, zoneBorough)},
		Aggregates: []slate.Aggregate{slate.Count()},
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "grouping by borough: %v\n", err)
		os.Exit(1)
	}
	boroughGroups, err := boroughStream.Collect()
	if err != nil {
		fmt.Fprintf(os.Stderr, "draining the borough grouping: %v\n", err)
		os.Exit(1)
	}
	byBorough := map[string]int{}
	for _, group := range boroughGroups {
		name, ok := group.Key[0].(slate.String)
		count, okCount := group.Values[0].(slate.Uint)
		if !ok || !okCount {
			fmt.Fprintf(os.Stderr, "a borough group is %v / %v\n", group.Key[0], group.Values[0])
			os.Exit(1)
		}
		byBorough[string(name)] = int(count)
	}
	check("grouping by the right table's borough agrees with the fold",
		sameCounts(byBorough, want.ByBorough), diffCounts(byBorough, want.ByBorough))

	// --- the index answers without reading a row --------------------------
	covering := slate.Query{
		Table:   "trips",
		Filter:  ptr(slate.Eq(tripPickupZone, slate.Uint(132))),
		Columns: []slate.Ordinal{tripPickupZone},
	}
	plan, err := session.Explain(ctx, covering)
	if err != nil {
		fmt.Fprintf(os.Stderr, "explaining: %v\n", err)
		os.Exit(1)
	}
	check("an index answers the covering query without touching a row",
		plan.IndexOnly && contains([]string{plan.Display}, plan.Display) &&
			hasSubstring(plan.Display, "by_pickup_zone"),
		fmt.Sprintf("%q index_only=%v", plan.Display, plan.IndexOnly))

	counted := slate.Query{
		Table:  "trips",
		Filter: ptr(slate.Eq(tripPickupZone, slate.Uint(132))),
	}
	inZone, _, err := aggregateOne(ctx, session, counted,
		slate.Grouping{Aggregates: []slate.Aggregate{slate.Count()}})
	if err != nil {
		fmt.Fprintf(os.Stderr, "counting the zone: %v\n", err)
		os.Exit(1)
	}
	check("the indexed count is the fold's count",
		inZone == want.Zone132, fmt.Sprintf("%d against %d", inZone, want.Zone132))

	fmt.Println()
	if len(failures) > 0 {
		fmt.Printf("%d failed: %v\n", len(failures), failures)
		os.Exit(1)
	}
	fmt.Println("the Go client agrees with the Python fold, through the same socket")
}

func ptr(e slate.Expr) *slate.Expr { return &e }

func contains(haystack []string, needle string) bool {
	for _, s := range haystack {
		if s == needle {
			return true
		}
	}
	return false
}

func hasSubstring(s, sub string) bool {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return true
		}
	}
	return false
}

func countAll(ctx context.Context, session *slate.Session) (int, *slate.ServedBy, error) {
	return aggregateOne(ctx, session, slate.Query{Table: "trips"},
		slate.Grouping{Aggregates: []slate.Aggregate{slate.Count()}})
}

func aggregateOne(
	ctx context.Context, session *slate.Session,
	query slate.Query, grouping slate.Grouping,
) (int, *slate.ServedBy, error) {
	stream, err := session.Aggregate(ctx, query, grouping)
	if err != nil {
		return 0, nil, err
	}
	groups, err := stream.Collect()
	if err != nil {
		return 0, nil, err
	}
	if len(groups) == 0 {
		return 0, stream.ServedBy(), nil
	}
	count, ok := groups[0].Values[0].(slate.Uint)
	if !ok {
		return 0, nil, fmt.Errorf("a count came back as %v", groups[0].Values[0])
	}
	return int(count), stream.ServedBy(), nil
}

func groupCounts(
	ctx context.Context, session *slate.Session,
	query slate.Query, grouping slate.Grouping,
) (map[string]int, error) {
	stream, err := session.Aggregate(ctx, query, grouping)
	if err != nil {
		return nil, err
	}
	groups, err := stream.Collect()
	if err != nil {
		return nil, err
	}
	out := map[string]int{}
	for _, group := range groups {
		key, ok := group.Key[0].(slate.Int)
		count, okCount := group.Values[0].(slate.Uint)
		if !ok || !okCount {
			return nil, fmt.Errorf("a group is %v / %v", group.Key[0], group.Values[0])
		}
		out[fmt.Sprint(int64(key))] = int(count)
	}
	return out, nil
}

func sameCounts(got, want map[string]int) bool {
	if len(got) != len(want) {
		return false
	}
	for key, value := range want {
		if got[key] != value {
			return false
		}
	}
	return true
}

func diffCounts(got, want map[string]int) string {
	keys := make([]string, 0, len(want))
	for key := range want {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	for _, key := range keys {
		if got[key] != want[key] {
			return fmt.Sprintf("%s: %d against %d", key, got[key], want[key])
		}
	}
	return fmt.Sprintf("%d groups against %d", len(got), len(want))
}

func sameJoined(got, want map[string][]float64) bool {
	if len(got) != len(want) {
		return false
	}
	for key, pair := range want {
		mine, ok := got[key]
		if !ok || len(mine) != 2 || mine[0] != pair[0] {
			return false
		}
		// The average is a float computed two different ways — a streaming
		// mean in the kernel, a sum over a slice in Python — so it is compared
		// with a tolerance rather than for equality.
		if math.Abs(mine[1]-pair[1]) > 1e-6 {
			return false
		}
	}
	return true
}

func diffJoined(got, want map[string][]float64) string {
	keys := make([]string, 0, len(want))
	for key := range want {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	for _, key := range keys {
		mine, ok := got[key]
		if !ok {
			return fmt.Sprintf("%s: missing", key)
		}
		if len(mine) != 2 || mine[0] != want[key][0] ||
			math.Abs(mine[1]-want[key][1]) > 1e-6 {
			return fmt.Sprintf("%s: %v against %v", key, mine, want[key])
		}
	}
	return fmt.Sprintf("%d groups against %d", len(got), len(want))
}
