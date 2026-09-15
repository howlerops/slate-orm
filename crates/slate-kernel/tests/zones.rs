//! Named timezones, against dates anyone can check and against a second
//! implementation.
//!
//! `Scalar::ZoneShift` adds a zone's offset *at that instant*, so every calendar
//! scalar reads the local wall clock with no change of its own. The table it
//! reads is generated from the system's IANA `tzdata` — see
//! `scripts/generate_zones.py` — which means the interesting question is not
//! "does the lookup agree with the table" but "is the table the right table,
//! and is the lookup reading it the right way round".
//!
//! Two kinds of test, because neither is enough alone:
//!
//! 1. **Stated facts.** DST boundaries and offsets written out here, from
//!    history rather than from the generator: the 2007 US rule change, Arizona
//!    never moving, India's half hour, Nepal's quarter hour, Sydney's summer in
//!    January. A generator that produced a plausible-but-shifted table passes a
//!    round trip and fails these.
//! 2. **A shape check on every zone**, which catches a table that is truncated,
//!    unsorted, or indexed off by one — the failures that would otherwise show
//!    up as a wrong answer in one month of one year.

#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use slate_kernel::{CalendarPart, Scalar, TimeUnit, zones};
use slate_schema::Row;
use slate_tuple::Value;

/// The local hour in `zone` at a UTC instant.
fn hour_in(seconds: i64, zone: &str) -> Value {
    Scalar::Literal(Value::I64(seconds))
        .in_zone(zone)
        .extract(TimeUnit::Hour)
        .evaluate(&Row::new(vec![]))
}

/// The local calendar date in `zone`, as `(year, month, day)`.
fn date_in(seconds: i64, zone: &str) -> (i64, i64, i64) {
    let shifted = Scalar::Literal(Value::I64(seconds)).in_zone(zone);
    let part = |part: CalendarPart| match shifted
        .clone()
        .calendar_part(part)
        .evaluate(&Row::new(vec![]))
    {
        Value::I64(n) => n,
        other => panic!("{part:?} in {zone} gave {other:?}"),
    };
    (
        part(CalendarPart::Year),
        part(CalendarPart::Month),
        part(CalendarPart::DayOfMonth),
    )
}

/// The offset a zone is at, in seconds, derived from the shift itself.
fn offset_at(seconds: i64, zone: &str) -> i64 {
    match Scalar::Literal(Value::I64(seconds))
        .in_zone(zone)
        .evaluate(&Row::new(vec![]))
    {
        Value::I64(shifted) => shifted - seconds,
        other => panic!("{zone} at {seconds} gave {other:?}"),
    }
}

// --- stated facts ---------------------------------------------------------

/// New York in winter and summer, and the hour the taxi sample is keyed on.
///
/// 2024-01-15T12:00:00Z is 07:00 in New York (UTC-5, standard time) and
/// 2024-07-15T12:00:00Z is 08:00 (UTC-4, daylight time). Both checkable with
/// `TZ=America/New_York date -d @<seconds>`.
#[test]
fn new_york_moves_an_hour_between_january_and_july() {
    let january = 1_705_320_000; // 2024-01-15T12:00:00Z
    let july = 1_721_044_800; // 2024-07-15T12:00:00Z
    assert_eq!(hour_in(january, "America/New_York"), Value::I64(7));
    assert_eq!(hour_in(july, "America/New_York"), Value::I64(8));
    assert_eq!(offset_at(january, "America/New_York"), -5 * 3600);
    assert_eq!(offset_at(july, "America/New_York"), -4 * 3600);
}

/// The 2007 rule change, which is the fact a projected table gets wrong.
///
/// The United States moved DST's start from the first Sunday in April to the
/// second Sunday in March, effective 2007. So 2006-03-15 was standard time and
/// 2007-03-15 was daylight time — the same calendar date, a different offset,
/// one year apart. A table generated from a single rule cannot have both.
#[test]
fn the_2007_dst_rule_change_is_in_the_table() {
    // 2006-03-15T12:00:00Z and 2007-03-15T12:00:00Z.
    assert_eq!(offset_at(1_142_424_000, "America/New_York"), -5 * 3600);
    assert_eq!(offset_at(1_173_960_000, "America/New_York"), -4 * 3600);
}

/// Arizona does not observe DST, so Phoenix never moves.
///
/// The same instants that shift New York by an hour leave Phoenix at UTC-7 all
/// year. A table that applied one zone's rules to another would fail here and
/// nowhere else, because every other US zone in the list *does* move.
#[test]
fn phoenix_never_moves() {
    for instant in [1_705_320_000_i64, 1_721_044_800] {
        assert_eq!(
            offset_at(instant, "America/Phoenix"),
            -7 * 3600,
            "Arizona opted out of daylight saving"
        );
    }
}

/// India is half an hour off, and Nepal is a quarter.
///
/// Both are whole-hour tables' classic failure, and Kathmandu is half-hour
/// tables' — +05:45. Neither observes DST, so one instant settles it.
#[test]
fn half_and_quarter_hour_offsets_are_exact() {
    let instant = 1_705_320_000; // 2024-01-15T12:00:00Z
    assert_eq!(offset_at(instant, "Asia/Kolkata"), 5 * 3600 + 1800);
    assert_eq!(offset_at(instant, "Asia/Kathmandu"), 5 * 3600 + 2700);
    // 12:00 UTC is 17:30 in Kolkata and 17:45 in Kathmandu, so the *hour* is
    // 17 in both and only the minute distinguishes them — which is why the
    // offset is asserted rather than the hour.
    assert_eq!(hour_in(instant, "Asia/Kolkata"), Value::I64(17));
    assert_eq!(hour_in(instant, "Asia/Kathmandu"), Value::I64(17));
}

/// Sydney's summer is in January, so its DST sign is the opposite way round.
///
/// A northern-hemisphere assumption baked into a lookup — "the larger offset is
/// the summer one, and summer is July" — gives the right answer in New York and
/// the wrong one here.
#[test]
fn sydney_is_on_daylight_time_in_january_and_not_in_july() {
    let january = 1_705_320_000; // 2024-01-15T12:00:00Z
    let july = 1_721_044_800; // 2024-07-15T12:00:00Z
    assert_eq!(offset_at(january, "Australia/Sydney"), 11 * 3600);
    assert_eq!(offset_at(july, "Australia/Sydney"), 10 * 3600);
    // And the *date* rolls over, which is the thing a caller notices: 12:00
    // UTC is 23:00 on the 15th in January and 22:00 on the 15th in July.
    assert_eq!(date_in(january, "Australia/Sydney"), (2024, 1, 15));
    assert_eq!(hour_in(january, "Australia/Sydney"), Value::I64(23));
}

/// A transition's instant belongs to the *new* offset, not the old one.
///
/// This is the one boundary a binary search gets wrong for free: an inclusive
/// bound written exclusively is right everywhere except at the transition's own
/// second, which is 1 second in 15 million and never hit by a test aimed at the
/// middle of a season. It is here because changing `<=` to `<` in the lookup
/// broke nothing at all — every stated fact above still passed.
///
/// New York springs forward at 2024-03-10T07:00:00Z and falls back at
/// 2024-11-03T06:00:00Z; both are entries in the table. The assertions are on
/// the transition second and the second before it, from either direction, so a
/// bound off by one in *either* direction fails.
#[test]
fn a_transition_takes_effect_at_its_own_second() {
    let spring = 1_710_054_000; // 2024-03-10T07:00:00Z
    assert_eq!(offset_at(spring - 1, "America/New_York"), -5 * 3600);
    assert_eq!(offset_at(spring, "America/New_York"), -4 * 3600);
    assert_eq!(offset_at(spring + 1, "America/New_York"), -4 * 3600);

    let fall = 1_730_613_600; // 2024-11-03T06:00:00Z
    assert_eq!(offset_at(fall - 1, "America/New_York"), -4 * 3600);
    assert_eq!(offset_at(fall, "America/New_York"), -5 * 3600);
    assert_eq!(offset_at(fall + 1, "America/New_York"), -5 * 3600);

    // And the local clock does what the folklore says: 01:59:59 is followed by
    // 03:00:00 in the spring, and 01:00:00 happens twice in the autumn — the
    // second of which is what this returns, since the lookup answers with the
    // offset in force and has no way to say "ambiguous".
    assert_eq!(hour_in(spring - 1, "America/New_York"), Value::I64(1));
    assert_eq!(hour_in(spring, "America/New_York"), Value::I64(3));
    assert_eq!(hour_in(fall - 1, "America/New_York"), Value::I64(1));
    assert_eq!(hour_in(fall, "America/New_York"), Value::I64(1));
}

/// A shift can move the date, which is the case a caller gets wrong.
///
/// 2024-01-15T02:00:00Z is still the 14th in New York — 21:00 the previous
/// evening. Reading the year, month and day out of a UTC timestamp and calling
/// it "the local date" is exactly the mistake this closes.
#[test]
fn a_zone_shift_can_move_the_calendar_date() {
    let instant = 1_705_284_000; // 2024-01-15T02:00:00Z
    assert_eq!(date_in(instant, "UTC"), (2024, 1, 15));
    assert_eq!(date_in(instant, "America/New_York"), (2024, 1, 14));
    assert_eq!(hour_in(instant, "America/New_York"), Value::I64(21));
}

/// UTC is the identity, and is in the table so a caller can say so.
#[test]
fn utc_is_the_identity() {
    for instant in [0_i64, 1_705_320_000, -2_208_988_800] {
        assert_eq!(offset_at(instant, "UTC"), 0);
    }
}

/// An unknown zone is null rather than a guess.
///
/// Null rather than an error because a `Scalar` has nowhere to put one. The
/// refusal a caller should see belongs at the edge, and the SQL front end and
/// the clients name the zones that exist — there is a test for that where
/// they are.
#[test]
fn an_unknown_zone_is_null() {
    assert_eq!(hour_in(0, "Mars/Olympus_Mons"), Value::Null);
    // Case matters, as IANA names do: `america/new_york` is not a zone.
    assert_eq!(hour_in(0, "america/new_york"), Value::Null);
}

/// A non-integer timestamp is null, as it is everywhere else here.
#[test]
fn a_non_timestamp_is_null() {
    let scalar = Scalar::Literal(Value::Str("nope".to_owned())).in_zone("UTC");
    assert_eq!(scalar.evaluate(&Row::new(vec![])), Value::Null);
}

// --- the shape of every table --------------------------------------------

/// Zones whose transitions stop inside the window, and the year each stopped.
///
/// Every one of these is a fact about the world rather than a gap in the
/// table, and each is checkable with `zoneinfo`:
///
/// - Arizona opted out of daylight saving in 1968, so Phoenix's last
///   transition is the end of DST in October 1967.
/// - Japan's occupation-era daylight saving was repealed in 1952; Tokyo's
///   last transition is September 1951 and it has been +09:00 since.
/// - India dropped its wartime daylight saving in 1945 and settled on
///   +05:30.
/// - Nepal moved from +05:30 to +05:45 at the start of 1986 and has not
///   moved since.
/// - `UTC` has exactly the window's opening entry, by construction.
const STOPPED_CHANGING: &[(&str, i64, &str)] = &[
    (
        "America/Phoenix",
        1967,
        "Arizona opted out of daylight saving",
    ),
    (
        "Asia/Tokyo",
        1951,
        "Japan repealed its post-war daylight saving",
    ),
    (
        "Asia/Kolkata",
        1945,
        "India dropped wartime daylight saving",
    ),
    (
        "Asia/Kathmandu",
        1985,
        "Nepal moved to +05:45 and has not moved since",
    ),
    (
        "UTC",
        1900,
        "UTC is the identity and has only the opening entry",
    ),
];

/// The UTC year an instant falls in.
///
/// This reads the year with the kernel's own calendar scalar, which is
/// circular only in appearance: `tests/calendar.rs` checks that path against
/// an independent implementation over four centuries, so a wrong year here
/// would already have failed there.
fn year_of(instant: i64) -> i64 {
    match Scalar::Literal(Value::I64(instant))
        .calendar_part(CalendarPart::Year)
        .evaluate(&Row::new(vec![]))
    {
        Value::I64(year) => year,
        other => panic!("the year of {instant} came back as {other:?}"),
    }
}

/// Every zone's table is sorted, non-empty, and covers the window it claims.
///
/// These are the failures a stated-fact test cannot reach: a table truncated
/// halfway, one accidentally sorted by offset, one whose first entry is after
/// the window's start so a 1950 instant falls off the front.
#[test]
fn every_table_is_sorted_and_covers_the_window() {
    assert!(!zones::ZONES.is_empty());
    let (from, to) = zones::COVERS;
    let window_start = (from - 1970) * 31_556_952; // approximate, for a bound
    for zone in zones::ZONES {
        assert!(
            !zone.transitions.is_empty(),
            "{} has no transitions",
            zone.name
        );
        let first = zone.transitions[0].0;
        assert!(
            first <= window_start + 31_556_952,
            "{}'s table starts at {first}, after {from}",
            zone.name
        );
        for pair in zone.transitions.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "{}'s transitions are not ascending: {} then {}",
                zone.name,
                pair[0].0,
                pair[1].0
            );
        }
        let last = zone.transitions[zone.transitions.len() - 1].0;
        // A table that stops in the middle of the window is the failure to
        // catch, and "the last entry is near the end" is the wrong way to
        // catch it: four of these zones stopped changing decades ago and
        // their tables legitimately end there. So the stop year is a stated
        // fact per zone (STOPPED_CHANGING), and every zone not listed must
        // still be moving at the end of the window.
        //
        // This is stricter than the bound it replaces, not looser. A
        // truncated Phoenix fails on the year; a Phoenix that acquired a
        // 2027 transition fails too, which is what should happen — Arizona
        // adopting daylight saving is news, not a detail to absorb quietly.
        match STOPPED_CHANGING
            .iter()
            .find(|(name, _, _)| *name == zone.name)
        {
            Some((_, year, why)) => assert_eq!(
                year_of(last),
                *year,
                "{}'s table ends at {last}; it should stop in {year}, because {why}",
                zone.name
            ),
            None => {
                let window_end = (to - 1970) * 31_556_952;
                assert!(
                    last > window_end - 3 * 31_556_952,
                    "{}'s table stops at {last}, well before {to}; if it stopped \
                     observing daylight saving, say so in STOPPED_CHANGING",
                    zone.name
                );
            }
        }
    }
    // And sorted by name, which is what lets the lookup bisect.
    for pair in zones::ZONES.windows(2) {
        assert!(
            pair[0].name < pair[1].name,
            "the zone list is not sorted: {} then {}",
            pair[0].name,
            pair[1].name
        );
    }
}

/// The offset never jumps by more than a couple of hours at a transition.
///
/// A real DST transition is 30 or 60 minutes; a few historical ones are larger,
/// and none is a day. A table whose instants and offsets got interleaved — the
/// exact failure a generator writing pairs in the wrong order produces — shows
/// up here as a jump of thousands of hours rather than as a wrong month.
#[test]
fn no_transition_moves_the_clock_by_more_than_a_few_hours() {
    for zone in zones::ZONES {
        for pair in zone.transitions.windows(2) {
            let jump = i64::from(pair[1].1) - i64::from(pair[0].1);
            assert!(
                jump.abs() <= 2 * 3600,
                "{} jumps {jump} seconds at {}",
                zone.name,
                pair[1].0
            );
        }
        // And every offset is a real one: within +/- 14 hours of UTC, which is
        // the range IANA uses, and a whole number of minutes.
        for (instant, offset) in zone.transitions {
            assert!(
                offset.abs() <= 14 * 3600,
                "{} is {offset} seconds from UTC at {instant}",
                zone.name
            );
        }
        // Every offset a zone *adopted* is a whole number of minutes. The
        // opening entry is exempt, and the exemption is not slack: before a
        // zone standardised, its offset was local mean time, which is
        // seconds-precise and genuinely not a whole minute. Paris was
        // +00:09:21 in 1900, Kolkata +05:21:10, Kathmandu +05:41:16 — real
        // values, checkable with `zoneinfo`. Skipping the first entry keeps
        // the check exact rather than widening it to "a whole second", which
        // would assert nothing.
        for (instant, offset) in &zone.transitions[1..] {
            assert_eq!(
                offset % 60,
                0,
                "{}'s offset at {instant} is not a whole minute",
                zone.name
            );
        }
    }
}
