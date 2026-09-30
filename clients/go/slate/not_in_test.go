package slate_test

// `Not(In(...))` — the composition every client has and none of them tested.
//
// # Why this file exists
//
// `ledger/2026-09-18-not-in-was-refused-on-a-claim-that-does-not-hold.md`
// taught the SQL front end to write `NOT IN` and then said of the clients:
//
//	**No client can express it.** `notIn` is a front-end operator; the wire
//	carries `Expr`, and the three SDKs build `Expr::In` with no negation
//	helper. A client that wants this builds the `Not` itself, which is
//	possible and undocumented.
//
// Its two sentences contradict each other and the second is the true one:
// `slate.Not` has been here the whole time, so `Not(In(...))` is the
// expression. `ledger/2026-09-29-three-could-be-anothers-counted.md` withdrew
// the first half and found something the caveat had not — **no test in any of
// the three clients used the negation helper at all.** Three doors, nobody
// walking through any of them.
//
// This is the Go one. It is the same shape as `disjunction_test.go` and for
// the same reason: a claim about what a client cannot do got written by
// reading the front end, and the only thing that stops the next one is
// something that runs.
//
// # The oracle
//
// `NOT IN` is the complement of `IN` **over the rows that exist**, which is
// the assertion here — the two together must cover every seeded id and share
// none. That is stronger than checking `NOT IN` against a hand-written list,
// because it fails if either arm drifts, and it is the property a reader
// actually relies on.
//
// It is not the *SQL* `NOT IN`, and the difference matters: SQL's is
// three-valued, so a null in the list makes the answer unknown and admits
// nothing. The kernel's `Expr::In` has the same rule — `docs/correctness.md`
// covers it — and this fixture has no nulls, so the complement holds. A
// nullable column would need its own case and does not have one.

import (
	"sort"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// Three of the six kinds, so the list admits some rows and excludes others.
// A list containing every kind would make the complement empty and a list
// containing none would make it everything; either passes a broken `Not` that
// returned its argument.
var excluded = []slate.Value{slate.String("memo"), slate.String("sheet")}

func TestAClientSendsNotInAsNotOfIn(t *testing.T) {
	session := disjunctive(t)

	inside := matching(t, session, slate.In(1, excluded...))
	outside := matching(t, session, slate.Not(slate.In(1, excluded...)))

	// Both arms must be non-empty, or the complement assertion below is
	// satisfied by a server that answered nothing to one of them.
	if len(inside) == 0 || len(outside) == 0 {
		t.Fatalf("an empty arm proves nothing: in=%v notIn=%v", inside, outside)
	}

	// Disjoint.
	if shared := intersection(inside, outside); len(shared) != 0 {
		t.Fatalf("IN and NOT IN both admit %v, so the negation did nothing", shared)
	}

	// And exhaustive, over the fixture. Read off `disjoined` rather than
	// written out, so a row added there is covered rather than silently
	// outside the claim.
	every := make([]uint64, 0, len(disjoined))
	for _, row := range disjoined {
		every = append(every, row[0].(uint64))
	}
	sort.Slice(every, func(i, j int) bool { return every[i] < every[j] })

	if covered := union(inside, outside); !sameIDs(covered, every) {
		t.Fatalf("IN and NOT IN together admit %v, not the %v rows seeded", covered, every)
	}
}

func TestNegationIsNotIgnoredOnASingleComparison(t *testing.T) {
	// The narrower half, and the one that would catch a `Not` that unwrapped
	// to its argument on the wire: the composition above is disjoint and
	// exhaustive whether or not the negation is applied *if* the server
	// happened to answer the complement for `In` too, which is far-fetched but
	// not excluded by that test alone. One comparison and its negation is.
	session := disjunctive(t)

	notes := matching(t, session, slate.Eq(1, slate.String("note")))
	others := matching(t, session, slate.Not(slate.Eq(1, slate.String("note"))))

	if len(notes) == 0 || len(others) == 0 {
		t.Fatalf("an empty arm proves nothing: eq=%v not=%v", notes, others)
	}
	if sameIDs(notes, others) {
		t.Fatalf("a comparison and its negation admit the same rows (%v)", notes)
	}
	if shared := intersection(notes, others); len(shared) != 0 {
		t.Fatalf("a comparison and its negation both admit %v", shared)
	}
}
