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

	book := func(id, author uint64, title string, year int64, rating float64) []slate.Value {
		return []slate.Value{
			slate.Uint(id), slate.Uint(author), slate.String(title),
			slate.Int(year), slate.Float(rating),
		}
	}
	books := [][]slate.Value{
		book(10, 1, "A Wizard of Earthsea", 1968, 4.4),
		book(11, 1, "The Dispossessed", 1974, 4.6),
		book(12, 1, "The Left Hand of Darkness", 1969, 4.5),
		book(13, 2, "Consider Phlebas", 1987, 4.1),
		book(14, 2, "The Player of Games", 1988, 4.4),
		book(15, 3, "Kindred", 1979, 4.5),
		book(16, 3, "Parable of the Sower", 1993, 4.4),
		book(17, 4, "Solaris", 1961, 4.3),
		book(18, 4, "The Cyberiad", 1965, 4.4),
		// A book whose author id matches nobody, so a right or full join has
		// an unmatched right side to show.
		book(19, 99, "Author Unknown", 1955, 3.2),
		// Published before 1960, so the `reader` role's row policy hides it
		// and the identity switcher has something to demonstrate.
		book(20, 4, "The Astronauts", 1951, 3.6),
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
	_, err := session.Upsert(ctx, "sales", sales...)
	return err
}
