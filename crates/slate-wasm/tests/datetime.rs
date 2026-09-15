//! Time functions in the SQL front end, against the real January 2024 sample.
//!
//! The kernel has had `Extract` and `DateTrunc` since scalars arrived, and
//! `CalendarPart` was added alongside this. None of it was reachable from SQL,
//! which is why the site's ClickHouse comparisons were *adapted* rather than
//! replicated: every one of them keys on `toYear(pickup_datetime)`, and there
//! was no year to key on.
//!
//! The interesting tests here are the oracle: rather than assert that a
//! particular trip falls in a particular hour, decode the sample independently
//! and fold it by hand, then check the kernel agrees group for group. That
//! catches the cases nobody thought to write down — a timezone assumption, an
//! off-by-one at a day boundary, a group silently dropped.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::{Value as Json, json};
use slate_wasm::{Playground, taxi};
use std::collections::BTreeMap;
use std::io::Read;

fn trip_bytes() -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../site/data/trips.bin.gz");
    let file = std::fs::File::open(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(file)
        .read_to_end(&mut out)
        .unwrap();
    out
}

fn loaded() -> Playground {
    let mut playground = Playground::new();
    let outcome: Json = serde_json::from_str(&playground.load_trips(&trip_bytes())).unwrap();
    assert_eq!(outcome["ok"], json!(100_000), "{outcome}");
    playground
}

fn run(playground: &Playground, text: &str) -> Json {
    let all: Vec<Json> = serde_json::from_str(&playground.sql(text)).unwrap();
    all.last().expect("a result").clone()
}

fn ok(playground: &Playground, text: &str) -> Json {
    let last = run(playground, text);
    assert!(last["error"].is_null(), "{text}: {}", last["error"]);
    last
}

fn refused(playground: &Playground, text: &str) -> String {
    let last = run(playground, text);
    last["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("{text} was accepted: {last}"))
        .to_owned()
}

/// A grouped result as key → count, for comparing against a fold.
fn counts(answer: &Json) -> BTreeMap<i64, i64> {
    answer["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row[0].as_str().unwrap().parse().unwrap(),
                row[1].as_str().unwrap().parse().unwrap(),
            )
        })
        .collect()
}

/// Every trip's pickup time, decoded straight from the committed file.
///
/// The independent half of the oracle: this never goes near the kernel, the
/// parser or the scalar evaluator.
fn pickup_times() -> Vec<i64> {
    taxi::decode(&trip_bytes())
        .unwrap()
        .iter()
        .map(|row| match row.values()[3] {
            slate_tuple::Value::I64(seconds) => seconds,
            ref other => panic!("pickup_time is {other:?}"),
        })
        .collect()
}

#[test]
fn the_hour_of_day_agrees_with_a_fold_over_the_raw_seconds() {
    let playground = loaded();
    let mut expected: BTreeMap<i64, i64> = BTreeMap::new();
    for seconds in pickup_times() {
        *expected
            .entry(seconds.div_euclid(3600).rem_euclid(24))
            .or_default() += 1;
    }

    let answer = ok(
        &playground,
        "SELECT hour(pickup_time), count(*) FROM trips GROUP BY hour(pickup_time)",
    );
    assert_eq!(counts(&answer), expected, "hour of day");
    // Every hour of the day appears in a month of taxi trips, and they sum to
    // the whole sample — so nothing was dropped or double-counted.
    assert_eq!(expected.len(), 24);
    assert_eq!(expected.values().sum::<i64>(), 100_000);
}

#[test]
fn the_day_of_week_agrees_with_a_fold_and_with_the_calendar() {
    let playground = loaded();
    let mut expected: BTreeMap<i64, i64> = BTreeMap::new();
    for seconds in pickup_times() {
        *expected
            .entry((seconds.div_euclid(86_400) + 4).rem_euclid(7))
            .or_default() += 1;
    }

    let answer = ok(
        &playground,
        "SELECT day_of_week(pickup_time), count(*) FROM trips GROUP BY day_of_week(pickup_time)",
    );
    assert_eq!(counts(&answer), expected, "day of week");
    assert_eq!(expected.len(), 7, "a month covers every weekday");

    // And a fact about the calendar rather than about the fold, so the two
    // halves cannot be wrong together: 2024-01-01 was a Monday, so the sample
    // — which starts that morning — has more Mondays than Sundays.
    let by_day = ok(
        &playground,
        "SELECT day(pickup_time), day_of_week(pickup_time), count(*) FROM trips \
         GROUP BY day(pickup_time), day_of_week(pickup_time)",
    );
    let first = &by_day["rows"].as_array().unwrap()[0];
    assert_eq!(
        (first[0].as_str().unwrap(), first[1].as_str().unwrap()),
        ("1", "1"),
        "the 1st of January 2024 was a Monday, which is 1 counting Sunday as 0"
    );
}

#[test]
fn the_sample_is_one_month_of_one_year_and_says_so() {
    let playground = loaded();
    // The whole point of `year()`: the site's ClickHouse comparisons key on it
    // and could not before. That it is constant here is a fact about the
    // sample, not a limitation — and asserting it means a sample swapped for a
    // multi-year one fails loudly rather than quietly answering a different
    // question.
    let years = ok(
        &playground,
        "SELECT year(pickup_time), count(*) FROM trips GROUP BY year(pickup_time)",
    );
    assert_eq!(counts(&years), BTreeMap::from([(2024, 100_000)]));

    let months = ok(
        &playground,
        "SELECT month(pickup_time), count(*) FROM trips GROUP BY month(pickup_time)",
    );
    assert_eq!(counts(&months), BTreeMap::from([(1, 100_000)]));

    // Thirty-one days, every one of them present.
    let days = ok(
        &playground,
        "SELECT day(pickup_time), count(*) FROM trips GROUP BY day(pickup_time)",
    );
    let days = counts(&days);
    assert_eq!(days.len(), 31, "January has 31 days");
    assert_eq!(
        days.keys().copied().collect::<Vec<_>>(),
        (1..=31).collect::<Vec<_>>()
    );
    assert_eq!(days.values().sum::<i64>(), 100_000);
}

#[test]
fn date_rounds_down_to_midnight_and_groups_by_calendar_day() {
    let playground = loaded();
    let dates = ok(
        &playground,
        "SELECT date(pickup_time), count(*) FROM trips GROUP BY date(pickup_time) \
         ORDER BY date(pickup_time)",
    );
    let dates = counts(&dates);
    assert_eq!(dates.len(), 31, "one group per calendar day");
    // Each key is midnight UTC: divisible by a day, and the first is
    // 2024-01-01. Grouping by `day()` would give the same 31 groups keyed
    // 1..31; `date()` keys them by instant, so they sort chronologically even
    // across a month boundary.
    for key in dates.keys() {
        assert_eq!(key % 86_400, 0, "{key} is not midnight");
    }
    assert_eq!(*dates.keys().next().unwrap(), 1_704_067_200);
    assert_eq!(*dates.keys().last().unwrap(), 1_704_067_200 + 30 * 86_400);
}

#[test]
fn the_same_call_twice_is_one_computed_column() {
    let playground = loaded();
    // `hour(pickup_time)` in the select list and again in GROUP BY and again
    // in ORDER BY. Registering it three times would make three identical group
    // keys: the same 24 groups, each row carrying the hour three times, and no
    // error anywhere.
    let answer = ok(
        &playground,
        "SELECT hour(pickup_time), count(*) FROM trips GROUP BY hour(pickup_time) \
         ORDER BY hour(pickup_time)",
    );
    assert_eq!(
        answer["spec"]["compute"].as_array().map(Vec::len),
        Some(1),
        "{}",
        answer["spec"]
    );
    assert_eq!(answer["columns"].as_array().map(Vec::len), Some(2));
    let hours: Vec<i64> = answer["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[0].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(hours, (0..24).collect::<Vec<_>>(), "ordered by the hour");
}

#[test]
fn two_different_calls_are_two_computed_columns() {
    let playground = loaded();
    let answer = ok(
        &playground,
        "SELECT day(pickup_time), hour(pickup_time), count(*) FROM trips \
         GROUP BY day(pickup_time), hour(pickup_time)",
    );
    assert_eq!(answer["spec"]["compute"].as_array().map(Vec::len), Some(2));
    // 31 days times 24 hours, and a January in New York has a trip in every
    // one of them.
    assert_eq!(answer["returned"], json!(744), "{}", answer["returned"]);
}

#[test]
fn a_computed_column_is_labelled_by_the_call_that_made_it() {
    let playground = loaded();
    // It has no name in the schema, so the header is rebuilt from the call.
    // Before that it read `11` — the ordinal — above a column of hours.
    let answer = ok(
        &playground,
        "SELECT hour(pickup_time), count(*), avg(total) FROM trips GROUP BY hour(pickup_time)",
    );
    assert_eq!(
        answer["columns"],
        json!(["hour(pickup_time)", "count(*)", "avg(total)"])
    );

    // Ungrouped, the computed column is appended after the table's own.
    let rows = ok(&playground, "SELECT hour(pickup_time) FROM trips LIMIT 1");
    let columns = rows["columns"].as_array().unwrap();
    assert_eq!(columns.len(), 12, "eleven columns and the computed one");
    assert_eq!(columns[11], json!("hour(pickup_time)"));
}

#[test]
fn a_computed_column_can_be_a_having_key() {
    let playground = loaded();
    // HAVING over a computed group key rather than over an aggregate: the
    // afternoon and evening hours, filtered by the key itself. Worth its own
    // test because the ordinal travels a different path here — through
    // `group_ordinal` into group space — than it does in the select list.
    let answer = ok(
        &playground,
        "SELECT hour(pickup_time), count(*) FROM trips GROUP BY hour(pickup_time) \
         HAVING hour(pickup_time) >= 18",
    );
    let hours: Vec<i64> = answer["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[0].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(
        hours,
        (18..24).collect::<Vec<_>>(),
        "the evening hours only"
    );
}

#[test]
fn round_buckets_a_float_into_an_integer_group_key() {
    let playground = loaded();
    // `round` is not a time function and shares the machinery because it is
    // the other thing a computed column is for: bucketing. It returns an
    // integer so the grouping does not depend on float equality.
    let answer = ok(
        &playground,
        "SELECT round(distance), count(*) FROM trips GROUP BY round(distance) \
         ORDER BY round(distance) LIMIT 6",
    );
    let buckets: Vec<i64> = answer["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[0].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(
        buckets,
        vec![0, 1, 2, 3, 4, 5],
        "whole-mile buckets, in order"
    );
    assert_eq!(answer["columns"][0], json!("round(distance)"));

    // Halves away from zero, which is SQL's rule and ClickHouse's: the count
    // in bucket 1 must include distances from 0.5 up to but not including 1.5.
    let mut expected = 0i64;
    for row in taxi::decode(&trip_bytes()).unwrap() {
        if matches!(row.values()[6], slate_tuple::Value::F64(miles) if (0.5..1.5).contains(&miles))
        {
            expected += 1;
        }
    }
    let one: i64 = answer["rows"].as_array().unwrap()[1][1]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(one, expected, "bucket 1 is [0.5, 1.5)");
}

#[test]
fn clickhouses_taxi_queries_replicate_rather_than_adapt() {
    let playground = loaded();
    // The two that could not be written before. Both key on the year —
    // `toYear(pickup_datetime)` in ClickHouse's own text — and with no year to
    // key on the site substituted a different column and said so. These are
    // now the same queries, column names aside.
    //
    // That the year is constant here is a property of a one-month sample, not
    // of the query: the grouping is real and would spread over a longer one.
    let q3 = ok(
        &playground,
        "SELECT passengers, year(pickup_time), count(*) FROM trips \
         GROUP BY passengers, year(pickup_time)",
    );
    assert_eq!(
        q3["columns"],
        json!(["passengers", "year(pickup_time)", "count(*)"])
    );
    let total: i64 = q3["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[2].as_str().unwrap().parse::<i64>().unwrap())
        .sum();
    assert_eq!(total, 100_000, "every trip is in exactly one group");

    let q4 = ok(
        &playground,
        "SELECT passengers, year(pickup_time), round(distance), count(*) FROM trips \
         GROUP BY passengers, year(pickup_time), round(distance) \
         ORDER BY year(pickup_time), count(*) DESC LIMIT 10",
    );
    assert_eq!(
        q4["columns"],
        json!([
            "passengers",
            "year(pickup_time)",
            "round(distance)",
            "count(*)"
        ])
    );
    let counts: Vec<i64> = q4["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[3].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(counts.len(), 10);
    assert!(
        counts.windows(2).all(|w| w[0] >= w[1]),
        "ORDER BY count(*) DESC within the year: {counts:?}"
    );
}

#[test]
fn every_function_the_parser_accepts_is_one_the_binding_lowers() {
    let playground = loaded();
    // Two lists that have to agree: the parser's, which decides what is a
    // computed column rather than an aggregate, and `compute_scalar`'s match,
    // which decides what one becomes. A name in the first and not the second
    // parses fine and then fails at lowering with "no such function", which
    // is a confusing way to learn the parser was updated and the binding was
    // not. Held together by running every one of them.
    for function in [
        "hour",
        "minute",
        "second",
        "year",
        "month",
        "day",
        "day_of_week",
        "date",
    ] {
        let sql = format!(
            "SELECT {function}(pickup_time), count(*) FROM trips \
             GROUP BY {function}(pickup_time)"
        );
        let answer = ok(&playground, &sql);
        assert!(
            answer["returned"].as_u64().unwrap_or(0) > 0,
            "{function}() returned nothing"
        );
        assert_eq!(
            answer["columns"][0],
            json!(format!("{function}(pickup_time)")),
            "{function}"
        );
    }
    // And the one that is not a time function.
    let answer = ok(
        &playground,
        "SELECT round(distance), count(*) FROM trips GROUP BY round(distance)",
    );
    assert!(answer["returned"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn it_refuses_what_it_cannot_answer() {
    let playground = loaded();

    // A timestamp is an integer of seconds; there is no date type. Calling a
    // time function on text would give a column of nulls, which looks like
    // data rather than a mistake.
    let message = refused(
        &playground,
        "SELECT hour(payment), count(*) FROM trips GROUP BY hour(payment)",
    );
    assert!(message.contains("needs a timestamp"), "{message}");
    assert!(message.contains("payment"), "{message}");

    // An unknown name. It parses as an aggregate — anything `word(...)` that
    // is not a time function does — and the message used to say so, naming a
    // category the reader never used.
    let message = refused(
        &playground,
        "SELECT nosuch(pickup_time), count(*) FROM trips GROUP BY nosuch(pickup_time)",
    );
    assert!(message.contains("no such function"), "{message}");
    assert!(
        message.contains("hour"),
        "it lists what there is: {message}"
    );

    // In the select list but not the group key: the same rule a bare column
    // follows.
    let message = refused(
        &playground,
        "SELECT hour(pickup_time), count(*) FROM trips GROUP BY pickup_zone",
    );
    assert!(
        message.contains("neither a group key nor an aggregate"),
        "{message}"
    );

    // Ordering a grouping by a call it did not group by.
    let message = refused(
        &playground,
        "SELECT day(pickup_time), count(*) FROM trips GROUP BY day(pickup_time) \
         ORDER BY hour(pickup_time)",
    );
    assert!(message.contains("does not compute"), "{message}");

    // A computed column on a join has to be the group key: a join returns
    // whole rows or one row per group, and there is no third shape.
    let message = refused(
        &playground,
        "SELECT hour(pickup_time) FROM trips JOIN zones ON trips.pickup_zone = zones.id",
    );
    assert!(message.contains("has to be the group key"), "{message}");

    // And it reads the *left* side. `zones` has no timestamp at all, so this
    // would otherwise resolve to nothing and group by null.
    let message = refused(
        &playground,
        "SELECT hour(borough), count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY hour(borough)",
    );
    assert!(message.contains("is not one"), "{message}");

    // `round` takes a number, and the message says which one it got instead.
    let message = refused(
        &playground,
        "SELECT round(payment), count(*) FROM trips GROUP BY round(payment)",
    );
    assert!(message.contains("round() needs a number"), "{message}");
}

#[test]
fn a_computed_column_can_be_grouped_ordered_and_filtered_at_once() {
    let playground = loaded();
    // The shape the site's busiest-hours example uses, and the one that
    // exercises every path a computed ordinal travels: the select list, the
    // group key, HAVING over an aggregate, and ORDER BY over the key.
    let answer = ok(
        &playground,
        "SELECT hour(pickup_time), count(*), avg(distance) FROM trips \
         GROUP BY hour(pickup_time) HAVING count(*) > 4000 ORDER BY count(*) DESC LIMIT 5",
    );
    let rows = answer["rows"].as_array().unwrap();
    assert!(!rows.is_empty() && rows.len() <= 5, "{}", rows.len());
    let mut previous = i64::MAX;
    for row in rows {
        let hour: i64 = row[0].as_str().unwrap().parse().unwrap();
        let count: i64 = row[1].as_str().unwrap().parse().unwrap();
        assert!((0..24).contains(&hour), "hour {hour}");
        assert!(count > 4000, "{count} did not pass the HAVING");
        assert!(count <= previous, "not ordered: {count} after {previous}");
        previous = count;
    }
}

// --- the zone the numbers are actually in ---------------------------------

/// The sample's `pickup_time` is New York wall clock, stored as if it were UTC.
///
/// This matters more than it sounds, and the entry that added these functions
/// got it wrong: it recorded that "trips per hour of day for New York is
/// therefore off by five", reasoning that the column is epoch seconds and the
/// extraction is UTC. Both halves are true and the conclusion is not.
/// `site/data/make-trips.py` stores each pickup as an offset from
/// `2024-01-01 00:00:00` *local*, and `taxi.rs` adds back the epoch second of
/// `2024-01-01 00:00:00` **UTC** — so the two conversions cancel and a UTC
/// extraction reads the local wall clock straight out.
///
/// The diurnal curve is the evidence, and it is not a subtle signal: taxi
/// pickups in New York trough in the small hours and peak in the evening rush.
/// If these were true UTC instants the curve would be shifted five hours and the
/// trough would land mid-morning.
#[test]
fn the_hours_are_new_york_local_which_is_what_the_curve_says() {
    let playground = loaded();
    let answer = ok(
        &playground,
        "SELECT hour(pickup_time), count(*) FROM trips GROUP BY hour(pickup_time)",
    );
    let by_hour = counts(&answer);

    let quietest = by_hour.iter().min_by_key(|(_, n)| **n).unwrap().0;
    let busiest = by_hour.iter().max_by_key(|(_, n)| **n).unwrap().0;
    assert_eq!(
        (*quietest, *busiest),
        (4, 18),
        "the quietest and busiest hours are 04:00 and 18:00 local; a five-hour \
         shift would put them at 09:00 and 23:00, which is what these numbers \
         would say if the column really were UTC instants: {by_hour:?}"
    );
    // And the shape between them, so that a fixture whose two extremes
    // happened to land right could not pass: the morning rush outruns the
    // pre-dawn lull several times over.
    assert!(
        by_hour[&8] > by_hour[&4] * 5,
        "08:00 should dwarf 04:00: {by_hour:?}"
    );
}

/// A fixed offset shifts the hours, and shifts them by exactly what it says.
///
/// The composition this rests on is that a zone conversion *is* an addition:
/// `hour(t, '-05:00')` compiles to `Extract(Hour, Add(t, -18000))`, which is
/// why it needed no kernel change and no new wire variant. The test that this
/// is really what happens is that the counts permute rather than change — the
/// same trips, relabelled — which a fresh calculation could get wrong in a way
/// a spot check on one hour would not catch.
#[test]
fn a_fixed_offset_rotates_the_hours_and_keeps_every_trip() {
    let playground = loaded();
    let utc = counts(&ok(
        &playground,
        "SELECT hour(pickup_time), count(*) FROM trips GROUP BY hour(pickup_time)",
    ));
    let shifted = counts(&ok(
        &playground,
        "SELECT hour(pickup_time, '-05:00'), count(*) FROM trips \
         GROUP BY hour(pickup_time, '-05:00')",
    ));

    let rotated: BTreeMap<i64, i64> = utc
        .iter()
        .map(|(hour, n)| ((hour - 5).rem_euclid(24), *n))
        .collect();
    assert_eq!(
        shifted, rotated,
        "-05:00 should rotate the histogram by five"
    );
    let total: i64 = shifted.values().sum();
    assert_eq!(total, 100_000, "no trip may be lost in the shift");
}

/// Half-hour zones work, which is the case an hours-only offset would fail
/// silently: India is +05:30 and Newfoundland is -03:30, and rounding either
/// to the hour gives a plausible histogram that is wrong for half the rows.
#[test]
fn a_half_hour_offset_is_not_rounded_to_the_hour() {
    let playground = loaded();
    let half = counts(&ok(
        &playground,
        "SELECT hour(pickup_time, '+05:30'), count(*) FROM trips \
         GROUP BY hour(pickup_time, '+05:30')",
    ));
    let whole = counts(&ok(
        &playground,
        "SELECT hour(pickup_time, '+05:00'), count(*) FROM trips \
         GROUP BY hour(pickup_time, '+05:00')",
    ));
    assert_ne!(
        half, whole,
        "+05:30 and +05:00 must not produce the same histogram"
    );
    assert_eq!(half.values().sum::<i64>(), 100_000);
}

/// The same call with and without a zone is two computed columns, because they
/// are two questions. If the offset were left out of `ComputeSpec`'s identity,
/// find-or-add would fold them together and the second would silently answer
/// as the first.
#[test]
fn a_zone_makes_it_a_different_computed_column() {
    let playground = loaded();
    let answer = ok(
        &playground,
        "SELECT hour(pickup_time, '-05:00'), count(*) FROM trips \
         GROUP BY hour(pickup_time, '-05:00')",
    );
    let spec = &answer["spec"]["compute"];
    assert_eq!(spec.as_array().unwrap().len(), 1, "{spec}");
    assert_eq!(spec[0]["offset"], json!(-18_000), "{spec}");

    // And the plain call, in the same playground, still has no offset.
    let plain = ok(
        &playground,
        "SELECT hour(pickup_time), count(*) FROM trips GROUP BY hour(pickup_time)",
    );
    assert_eq!(plain["spec"]["compute"][0]["offset"], json!(0));
}

#[test]
fn a_malformed_or_unresolvable_zone_is_refused_with_the_reason() {
    let playground = loaded();
    for (text, wanted) in [
        ("'America/New_York'", "timezone database"),
        ("'-5'", "not a UTC offset"),
        ("'lunchtime'", "not a UTC offset"),
        ("'+99:00'", "not a real offset"),
        ("'-05:99'", "not a real offset"),
    ] {
        let message = refused(
            &playground,
            &format!(
                "SELECT hour(pickup_time, {text}), count(*) FROM trips \
                 GROUP BY hour(pickup_time, {text})"
            ),
        );
        assert!(
            message.contains(wanted),
            "{text} should be refused for {wanted}: {message}"
        );
    }
    // An aggregate is not a time function and does not take a second argument.
    let message = refused(&playground, "SELECT max(fare, '-05:00') FROM trips");
    assert!(message.contains("takes one column"), "{message}");
    // Neither does `round`, which is in `TIME_FUNCTIONS` but is not one.
    let message = refused(
        &playground,
        "SELECT round(distance, '-05:00'), count(*) FROM trips \
         GROUP BY round(distance, '-05:00')",
    );
    assert!(message.contains("takes no timezone"), "{message}");
}

// --- time functions on a join ---------------------------------------------

/// Trips per hour, with the zone's borough — one query, which is the thing
/// that used to need two.
///
/// The oracle is a fold over the decoded file joined to the zone table by
/// hand, so nothing in it goes through the kernel's join, its grouper or the
/// computed-column plumbing.
#[test]
fn an_hour_key_on_a_join_agrees_with_a_hand_rolled_join() {
    let playground = loaded();
    let answer = ok(
        &playground,
        "SELECT hour(pickup_time), count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY hour(pickup_time)",
    );

    // Every trip whose pickup_zone matches a zone row, folded by local hour.
    let zone_ids: std::collections::BTreeSet<u64> = taxi::zone_rows()
        .iter()
        .map(|row| match row.values()[0] {
            slate_tuple::Value::U64(id) => id,
            ref other => panic!("zone id is {other:?}"),
        })
        .collect();
    let mut expected: BTreeMap<i64, i64> = BTreeMap::new();
    for row in taxi::decode(&trip_bytes()).unwrap() {
        let (zone, seconds) = match (&row.values()[1], &row.values()[3]) {
            (slate_tuple::Value::U64(z), slate_tuple::Value::I64(s)) => (*z, *s),
            other => panic!("unexpected {other:?}"),
        };
        if zone_ids.contains(&zone) {
            *expected
                .entry(seconds.div_euclid(3600).rem_euclid(24))
                .or_default() += 1;
        }
    }
    assert_eq!(counts(&answer), expected);
    assert_eq!(answer["columns"][0], json!("hour(pickup_time)"));
}

/// The join's computed column is on the join, not on a side — which is what
/// the spec has to say, because a side's own computed column is refused by the
/// kernel now that it is known to be dropped.
#[test]
fn the_join_spec_puts_the_computed_column_past_both_tables() {
    let playground = loaded();
    let answer = ok(
        &playground,
        "SELECT hour(pickup_time), count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY hour(pickup_time)",
    );
    let spec = &answer["spec"];
    assert_eq!(spec["compute"][0]["function"], json!("hour"));
    // `trips` has 11 columns and `zones` 3, so the first computed column is 14.
    let width = taxi::trips().columns().len() + taxi::zones().columns().len();
    assert_eq!(
        spec["groupBy"],
        json!(width),
        "the group key must sit past both tables: {spec}"
    );
}

/// And a zone offset works there too, which is the composition of the two
/// features in this change and the one place they could have failed to meet.
#[test]
fn a_join_takes_a_timezone_as_well() {
    let playground = loaded();
    let plain = counts(&ok(
        &playground,
        "SELECT hour(pickup_time), count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY hour(pickup_time)",
    ));
    let shifted = counts(&ok(
        &playground,
        "SELECT hour(pickup_time, '-05:00'), count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY hour(pickup_time, '-05:00')",
    ));
    let rotated: BTreeMap<i64, i64> = plain
        .iter()
        .map(|(hour, n)| ((hour - 5).rem_euclid(24), *n))
        .collect();
    assert_eq!(shifted, rotated);
}
