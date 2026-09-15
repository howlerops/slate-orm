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

// --- truncating to a calendar boundary ------------------------------------

/// The first instant of a month or a year, as `CalendarTrunc` computes it.
fn truncated(seconds: i64, unit: slate_kernel::CalendarUnit) -> i64 {
    let scalar = Scalar::Literal(Value::I64(seconds)).calendar_trunc(unit);
    match scalar.evaluate(&Row::new(vec![])) {
        Value::I64(n) => n,
        other => panic!("trunc {unit:?} of {seconds} gave {other:?}"),
    }
}

/// Hand-checked boundaries, produced with `date -u -d @<seconds>` as the cases
/// above were.
///
/// `date_trunc` to a *fixed* unit is a division and needs no calendar. A month
/// is not a fixed number of seconds — this is the whole reason `CalendarUnit`
/// exists — so truncating to one decodes the date, drops the day and encodes it
/// again, which runs `days_from_civil`: arithmetic transcribed from a reference
/// and therefore exactly the kind of code that is wrong by one in a way no
/// round trip catches.
#[test]
fn truncation_agrees_with_boundaries_worked_out_by_hand() {
    use slate_kernel::CalendarUnit::{Month, Year};

    // 2024-02-29T13:45:00Z, a leap day.
    let leap = 1_709_214_300;
    assert_eq!(
        truncated(leap, Month),
        1_706_745_600,
        "2024-02-01T00:00:00Z"
    );
    assert_eq!(truncated(leap, Year), 1_704_067_200, "2024-01-01T00:00:00Z");

    // The epoch itself is already both boundaries.
    assert_eq!(truncated(0, Month), 0);
    assert_eq!(truncated(0, Year), 0);

    // 1969-12-31T23:59:59Z — one second before the epoch, which is where a
    // truncation that rounds toward zero rather than flooring goes wrong: it
    // would return 1970-01-01 and put a December instant in the next year.
    assert_eq!(truncated(-1, Month), -2_678_400, "1969-12-01T00:00:00Z");
    assert_eq!(truncated(-1, Year), -31_536_000, "1969-01-01T00:00:00Z");

    // 2000-03-01T00:00:00Z, just past the century leap day that the
    // divisible-by-400 rule keeps.
    let after_2000_leap = 951_868_800;
    assert_eq!(truncated(after_2000_leap, Month), after_2000_leap);
    assert_eq!(truncated(after_2000_leap, Year), 946_684_800, "2000-01-01");

    // 1900-02-28, the century that is *not* a leap year.
    let y1900 = -2_203_977_600; // 1900-02-28T00:00:00Z
    assert_eq!(truncated(y1900, Month), -2_206_310_400, "1900-02-01");
    assert_eq!(truncated(y1900, Year), -2_208_988_800, "1900-01-01");
}

/// `days_from_civil` really is the inverse of `civil_from_days`.
///
/// Neither is reachable directly — both are private — so the round trip goes
/// through the two scalars that use them: truncating to a month gives an
/// instant whose year and month read back as the ones the original had, and
/// whose day is the first.
///
/// Every day of a four-century span, which is the period the Gregorian rules
/// repeat over, so this covers every leap-year case there is: the ordinary
/// one, the century that skips it, and the four-hundredth that does not.
#[test]
fn truncating_and_reading_the_date_back_agree_over_four_centuries() {
    use slate_kernel::CalendarUnit::{Month, Year};

    // 1700-01-01 to 2100-01-01, stepping a day. 400 years is the Gregorian
    // cycle; starting before 1970 means half the range is negative, which is
    // where a floor and a truncation differ.
    let start = -8_520_336_000_i64; // 1700-01-01T00:00:00Z
    let day = 86_400;
    let mut at = start;
    let mut months = 0;
    let mut years = 0;
    while at < 4_102_444_800 {
        let (year, month, _, _) = civil(at);

        let month_start = truncated(at, Month);
        let (ty, tm, td, _) = civil(month_start);
        assert_eq!(
            (ty, tm, td),
            (year, month, 1),
            "truncating {at} to its month gave {month_start}"
        );
        assert!(month_start <= at, "a truncation must not move forwards");
        assert!(
            at - month_start < 31 * day,
            "a month is at most 31 days, not {}",
            at - month_start
        );
        if month_start == at {
            months += 1;
        }

        let year_start = truncated(at, Year);
        let (yy, ym, yd, _) = civil(year_start);
        assert_eq!((yy, ym, yd), (year, 1, 1));
        assert!(year_start <= at);
        assert!(at - year_start < 366 * day);
        if year_start == at {
            years += 1;
        }

        at += day;
    }
    // 400 years of months and years, which is what says the loop really
    // covered the span rather than exiting early.
    assert_eq!(months, 400 * 12, "one month boundary per month");
    assert_eq!(years, 400, "one year boundary per year");
}

/// A non-integer timestamp is null rather than a guess, as everywhere else.
#[test]
fn truncating_a_non_timestamp_is_null() {
    use slate_kernel::CalendarUnit::Month;
    let scalar = Scalar::Literal(Value::Str("nope".to_owned())).calendar_trunc(Month);
    assert_eq!(scalar.evaluate(&Row::new(vec![])), Value::Null);
}
