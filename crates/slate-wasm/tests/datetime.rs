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
        "month_start",
        "year_start",
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

    // A computed column on a join reads *either* side now, so `borough` — a
    // `zones` column — resolves rather than failing to. It is still refused,
    // and the refusal is better: it names the real problem, which is that a
    // string is not a timestamp.
    //
    // This assertion used to be `is not one`, from "reads a column of `trips`
    // (the left side); `borough` is not one". That was a scope error standing
    // in for a type error, and the scope was a limitation of the spec rather
    // than of the kernel — `Join::compute` has always been evaluated over the
    // joined row.
    let message = refused(
        &playground,
        "SELECT hour(borough), count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY hour(borough)",
    );
    assert!(message.contains("needs a timestamp"), "{message}");
    assert!(message.contains("Str"), "{message}");

    // A name on neither side is still a scope error, and says both tables
    // rather than one — which is what made the old message misleading.
    let message = refused(
        &playground,
        "SELECT hour(nonesuch), count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY hour(nonesuch)",
    );
    assert!(message.contains("trips"), "{message}");
    assert!(message.contains("zones"), "{message}");

    // Selecting a bare column beside a GROUP BY that is not it: SQL's "column
    // must appear in the GROUP BY clause". The check now compares the two
    // ordinals rather than refusing every bare column, so this is the half that
    // must still be refused — and a mutation that stopped checking survived
    // until this case existed, because every other test selected either the key
    // itself or only aggregates.
    let message = refused(
        &playground,
        "SELECT borough, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY zone",
    );
    assert!(message.contains("is not the group key"), "{message}");

    // And a name on neither table, which is a scope error rather than a
    // grouping one and must not be reported as the latter.
    let message = refused(
        &playground,
        "SELECT nonesuch, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY borough",
    );
    assert!(message.contains("is not the group key"), "{message}");

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
        // A name-shaped argument is a zone that does not exist, and the
        // refusal has to name the ones that do — `America/New_York` *is* one
        // now, so the message a misspelling gets is a list and not a
        // statement that the feature is missing.
        ("'america/new_york'", "no such timezone"),
        ("'America/Nowhere'", "America/New_York"),
        ("'lunchtime'", "no such timezone"),
        // Offset-shaped arguments keep the offset syntax message: `-5` is a
        // typo for `-05:00` and telling its author about IANA names helps
        // nobody.
        ("'-5'", "not a UTC offset"),
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

/// **Group by the hour and average the fare, over `trips JOIN zones`.**
///
/// The previous entry recorded this as not expressible, and named the reason:
/// "a join's computed column reads the left table and its aggregates read the
/// right". Both halves were true of `JoinSpec` and neither was true of the
/// kernel — `Join::compute` is evaluated over the joined row, and a grouping's
/// aggregates are ordinals in the joined space like any other. The spec
/// resolved a computed column's name against the left table only and shifted
/// every aggregate past every left column unconditionally, so a query whose key
/// *and* aggregate both live on `trips` had nowhere to land. The workbench
/// example filtered on the right side and counted instead.
///
/// The oracle is a fold over the decoded file joined to the zone table by hand,
/// which never goes near the kernel's join, its grouper or its scalars.
#[test]
fn the_hour_and_the_average_fare_can_come_from_the_same_table() {
    let playground = loaded();
    let answer = ok(
        &playground,
        "SELECT hour(pickup_time), avg(fare) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY hour(pickup_time)",
    );

    // Both positions name `trips`, which is the case that was refused.
    let spec = &answer["spec"];
    assert_eq!(spec["compute"][0]["input"], json!(0), "{spec}");
    assert_eq!(spec["aggregates"][0]["input"], json!(0), "{spec}");
    assert_eq!(spec["aggregates"][0]["kind"], json!("avg"), "{spec}");

    let zone_ids: std::collections::BTreeSet<u64> = taxi::zone_rows()
        .iter()
        .map(|row| match row.values()[0] {
            slate_tuple::Value::U64(id) => id,
            ref other => panic!("zone id is {other:?}"),
        })
        .collect();
    let mut sums: BTreeMap<i64, (f64, i64)> = BTreeMap::new();
    for row in taxi::decode(&trip_bytes()).unwrap() {
        let (zone, seconds, fare) = match (&row.values()[1], &row.values()[3], &row.values()[7]) {
            (
                slate_tuple::Value::U64(z),
                slate_tuple::Value::I64(s),
                slate_tuple::Value::F64(f),
            ) => (*z, *s, *f),
            other => panic!("unexpected {other:?}"),
        };
        if zone_ids.contains(&zone) {
            let entry = sums
                .entry(seconds.div_euclid(3600).rem_euclid(24))
                .or_insert((0.0, 0));
            entry.0 += fare;
            entry.1 += 1;
        }
    }
    assert_eq!(sums.len(), 24, "every hour of the day should appear");

    let rows = answer["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 24);
    for row in rows {
        let hour: i64 = row[0].as_str().unwrap().parse().unwrap();
        let got: f64 = row[1].as_str().unwrap().parse().unwrap();
        let (sum, n) = sums[&hour];
        let want = sum / n as f64;
        // Floating point, summed in a different order by each side, so this
        // compares within a tolerance rather than exactly — and the tolerance
        // is tight enough that a wrong column would not fit inside it: the
        // fares in this sample run from about 3 to 250.
        assert!(
            (got - want).abs() < 1e-6,
            "hour {hour}: the kernel says {got}, the fold says {want}"
        );
    }
}

/// An aggregate over the **right** table still works, which is what the spec
/// could do before and must not have lost.
#[test]
fn an_aggregate_may_still_read_the_right_table() {
    let playground = loaded();
    let answer = ok(
        &playground,
        "SELECT hour(pickup_time), count(distinct borough) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY hour(pickup_time)",
    );
    let spec = &answer["spec"];
    assert_eq!(spec["aggregates"][0]["input"], json!(1), "{spec}");
    // Every hour of the sample touches every borough that has trips in it, so
    // the count is the same across the hours and is not the zone count.
    let rows = answer["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 24);
    for row in rows {
        let boroughs: i64 = row[1].as_str().unwrap().parse().unwrap();
        assert!(
            (1..=8).contains(&boroughs),
            "New York has a handful of boroughs, not {boroughs}"
        );
    }
}

/// And a computed column reading the **right** table, which is the other half.
///
/// `zones.id` is an integer, so `round()` applies to it — a contrived query,
/// and the only right-side numeric column the fixture has. What it proves is
/// that the ordinal is shifted past every `trips` column rather than read at
/// its own: `zones.id` is column 0 of `zones` and column 11 of the joined row,
/// and reading it unshifted would give `trips.id` — 100,000 distinct values
/// rather than a few hundred.
#[test]
fn a_computed_column_may_read_the_right_table() {
    let playground = loaded();
    let answer = ok(
        &playground,
        "SELECT round(zones.id), count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY round(zones.id)",
    );
    let spec = &answer["spec"];
    assert_eq!(spec["compute"][0]["input"], json!(1), "{spec}");
    assert_eq!(spec["compute"][0]["column"], json!(0), "{spec}");

    let rows = answer["rows"].as_array().unwrap();
    // One group per zone that any trip picks up in. Far fewer than the 100,000
    // groups `trips.id` would have produced, which is the failure this pins.
    assert!(
        (2..1000).contains(&rows.len()),
        "got {} groups; trips.id would give 100,000",
        rows.len()
    );
    let total: i64 = rows
        .iter()
        .map(|row| row[1].as_str().unwrap().parse::<i64>().unwrap())
        .sum();
    assert!(
        total > 90_000,
        "the groups should cover nearly every trip, not {total}"
    );
}

/// Grouping by a bare column of the **right** table.
///
/// The plainest joined-space case and the one a mutation found missing: every
/// other test here groups by a computed column, whose ordinal is past both
/// tables and so is shifted by construction. A bare right column is shifted by
/// the left table's *width*, and getting that wrong is a wrong answer rather
/// than an error — `borough` is column 1 of `zones`, so an unshifted ordinal 1
/// reads `trips.pickup_zone`: 260-odd numeric groups where the query asked for
/// a handful of named boroughs.
#[test]
fn a_group_key_may_be_a_bare_column_of_the_right_table() {
    let playground = loaded();
    let answer = ok(
        &playground,
        "SELECT borough, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY borough",
    );
    // `trips` has 11 columns, so `zones.borough` is joined ordinal 11 + 1.
    let expected = taxi::trips().columns().len() + 1;
    assert_eq!(
        answer["spec"]["groupBy"],
        json!(expected),
        "{}",
        answer["spec"]
    );

    let rows = answer["rows"].as_array().unwrap();
    assert!(
        (2..=8).contains(&rows.len()),
        "New York has a handful of boroughs, not {} groups",
        rows.len()
    );
    // The keys are borough *names*, not zone ids — which is what an unshifted
    // ordinal would have produced.
    for row in rows {
        let key = row[0].as_str().unwrap();
        assert!(
            key.parse::<i64>().is_err(),
            "the group key should be a borough name, got {key}"
        );
    }
    let total: i64 = rows
        .iter()
        .map(|row| row[1].as_str().unwrap().parse::<i64>().unwrap())
        .sum();
    assert!(
        total > 90_000,
        "the boroughs should cover nearly every trip"
    );
}

/// And a qualified name on either side, which is how a caller disambiguates.
#[test]
fn a_qualified_name_picks_its_own_side() {
    let playground = loaded();
    let left = ok(
        &playground,
        "SELECT trips.id, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY trips.id",
    );
    assert_eq!(left["spec"]["groupBy"], json!(0), "{}", left["spec"]);

    let right = ok(
        &playground,
        "SELECT zones.id, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY zones.id",
    );
    let expected = taxi::trips().columns().len();
    assert_eq!(
        right["spec"]["groupBy"],
        json!(expected),
        "{}",
        right["spec"]
    );
    // Both tables have an `id`, so this is the case where an unqualified name
    // would have to guess — and where guessing wrong is 100,000 groups instead
    // of a few hundred.
    assert!(
        left["rows"].as_array().unwrap().len() > right["rows"].as_array().unwrap().len(),
        "trips.id has far more distinct values than zones.id"
    );
}

// --- truncating to a month, which `month()` cannot order by ---------------

/// `month_start()` groups by the calendar month and orders as the months do.
///
/// `month()` returns 1 to 12, so ordering by it puts every January of every
/// year together — which is the right answer to a different question and the
/// reason `date_trunc` needed a calendar boundary rather than another
/// `TimeUnit`. The sample is one month, so this checks the *shape*: one group,
/// at the first instant of January 2024, holding every trip.
#[test]
fn a_month_boundary_is_one_group_at_the_first_instant_of_the_month() {
    let playground = loaded();
    let answer = ok(
        &playground,
        "SELECT month_start(pickup_time), count(*) FROM trips \
         GROUP BY month_start(pickup_time)",
    );
    let rows = answer["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "the sample is one calendar month: {rows:?}");
    let key: i64 = rows[0][0].as_str().unwrap().parse().unwrap();
    // 2024-01-01T00:00:00Z. `date -u -d @1704067200` says so.
    assert_eq!(key, 1_704_067_200);
    let count: i64 = rows[0][1].as_str().unwrap().parse().unwrap();
    assert_eq!(count, 100_000, "every trip is in January");

    // And the year boundary is the same instant here, since January is the
    // first month — which is a coincidence of this sample, not a property, so
    // the next test uses dates that distinguish them.
    let by_year = ok(
        &playground,
        "SELECT year_start(pickup_time), count(*) FROM trips \
         GROUP BY year_start(pickup_time)",
    );
    let year_rows = by_year["rows"].as_array().unwrap();
    assert_eq!(year_rows.len(), 1);
    assert_eq!(
        year_rows[0][0].as_str().unwrap().parse::<i64>().unwrap(),
        1_704_067_200
    );
}

/// The two boundaries differ, on rows written for the purpose.
///
/// The taxi sample is a single month, so it cannot tell `month_start` from
/// `year_start` — both give one group at the same instant. These rows span
/// three months of two years, where the two disagree about how many groups
/// there are and about where each one starts.
#[test]
fn a_month_boundary_and_a_year_boundary_disagree_where_they_should() {
    let playground = loaded();
    // 2023-02-14, 2023-02-20, 2023-11-05, 2024-03-09 — four instants in three
    // months of two years. Each verified with `date -u -d @<seconds>`.
    //
    // Ids at 900_001 and up, so `WHERE id >= 900001` selects exactly these:
    // the sample's ids run 1 to 100,000. The SQL subset has no `OR`, so a
    // range over `pickup_time` could not have picked out instants on both
    // sides of January 2024 in one query.
    for (id, at) in [
        (900_001, 1_676_332_800_i64),
        (900_002, 1_676_851_200),
        (900_003, 1_699_142_400),
        (900_004, 1_709_942_400),
    ] {
        let sql = format!(
            "INSERT INTO trips VALUES ({id}, 1, 1, {at}, 600, 1, 1.0, 5.0, 0.0, 5.0, 'cash')"
        );
        ok(&playground, &sql);
    }

    let months = ok(
        &playground,
        "SELECT month_start(pickup_time), count(*) FROM trips \
         WHERE id >= 900001 GROUP BY month_start(pickup_time)",
    );
    let month_keys: Vec<i64> = months["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r[0].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(
        month_keys,
        vec![1_675_209_600, 1_698_796_800, 1_709_251_200],
        "2023-02-01, 2023-11-01, 2024-03-01"
    );

    let years = ok(
        &playground,
        "SELECT year_start(pickup_time), count(*) FROM trips \
         WHERE id >= 900001 GROUP BY year_start(pickup_time)",
    );
    let year_rows = years["rows"].as_array().unwrap();
    let year_keys: Vec<i64> = year_rows
        .iter()
        .map(|r| r[0].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(
        year_keys,
        vec![1_672_531_200, 1_704_067_200],
        "2023-01-01 and 2024-01-01: three months collapse to two years"
    );
    // The February rows land in 2023 together, which is the collapse.
    let counts: Vec<i64> = year_rows
        .iter()
        .map(|r| r[1].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(counts, vec![3, 1]);
}

// --- named zones ----------------------------------------------------------

/// A named zone and the fixed offset it was in agree, over a month it did not
/// move.
///
/// The differential that matters: `'-05:00'` is arithmetic on a constant and
/// `'America/New_York'` is a binary search through a transition table, two
/// entirely separate paths through the binding and the kernel. January 2024 is
/// wholly Eastern Standard Time — the United States springs forward on the
/// second Sunday in March — so over this sample the two must produce the same
/// 24 groups with the same counts. They are compared against the independent
/// fold as well, so a bug common to both still fails.
#[test]
fn a_named_zone_agrees_with_the_fixed_offset_it_was_in() {
    let playground = loaded();
    let mut expected: BTreeMap<i64, i64> = BTreeMap::new();
    for seconds in pickup_times() {
        *expected
            .entry((seconds - 5 * 3600).div_euclid(3600).rem_euclid(24))
            .or_default() += 1;
    }

    let named = ok(
        &playground,
        "SELECT hour(pickup_time, 'America/New_York'), count(*) FROM trips \
         GROUP BY hour(pickup_time, 'America/New_York')",
    );
    let fixed = ok(
        &playground,
        "SELECT hour(pickup_time, '-05:00'), count(*) FROM trips \
         GROUP BY hour(pickup_time, '-05:00')",
    );
    assert_eq!(counts(&named), expected, "New York in January is UTC-5");
    assert_eq!(counts(&fixed), expected, "the fixed offset agrees");

    // And the spec says which mechanism each used, so a future change that
    // quietly lowered the name to a constant would fail here rather than pass
    // the comparison above by cheating.
    assert_eq!(
        named["spec"]["compute"][0]["zone"],
        json!("America/New_York")
    );
    assert_eq!(named["spec"]["compute"][0]["offset"], json!(0));
    assert_eq!(fixed["spec"]["compute"][0]["offset"], json!(-18_000));
    assert!(
        fixed["spec"]["compute"][0]["zone"].is_null(),
        "a fixed offset carries no zone name: {}",
        fixed["spec"]
    );
}

/// A zone that *was* in daylight saving shifts by a different amount.
///
/// The sample is one January, so the sample alone cannot tell a table lookup
/// from a constant −5. Sydney can: it is UTC+11 in January, eleven hours the
/// other way, and a lookup that returned New York's answer or a fixed zero
/// fails. Compared against a fold rather than against another query, so this
/// stands on its own.
#[test]
fn a_southern_zone_shifts_the_other_way_and_by_its_own_amount() {
    let playground = loaded();
    let mut expected: BTreeMap<i64, i64> = BTreeMap::new();
    for seconds in pickup_times() {
        *expected
            .entry((seconds + 11 * 3600).div_euclid(3600).rem_euclid(24))
            .or_default() += 1;
    }
    let answer = ok(
        &playground,
        "SELECT hour(pickup_time, 'Australia/Sydney'), count(*) FROM trips \
         GROUP BY hour(pickup_time, 'Australia/Sydney')",
    );
    assert_eq!(
        counts(&answer),
        expected,
        "Sydney is on daylight time in January, at UTC+11"
    );
}

/// Three spellings of the same call are three computed columns, and the header
/// says which is which.
///
/// Without the second argument in the header they all print `hour(pickup_time)`
/// and the reader has three identical columns with three different answers.
#[test]
fn the_zone_is_part_of_the_column_and_of_its_header() {
    let playground = loaded();
    let answer = ok(
        &playground,
        "SELECT hour(pickup_time), hour(pickup_time, '-05:00'), \
         hour(pickup_time, 'America/New_York') FROM trips LIMIT 1",
    );
    // The header lists every column of the table and then the computed ones,
    // and a projection nulls the cells it did not ask for rather than dropping
    // them — so the three calls are the last three of each.
    let columns = answer["columns"].as_array().unwrap();
    assert_eq!(
        &columns[columns.len() - 3..],
        [
            json!("hour(pickup_time)"),
            json!("hour(pickup_time, '-05:00')"),
            json!("hour(pickup_time, 'America/New_York')")
        ],
        "{answer}"
    );
    assert_eq!(
        answer["spec"]["compute"].as_array().unwrap().len(),
        3,
        "three calls, three computed columns: {}",
        answer["spec"]
    );
    // The first differs from the other two by five hours; the second and third
    // agree, because January is standard time.
    let row = &answer["rows"].as_array().unwrap()[0];
    let first = columns.len() - 3;
    let hour = |at: usize| row[first + at].as_str().unwrap().parse::<i64>().unwrap();
    assert_eq!(hour(1), hour(2), "{row}");
    assert_eq!((hour(0) - 5).rem_euclid(24), hour(1), "{row}");
}

// --- ordering a grouped join, and saying when a name is ambiguous ---------

/// `ORDER BY` on a grouped join orders the *groups*, and by an aggregate.
///
/// `JoinSpec` had no sort at all, so this was refused with "ORDER BY is not
/// supported on a join yet" — true of both shapes and explanatory of neither.
/// A grouped join's groups are `[key, aggregates...]` and the kernel's
/// `Grouping::sort` has always been able to order them; the spec simply could
/// not say so.
///
/// Ordering by `count(*)` descending rather than by the key, because the key
/// order is what comes back anyway: an ORDER BY that agrees with the default
/// passes whether or not it was lowered.
#[test]
fn a_grouped_join_can_be_ordered_by_its_aggregate() {
    let playground = loaded();
    let by_count = ok(
        &playground,
        "SELECT borough, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id \
         GROUP BY borough ORDER BY count(*) DESC",
    );
    let counts: Vec<i64> = by_count["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[1].as_str().unwrap().parse().unwrap())
        .collect();
    assert!(counts.len() > 1, "the sample covers several boroughs");
    assert!(
        counts.windows(2).all(|pair| pair[0] >= pair[1]),
        "descending by count: {counts:?}"
    );
    // The spec carries it, so this is a lowering rather than a coincidence of
    // how the groups happened to come back.
    assert_eq!(
        by_count["spec"]["sort"],
        json!([{"column": 1, "descending": true}]),
        "{}",
        by_count["spec"]
    );

    // Ascending is the same groups the other way round, which rules out an
    // ordering that ignores the direction.
    let ascending = ok(
        &playground,
        "SELECT borough, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id \
         GROUP BY borough ORDER BY count(*) ASC",
    );
    let up: Vec<i64> = ascending["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[1].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(up, counts.iter().rev().copied().collect::<Vec<_>>());

    // And by the key, which is `column: 0` — the group's first slot, not the
    // joined row's first column.
    let by_key = ok(
        &playground,
        "SELECT borough, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id \
         GROUP BY borough ORDER BY borough DESC",
    );
    assert_eq!(
        by_key["spec"]["sort"],
        json!([{"column": 0, "descending": true}])
    );
    let boroughs: Vec<String> = by_key["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[0].as_str().unwrap().to_owned())
        .collect();
    assert!(
        boroughs.windows(2).all(|pair| pair[0] >= pair[1]),
        "descending by borough: {boroughs:?}"
    );
}

/// A computed group key can be ordered too, and only what the groups carry can.
#[test]
fn ordering_a_grouped_join_names_the_key_or_an_aggregate() {
    let playground = loaded();
    let ordered = ok(
        &playground,
        "SELECT hour(pickup_time), count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id \
         GROUP BY hour(pickup_time) ORDER BY hour(pickup_time) DESC",
    );
    let hours: Vec<i64> = ordered["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[0].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(hours, (0..24).rev().collect::<Vec<_>>());

    // An ungrouped join has no sort to lower onto, and the refusal says which
    // shape the reader is in rather than "not supported yet".
    let message = refused(
        &playground,
        "SELECT * FROM trips JOIN zones ON trips.pickup_zone = zones.id \
         ORDER BY trips.id",
    );
    assert!(message.contains("needs a GROUP BY"), "{message}");
    assert!(
        message.contains("orders groups, not joined rows"),
        "{message}"
    );

    // An aggregate the select list does not compute is not in the groups.
    let message = refused(
        &playground,
        "SELECT borough, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id \
         GROUP BY borough ORDER BY max(fare) DESC",
    );
    assert!(message.contains("does not compute"), "{message}");

    // Neither is a column that is not the group key.
    let message = refused(
        &playground,
        "SELECT borough, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id \
         GROUP BY borough ORDER BY zone",
    );
    assert!(
        message.contains("names the group key or one of"),
        "{message}"
    );
}

/// An unqualified name both of a join's tables have is a warning, not silence.
///
/// `trips` and `zones` both have `id`, and the resolution rule is left-first —
/// so `SELECT id ... FROM trips JOIN zones` answers about `trips.id`. That is
/// a defensible rule and was documented only in the parser's source. The query
/// still runs; there is now a line on screen saying which side it chose.
#[test]
fn an_ambiguous_name_on_a_join_warns_and_still_answers() {
    let playground = loaded();
    let answer = ok(
        &playground,
        "SELECT id, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY id",
    );
    let warnings = answer["warnings"].as_array().expect("a warnings array");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let warning = warnings[0].as_str().unwrap();
    assert!(warning.contains("`id` is a column of both"), "{warning}");
    assert!(warning.contains("trips.id"), "{warning}");
    assert!(warning.contains("Qualify it"), "{warning}");
    // It really did group by `trips.id`, which is unique, so there is one
    // group per matched trip rather than one per zone.
    let groups = answer["rows"].as_array().unwrap().len();
    assert!(
        groups > 300,
        "grouping by trips.id gives many groups: {groups}"
    );

    // Said once per name, not once per mention: `id` appears twice above.
    // And a name only one side has says nothing at all.
    let unambiguous = ok(
        &playground,
        "SELECT borough, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY borough",
    );
    assert!(
        unambiguous["warnings"].is_null() || unambiguous["warnings"].as_array().unwrap().is_empty(),
        "{}",
        unambiguous["warnings"]
    );

    // Qualifying it silences the warning, which is the whole point of the
    // advice the warning gives.
    let qualified = ok(
        &playground,
        "SELECT zones.id, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id GROUP BY zones.id",
    );
    assert!(
        qualified["warnings"].is_null() || qualified["warnings"].as_array().unwrap().is_empty(),
        "{}",
        qualified["warnings"]
    );
    // And it grouped by the *zone*, so far fewer groups than by trip.
    let zones = qualified["rows"].as_array().unwrap().len();
    assert!(zones < groups, "{zones} zones against {groups} trips");
}

/// `LIMIT` and `OFFSET` on a grouped join cut the **groups**, not the rows.
///
/// `OFFSET` was not expressible on a join at all — `JoinSpec` had no field for
/// it, though `Join` and `Grouping` both do — so this is the first test of
/// either window on a grouped join.
///
/// It also withdraws a hypothesis. `join.limit` used to be set whether or not
/// the read was grouped, which looked like the mistake the single-table path is
/// careful to avoid: a window over the rows going *into* a grouping answers a
/// different question. Putting that "bug" back changed nothing, because
/// `narrowed_join` in the kernel clears a grouped join's `limit` and `offset`
/// and says why. The test is kept: it pins the behaviour that makes the
/// binding's arrangement harmless, and nothing else asserted it from here.
#[test]
fn a_grouped_joins_limit_and_offset_cut_the_groups() {
    let playground = loaded();
    let all = ok(
        &playground,
        "SELECT borough, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id \
         GROUP BY borough ORDER BY count(*) DESC",
    );
    let rows = |answer: &Json| -> Vec<(String, i64)> {
        answer["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row[0].as_str().unwrap().to_owned(),
                    row[1].as_str().unwrap().parse().unwrap(),
                )
            })
            .collect()
    };
    let every = rows(&all);
    assert!(every.len() >= 4, "the sample covers several boroughs");
    // Every group still counts the whole sample between them, which is what
    // says the limit below is cutting groups rather than rows.
    assert_eq!(
        every.iter().map(|(_, n)| n).sum::<i64>(),
        100_000,
        "{every:?}"
    );

    let first_two = ok(
        &playground,
        "SELECT borough, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id \
         GROUP BY borough ORDER BY count(*) DESC LIMIT 2",
    );
    assert_eq!(rows(&first_two), every[..2].to_vec());

    let skipped = ok(
        &playground,
        "SELECT borough, count(*) FROM trips JOIN zones \
         ON trips.pickup_zone = zones.id \
         GROUP BY borough ORDER BY count(*) DESC LIMIT 2 OFFSET 1",
    );
    assert_eq!(rows(&skipped), every[1..3].to_vec());
    assert_eq!(skipped["spec"]["offset"], json!(1));

    // And on an *ungrouped* join they cut rows, which is the only thing they
    // could mean there.
    let three = ok(
        &playground,
        "SELECT * FROM trips JOIN zones ON trips.pickup_zone = zones.id LIMIT 3",
    );
    assert_eq!(three["rows"].as_array().unwrap().len(), 3);
    let past = ok(
        &playground,
        "SELECT * FROM trips JOIN zones ON trips.pickup_zone = zones.id \
         LIMIT 3 OFFSET 2",
    );
    let three = three["rows"].as_array().unwrap();
    let past = past["rows"].as_array().unwrap();
    assert_eq!(past.len(), 3);
    assert_eq!(&past[..1], &three[2..3], "an offset drops the leading rows");
}
