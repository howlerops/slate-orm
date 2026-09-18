// N single inserts against one batch of N, through the Go SDK.
//
// The shape slate-headbench uses, from a client instead of from the wire: the
// multiplier a client sees is the wire's minus the per-operation work a client
// does and a batch does not save. Reports a median and a range over five runs,
// and does not assert — see ../README.md.
package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"sort"
	"time"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

func main() {
	address := flag.String("address", "", "the head node")
	rows := flag.Int("rows", 100, "rows per arm")
	runs := flag.Int("runs", 5, "runs per arm")
	flag.Parse()

	client, err := slate.Dial(*address, slate.Identity{
		Principal: "u64:1", Tenant: "u64:1", Roles: []string{"app"},
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "connecting: %v\n", err)
		os.Exit(1)
	}
	defer client.Close()
	session := client.Session()
	ctx := context.Background()
	base := uint64(2_000_000)

	singles := make([]float64, 0, *runs)
	batches := make([]float64, 0, *runs)
	for run := 0; run < *runs; run++ {
		// Disjoint key ranges per run and per arm: an insert refuses a taken
		// key, and a second pass over the same keys would time the refusal.
		at := base + uint64(run**rows*2)
		singles = append(singles, timeSingles(ctx, session, at, *rows))
		batches = append(batches, timeBatch(ctx, session, at+uint64(*rows), *rows))
	}
	report("go", *rows, singles, batches)
}

func timeSingles(ctx context.Context, session *slate.Session, base uint64, rows int) float64 {
	start := time.Now()
	for n := 0; n < rows; n++ {
		if _, err := session.Insert(ctx, "bench",
			[]slate.Value{slate.Uint(base + uint64(n)), slate.String("row")}); err != nil {
			fmt.Fprintf(os.Stderr, "insert: %v\n", err)
			os.Exit(1)
		}
	}
	return time.Since(start).Seconds()
}

func timeBatch(ctx context.Context, session *slate.Session, base uint64, rows int) float64 {
	batch := slate.NewBatch(slate.AllOrNothing)
	for n := 0; n < rows; n++ {
		batch.Insert("bench", []slate.Value{slate.Uint(base + uint64(n)), slate.String("row")})
	}
	start := time.Now()
	if _, err := session.Batch(ctx, batch); err != nil {
		fmt.Fprintf(os.Stderr, "batch: %v\n", err)
		os.Exit(1)
	}
	return time.Since(start).Seconds()
}

func report(name string, rows int, singles, batches []float64) {
	perSingle := per(singles, rows)
	perBatch := per(batches, rows)
	fmt.Printf("%s\t%d\t%.1f\t%.1f\t%.1f\t%.1f\t%.1f\t%.1f\t%.1f\n",
		name, rows,
		median(perSingle), perSingle[0], perSingle[len(perSingle)-1],
		median(perBatch), perBatch[0], perBatch[len(perBatch)-1],
		median(perSingle)/median(perBatch))
}

// per returns per-row microseconds, sorted so min, median and max are indices.
func per(seconds []float64, rows int) []float64 {
	out := make([]float64, len(seconds))
	for i, s := range seconds {
		out[i] = s / float64(rows) * 1e6
	}
	sort.Float64s(out)
	return out
}

func median(sorted []float64) float64 {
	n := len(sorted)
	if n%2 == 1 {
		return sorted[n/2]
	}
	return (sorted[n/2-1] + sorted[n/2]) / 2
}
