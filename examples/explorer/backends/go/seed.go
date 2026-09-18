package main

import (
	"context"
	"time"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// seed writes the demo's data.
//
// In the Go adapter rather than in the head node's configuration because
// `slate-serverd` has no seed section — it serves a schema, it does not
// populate one. Run once by `run.sh` before any adapter starts serving, so
// three adapters do not race to write the same rows.
//
// Upsert rather than insert, so running it twice is not an error and a
// half-finished run can simply be repeated.
func (s *server) seed() error {
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	session := s.clients["app"].Session()

	authors := [][]slate.Value{
		{slate.Uint(1), slate.String("Ursula K. Le Guin"), slate.String("US"), slate.Int(1929)},
		{slate.Uint(2), slate.String("Iain M. Banks"), slate.String("UK"), slate.Int(1954)},
		{slate.Uint(3), slate.String("Octavia E. Butler"), slate.String("US"), slate.Int(1947)},
		{slate.Uint(4), slate.String("Stanisław Lem"), slate.String("PL"), slate.Int(1921)},
		// An author with no books at all, so an inner join and an outer join
		// give visibly different answers in the UI.
		{slate.Uint(5), slate.String("Ann Leckie"), slate.String("US"), slate.Int(1966)},
	}
	if _, err := session.Upsert(ctx, "authors", authors...); err != nil {
		return err
	}

	// `released` is seconds since the epoch — there is no date type — and
	// `embedding` is a four-dimensional vector.
	//
	// Both are here so the conformance runner can compare the three SDKs on a
	// *calendar* expression and on a *distance*, rather than on arithmetic
	// alone. Division is the one scalar every language spells the same way, so
	// "the three clients agree about `year / 10 * 10`" was a much weaker claim
	// than it looked.
	//
	// The dates are deliberately spread: seven of the eleven are before 1970,
	// so `year(released)` runs on *negative* epoch seconds and exercises the
	// floored division a naive `/ 86400` gets wrong; some are in daylight
	// saving and some are not, so `hour(released, 'America/New_York')` is not
	// a constant shift; and `The Player of Games` at 02:10 UTC is the previous
	// day in New York, which is the case a caller who reads a UTC timestamp
	// and calls it the local date gets wrong.
	//
	// `price` is in cents, which is what a decimal column at scale 2 holds:
	// `1250` is 12.50. The prices differ per book rather than being a
	// constant, so a `SUM` over them is a number a reader can check and an
	// adapter that dropped the column would be visible rather than merely
	// suspicious.
	book := func(
		id, author uint64, title string, year int64, rating float64,
		released int64, embedding []float32, price int64,
	) []slate.Value {
		return []slate.Value{
			slate.Uint(id), slate.Uint(author), slate.String(title),
			slate.Int(year), slate.Float(rating),
			slate.Int(released), slate.Vector(embedding), slate.Units(price),
		}
	}
	books := [][]slate.Value{
		book(10, 1, "A Wizard of Earthsea", 1968, 4.4, -36754200, []float32{0.9, 0.1, 0, 0}, 1295),
		book(11, 1, "The Dispossessed", 1974, 4.6, 137840700, []float32{0.1, 0.9, 0, 0}, 1450),
		book(12, 1, "The Left Hand of Darkness", 1969, 4.5, -26370900, []float32{0, 0.1, 0.9, 0}, 1399),
		book(13, 2, "Consider Phlebas", 1987, 4.1, 545570400, []float32{0, 0, 0.1, 0.9}, 1599),
		book(14, 2, "The Player of Games", 1988, 4.4, 584244600, []float32{0.5, 0.5, 0, 0}, 1650),
		book(15, 3, "Kindred", 1979, 4.5, 297129300, []float32{0, 0.5, 0.5, 0}, 1250),
		book(16, 3, "Parable of the Sower", 1993, 4.4, 726824400, []float32{0, 0, 0.5, 0.5}, 1375),
		book(17, 4, "Solaris", 1961, 4.3, -260006400, []float32{0.25, 0.25, 0.25, 0.25}, 1100),
		book(18, 4, "The Cyberiad", 1965, 4.4, -127248300, []float32{0.8, 0, 0.2, 0}, 1050),
		// A book whose author id matches nobody, so a right or full join has
		// an unmatched right side to show.
		book(19, 99, "Author Unknown", 1955, 3.2, -452489700, []float32{0, 0.8, 0, 0.2}, 999),
		// Published before 1960, so the `reader` role's row policy hides it
		// and the identity switcher has something to demonstrate.
		book(20, 4, "The Astronauts", 1951, 3.6, -596808000, []float32{0.2, 0, 0, 0.8}, 875),
	}
	if _, err := session.Upsert(ctx, "books", books...); err != nil {
		return err
	}

	sale := func(id, book uint64, units int64) []slate.Value {
		return []slate.Value{slate.Uint(id), slate.Uint(book), slate.Int(units)}
	}
	sales := [][]slate.Value{
		sale(100, 10, 240), sale(101, 11, 310), sale(102, 12, 280),
		sale(103, 13, 190), sale(104, 14, 210), sale(105, 15, 260),
		sale(106, 16, 300), sale(107, 17, 175), sale(108, 18, 140),
		sale(109, 19, 90), sale(110, 20, 60),
	}
	if _, err := session.Upsert(ctx, "sales", sales...); err != nil {
		return err
	}

	// Editions, so `sales -> books -> editions` has a second level. Book 10
	// has two and book 11 has one; book 12 has none at all, which is the row
	// that separates "the rows at the bottom" from "the rows with nothing
	// below them" when a path's middle level is dropped.
	edition := func(id, book uint64, format string) []slate.Value {
		return []slate.Value{slate.Uint(id), slate.Uint(book), slate.String(format)}
	}
	editions := [][]slate.Value{
		edition(500, 10, "hardback"),
		edition(501, 10, "paperback"),
		edition(502, 11, "paperback"),
	}
	_, err := session.Upsert(ctx, "editions", editions...)
	return err
}
