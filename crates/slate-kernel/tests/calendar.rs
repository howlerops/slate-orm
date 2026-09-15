//! Calendar fields of a timestamp, against dates worked out by hand.
//!
//! `Scalar::Extract` handles the fixed-length units by division. The calendar
//! ones cannot be: a month is not a number of seconds, and a year is not
//! either. `civil_from_days` does the arithmetic, and arithmetic transcribed
//! from a reference is exactly the kind of code that is subtly wrong in a way
//! no round trip would catch — a `+ 1` in the wrong place shifts every date by
//! a day and every self-consistency check still passes.
//!
//! So these are hand-checked instants: leap days, century boundaries, the
//! epoch itself, and the day before it. Each date below can be verified with
//! `date -u -d @<seconds>`, which is how they were produced.

#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use slate_kernel::{CalendarPart, Scalar};
use slate_schema::Row;
use slate_tuple::Value;

/// A calendar field of a fixed instant.
fn part_of(seconds: i64, part: CalendarPart) -> i64 {
    let scalar = Scalar::Literal(Value::I64(seconds)).calendar_part(part);
    match scalar.evaluate(&Row::new(vec![])) {
        Value::I64(n) => n,
        other => panic!("{part:?} of {seconds} gave {other:?}"),
    }
}

/// Year, month, day and weekday at once, which is how the cases read.
fn civil(seconds: i64) -> (i64, i64, i64, i64) {
    (
        part_of(seconds, CalendarPart::Year),
        part_of(seconds, CalendarPart::Month),
        part_of(seconds, CalendarPart::DayOfMonth),
        part_of(seconds, CalendarPart::DayOfWeek),
    )
}

#[test]
fn it_agrees_with_dates_worked_out_by_hand() {
    // `date -u -d @0` — Thursday, and 4 is Thursday counting Sunday as 0.
    assert_eq!(civil(0), (1970, 1, 1, 4), "the epoch");
    // The last second before it. Truncating division would put this in 1970.
    assert_eq!(civil(-1), (1969, 12, 31, 3), "one second before the epoch");
    // A whole day before, so the division is exact and only the sign matters.
    assert_eq!(
        civil(-86_400),
        (1969, 12, 31, 3),
        "one day before the epoch"
    );
    assert_eq!(
        civil(-86_401),
        (1969, 12, 30, 2),
        "a day and a second before"
    );

    // 2024-01-01, which is where the taxi sample starts. A Monday.
    assert_eq!(civil(1_704_067_200), (2024, 1, 1, 1), "2024-01-01");
    // 2024-01-31 23:59:59, where it ends. A Wednesday.
    assert_eq!(civil(1_706_745_599), (2024, 1, 31, 3), "2024-01-31");

    // Leap day in a year divisible by four.
    assert_eq!(civil(1_709_164_800), (2024, 2, 29, 4), "2024-02-29");
    assert_eq!(civil(1_709_251_200), (2024, 3, 1, 5), "2024-03-01");

    // 2000 is a leap year (divisible by 400) and 1900 is not (by 100 but not
    // 400). Every wrong implementation of this rule gets one of them wrong.
    assert_eq!(civil(951_782_400), (2000, 2, 29, 2), "2000-02-29 exists");
    assert_eq!(civil(-2_203_977_600), (1900, 2, 28, 3), "1900-02-28");
    assert_eq!(
        civil(-2_203_891_200),
        (1900, 3, 1, 4),
        "1900-03-01 follows 02-28, because 1900 is not a leap year"
    );
}

#[test]
fn the_week_advances_one_day_at_a_time_and_wraps() {
    // Independent of the civil arithmetic — a different code path — so it gets
    // its own check: 365 consecutive days from the epoch, each one weekday.
    let mut expected = 4; // Thursday
    for day in 0..365 {
        assert_eq!(
            part_of(day * 86_400, CalendarPart::DayOfWeek),
            expected,
            "day {day} after the epoch"
        );
        expected = (expected + 1) % 7;
    }
    // And backwards, which is where a truncating remainder would go negative.
    let mut expected = 4;
    for day in 0..365 {
        assert_eq!(
            part_of(-day * 86_400, CalendarPart::DayOfWeek),
            expected,
            "day {day} before the epoch"
        );
        expected = (expected + 6) % 7;
    }
}

#[test]
fn every_day_of_a_leap_year_round_trips_through_the_calendar() {
    // An oracle over 2024: walk the year a day at a time, keeping the expected
    // date by hand-rolled increment, and check the scalar agrees. This catches
    // the class the hand-written cases cannot — a month length that is right
    // at both ends and wrong in the middle.
    let lengths = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let (mut month, mut day) = (1usize, 1i64);
    for offset in 0..366 {
        let seconds = 1_704_067_200 + offset * 86_400;
        assert_eq!(civil(seconds).0, 2024, "day {offset} of 2024 left the year");
        assert_eq!(
            (civil(seconds).1, civil(seconds).2),
            (month as i64, day),
            "day {offset} of 2024"
        );
        day += 1;
        if day > lengths[month - 1] {
            day = 1;
            month += 1;
        }
    }
    assert_eq!(month, 13, "the walk did not consume the year");
}

#[test]
fn a_non_integer_timestamp_is_null_rather_than_a_guess() {
    // Same rule the rest of `Scalar` follows: a value of the wrong shape
    // produces null, not an error and not a coercion.
    let text = Scalar::Literal(Value::Str("2024-01-01".to_owned()))
        .calendar_part(CalendarPart::Year)
        .evaluate(&Row::new(vec![]));
    assert_eq!(text, Value::Null, "a string is not a timestamp");

    let missing = Scalar::Literal(Value::Null)
        .calendar_part(CalendarPart::DayOfWeek)
        .evaluate(&Row::new(vec![]));
    assert_eq!(missing, Value::Null);
}
