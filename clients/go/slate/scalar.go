package slate

import (
	pb "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
)

// Scalar is a value computed from a row, rather than read out of one.
//
// A query's computed values are appended after its table's own columns, and a
// filter, a sort key, a GROUP BY key or an aggregate names one with
// [ComputedAt] — so none of them has to learn what an expression is. That is
// the kernel's arrangement; this file only builds the expressions.
//
// Two rules the server enforces and this package surfaces rather than
// duplicates:
//
//   - A later computed value may read an earlier one; reading forwards is
//     refused. The values are numbered in the order [Query.Compute] declares
//     them, so `ComputedAt(0, 0)` inside computed value 1 is the natural thing
//     to write and the illegal direction is the awkward one.
//   - An input's computed value has no slot in a *joined* row, because the
//     kernel packs a joined row by declared table width. Naming one across
//     inputs is refused by the server with that reason. [JoinComputed] names
//     the other kind — a value belonging to the join itself, evaluated over the
//     whole joined row and able to read any input — which does have a slot,
//     past every input's columns.
//
// A `Scalar` with no node set is a client bug the server refuses; every
// constructor here sets one, and the zero value is never handed out.
type Scalar struct{ wire *pb.Scalar }

// Col reads a column of the query's own table.
func Col(o Ordinal) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Column{Column: columnRef(o)}}}
}

// Ref reads any reference: a column of an input, a computed value, or one of
// the join's own. See [At], [ComputedAt] and [JoinComputed].
func Ref(c Column) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Column{Column: c.ref()}}}
}

// Lit is a constant.
//
// A [Value] rather than a bare Go number, for the reason [Value] exists: a
// literal has no declared type and the wire has two integer widths that do not
// compare equal, so `2` would have to guess which one the column is.
func Lit(v Value) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Literal{Literal: v.toProto()}}}
}

func pair(left, right Scalar) *pb.ScalarPair {
	return &pb.ScalarPair{Left: left.wire, Right: right.wire}
}

// Add is `a + b`, on numbers. String concatenation is [Concat].
func Add(a, b Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Add{Add: pair(a, b)}}}
}

// Sub is `a - b`.
func Sub(a, b Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Sub{Sub: pair(a, b)}}}
}

// Mul is `a * b`.
func Mul(a, b Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Mul{Mul: pair(a, b)}}}
}

// Div is `a / b`. Integer division on two integers, as the kernel's is.
func Div(a, b Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Div{Div: pair(a, b)}}}
}

// Length is the length of a string in characters, or of a byte string in
// bytes.
func Length(s Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Length{Length: s.wire}}}
}

// Lower lowercases a string.
func Lower(s Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Lower{Lower: s.wire}}}
}

// Upper uppercases a string.
func Upper(s Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Upper{Upper: s.wire}}}
}

// Round rounds a number to the nearest integer, halves away from zero.
//
// Returns an integer, so it can be a group key without the grouping depending
// on float equality.
func Round(s Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Round{Round: s.wire}}}
}

func scalarList(parts []Scalar) *pb.ScalarList {
	out := make([]*pb.Scalar, 0, len(parts))
	for _, p := range parts {
		out = append(out, p.wire)
	}
	return &pb.ScalarList{Scalars: out}
}

// Concat joins strings end to end.
func Concat(parts ...Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Concat{Concat: scalarList(parts)}}}
}

// Coalesce is the first part that is not null.
func Coalesce(parts ...Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Coalesce{Coalesce: scalarList(parts)}}}
}

// TimeUnit is which part of a timestamp, for [Extract] and [DateTrunc].
//
// Timestamps are seconds since the epoch in an integer column: there is no date
// type to be more precise about. Every member here is a fixed number of
// seconds, which is what lets [DateTrunc] be defined as arithmetic — a month is
// not, and lives on [CalendarPart] instead.
type TimeUnit int

// The time units.
const (
	// Second is one second.
	Second TimeUnit = iota
	// Minute is sixty seconds.
	Minute
	// Hour is 3600 seconds.
	Hour
	// Day is 86400 seconds.
	Day
)

func (u TimeUnit) wire() pb.TimeUnit {
	switch u {
	case Minute:
		return pb.TimeUnit_TIME_UNIT_MINUTE
	case Hour:
		return pb.TimeUnit_TIME_UNIT_HOUR
	case Day:
		return pb.TimeUnit_TIME_UNIT_DAY
	default:
		return pb.TimeUnit_TIME_UNIT_SECOND
	}
}

// CalendarPart is a calendar field of a timestamp, which [TimeUnit] cannot
// name.
//
// Separate from [TimeUnit] because that one promises a fixed number of seconds
// and a month has none. A `Month` member there would give it a length that is
// a lie, and the lie would be silent.
type CalendarPart int

// The calendar fields.
const (
	// Year is the proleptic Gregorian year, negative before 1 CE.
	Year CalendarPart = iota
	// Month is 1 to 12.
	Month
	// DayOfMonth is 1 to 31.
	//
	// Named in full rather than `Day`, because [Day] counts days since the
	// epoch and the two are both plausible-looking integers. SQL's
	// `EXTRACT(DAY FROM t)` means this one.
	DayOfMonth
	// DayOfWeek is 0 for Sunday through 6 for Saturday, matching ClickHouse,
	// MySQL and SQLite rather than ISO.
	DayOfWeek
)

func (p CalendarPart) wire() pb.CalendarPart {
	switch p {
	case Month:
		return pb.CalendarPart_CALENDAR_PART_MONTH
	case DayOfMonth:
		return pb.CalendarPart_CALENDAR_PART_DAY_OF_MONTH
	case DayOfWeek:
		return pb.CalendarPart_CALENDAR_PART_DAY_OF_WEEK
	default:
		return pb.CalendarPart_CALENDAR_PART_YEAR
	}
}

// Extract is one part of a timestamp, as a number.
func Extract(unit TimeUnit, s Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Extract{
		Extract: &pb.TimePart{Unit: unit.wire(), Value: s.wire},
	}}}
}

// DateTrunc is a timestamp truncated to `unit`.
func DateTrunc(unit TimeUnit, s Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_DateTrunc{
		DateTrunc: &pb.TimePart{Unit: unit.wire(), Value: s.wire},
	}}}
}

// PartOf is a calendar field of a timestamp: the year, the day of the week.
//
// Timestamps are seconds since the epoch in an integer column, read in UTC.
// There is no date type to carry a zone, so reading one in local time is done
// by shifting the timestamp first: `PartOf(part, InZone(name, column))` for a
// named zone, or `PartOf(part, Add(column, Lit(I64(3600*hours))))` for a fixed
// offset, which is what such a conversion is.
func PartOf(part CalendarPart, s Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_CalendarPart{
		CalendarPart: &pb.CalendarField{Part: part.wire(), Value: s.wire},
	}}}
}

// YearOf is the year of a timestamp.
func YearOf(s Scalar) Scalar { return PartOf(Year, s) }

// MonthOf is the month, 1 to 12.
func MonthOf(s Scalar) Scalar { return PartOf(Month, s) }

// DayOfMonthOf is the day of the month, 1 to 31.
func DayOfMonthOf(s Scalar) Scalar { return PartOf(DayOfMonth, s) }

// DayOfWeekOf is the day of the week, 0 for Sunday.
func DayOfWeekOf(s Scalar) Scalar { return PartOf(DayOfWeek, s) }

// CalendarUnit is a calendar boundary [CalendarTruncOf] can floor a timestamp
// to.
//
// Separate from [TimeUnit] for the reason [CalendarPart] is: those are all a
// fixed number of seconds and a month is not, so `DateTrunc` is a division
// while this decodes the date, drops the fields below the boundary and encodes
// it again.
//
// A day is absent on purpose — it *is* a fixed number of seconds, so
// `DateTrunc(Day, t)` already means it.
type CalendarUnit int

// The calendar boundaries.
const (
	// MonthStart is the first instant of the month, in UTC.
	MonthStart CalendarUnit = iota
	// YearStart is the first instant of the year, in UTC.
	YearStart
)

func (u CalendarUnit) wire() pb.CalendarUnit {
	if u == YearStart {
		return pb.CalendarUnit_CALENDAR_UNIT_YEAR
	}
	return pb.CalendarUnit_CALENDAR_UNIT_MONTH
}

// CalendarTruncOf is the first instant of the month or year containing a
// timestamp, in UTC.
//
// Floors, including below the epoch: an instant in December 1969 truncates to
// 1969-12-01 rather than forward to 1970-01-01.
func CalendarTruncOf(unit CalendarUnit, s Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_CalendarTrunc{
		CalendarTrunc: &pb.CalendarTrunc{Unit: unit.wire(), Value: s.wire},
	}}}
}

// MonthStartOf is the first instant of the month, in UTC.
func MonthStartOf(s Scalar) Scalar { return CalendarTruncOf(MonthStart, s) }

// YearStartOf is the first instant of the year, in UTC.
func YearStartOf(s Scalar) Scalar { return CalendarTruncOf(YearStart, s) }

// CaseBranch is one `WHEN ... THEN ...` of a [Case].
type CaseBranch struct {
	When Expr
	Then Scalar
}

// Case is SQL's `CASE WHEN`.
//
// `otherwise` is required, because SQL's `CASE` with no `ELSE` produces null
// and an explicit null literal says so — where an absent field would be a
// client that forgot. The wire makes the same demand.
func Case(branches []CaseBranch, otherwise Scalar) Scalar {
	wire := make([]*pb.CaseBranch, 0, len(branches))
	for _, b := range branches {
		wire = append(wire, &pb.CaseBranch{When: b.When.wire, Then: b.Then.wire})
	}
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Case{
		Case: &pb.Case{Branches: wire, Otherwise: otherwise.wire},
	}}}
}

// Metric is how to measure the distance between two vectors.
type Metric int

// The distance metrics.
const (
	// L2 is Euclidean distance.
	L2 Metric = iota
	// L2Squared skips the square root, which orders the same way for less work.
	L2Squared
	// Cosine is one minus the cosine similarity.
	Cosine
	// NegativeInnerProduct orders by inner product, largest first.
	NegativeInnerProduct
)

func (m Metric) wire() pb.Metric {
	switch m {
	case L2Squared:
		return pb.Metric_METRIC_L2_SQUARED
	case Cosine:
		return pb.Metric_METRIC_COSINE
	case NegativeInnerProduct:
		return pb.Metric_METRIC_NEGATIVE_INNER_PRODUCT
	default:
		return pb.Metric_METRIC_L2
	}
}

// Distance is the distance between two vectors.
func Distance(left, right Scalar, metric Metric) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_Distance{
		Distance: &pb.Distance{Left: left.wire, Right: right.wire, Metric: metric.wire()},
	}}}
}

// RegexpReplace replaces every match of `pattern` in `value`.
//
// Capture references may be written `$1` or `\1`; the server accepts both,
// because every SQL dialect spells it with a backslash and inserting the
// literal text would be a wrong answer that looks like a right one.
func RegexpReplace(value Scalar, pattern, replacement string) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_RegexpReplace{
		RegexpReplace: &pb.RegexpReplace{
			Value: value.wire, Pattern: pattern, Replacement: replacement,
		},
	}}}
}

func scalarsToProto(scalars []Scalar) []*pb.Scalar {
	if len(scalars) == 0 {
		return nil
	}
	out := make([]*pb.Scalar, 0, len(scalars))
	for _, s := range scalars {
		out = append(out, s.wire)
	}
	return out
}

// InZone reads a UTC timestamp as local time in a named IANA zone.
//
// Adds the zone's offset *at that instant*, so anything wrapped around the
// result reads the local wall clock:
//
//	slate.Extract(slate.Hour, slate.InZone("America/New_York", pickup))
//
// which is the local hour, daylight saving included, rather than the UTC one.
//
// The name is case-sensitive, as IANA names are: "america/new_york" is not a
// zone. This does not check it — the server holds the list, refuses a name it
// does not have, and names the ones it does. A copy of the list here would be
// a copy that goes stale silently, which is worse than a round trip to be
// told.
func InZone(zone string, s Scalar) Scalar {
	return Scalar{&pb.Scalar{Node: &pb.Scalar_ZoneShift{
		ZoneShift: &pb.ZoneShift{Zone: zone, Value: s.wire},
	}}}
}
