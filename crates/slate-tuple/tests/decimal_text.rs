//! Reading and writing a decimal as text, at a scale the schema supplies.
//!
//! The pair exists because the SQL front end has to turn `19.99` into units
//! and the workbench has to turn units back into `19.99`, and both had been
//! doing it with `f64` in between — which is the loss the type was added to
//! prevent, reintroduced at the edges.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use slate_tuple::Value;

/// The written form and the units it stands for, at each scale.
#[test]
fn a_written_number_is_a_count_of_units() {
    let cases: &[(&str, u8, i64)] = &[
        ("19.99", 2, 1999),
        // Short of the scale: padded, not reinterpreted. `19.9` is nineteen
        // ninety, which is the reading anyone writing a price intends.
        ("19.9", 2, 1990),
        ("19", 2, 1900),
        ("0.01", 2, 1),
        (".50", 2, 50),
        ("-19.99", 2, -1999),
        ("+19.99", 2, 1999),
        ("  19.99  ", 2, 1999),
        // Scale 0 is a decimal that happens to count whole things.
        ("1250", 0, 1250),
        ("0.0825", 4, 825),
        ("8.20", 2, 820),
        ("0", 2, 0),
        ("-0.01", 2, -1),
    ];
    for (text, scale, units) in cases {
        assert_eq!(
            Value::decimal_from_str(text, *scale),
            Ok(Value::Decimal(*units)),
            "{text:?} at scale {scale}"
        );
    }
}

/// The multiply-by-a-power-of-ten version this replaces, on the number that
/// exposes it.
///
/// Not a hypothetical: `8.20` is the price of a thing, and
/// `("8.20".parse::<f64>() * 100.0) as i64` is 819 — a cent lost, silently,
/// on a value a person typed. Asserted here rather than described, so that
/// anyone tempted to simplify `decimal_from_str` into a multiply has the
/// counterexample in front of them.
#[test]
fn the_float_shortcut_loses_a_cent() {
    #![allow(clippy::cast_possible_truncation)]
    let via_float = ("8.20".parse::<f64>().unwrap() * 100.0) as i64;
    assert_eq!(via_float, 819, "the shortcut, and why it is not used");
    assert_eq!(
        Value::decimal_from_str("8.20", 2),
        Ok(Value::Decimal(820)),
        "and the version that splits on the point"
    );
}

/// More places than the column has is refused, not rounded — when the extra
/// digits carry something.
///
/// A caller who wrote `19.999` meant three places, and a column at scale 2
/// cannot hold them. Rounding would be a value silently different from the one
/// that was typed, which is the class of thing this type exists to stop.
#[test]
fn more_places_than_the_scale_is_refused() {
    let error = Value::decimal_from_str("19.999", 2).expect_err("three places, scale two");
    assert!(error.contains("3 places"), "{error}");
    assert!(
        error.contains("scale"),
        "and says what to do about it: {error}"
    );
}

/// A *trailing zero* past the scale is not a loss and is not refused.
///
/// `19.830` at scale 2 is exactly 1983 units. The first version refused it on
/// a digit count alone, which is pedantry dressed as rigour: people write
/// prices with trailing zeros, and a rule that refuses a number it can hold
/// exactly teaches readers to distrust the rule.
#[test]
fn a_trailing_zero_past_the_scale_is_not_a_loss() {
    assert_eq!(
        Value::decimal_from_str("19.830", 2),
        Ok(Value::Decimal(1983))
    );
    assert_eq!(
        Value::decimal_from_str("19.8300000", 2),
        Ok(Value::Decimal(1983))
    );
    assert_eq!(Value::decimal_from_str("7.00", 0), Ok(Value::Decimal(7)));
    // And one non-zero digit anywhere in the tail is still a refusal.
    assert!(Value::decimal_from_str("19.8301", 2).is_err());
}

#[test]
fn what_is_not_a_decimal() {
    for text in ["", ".", "-", "abc", "1.2.3", "1,200", "1e3", "0x10", "1 2"] {
        assert!(
            Value::decimal_from_str(text, 2).is_err(),
            "{text:?} parsed as a decimal"
        );
    }
}

/// A magnitude past `i64` is refused rather than wrapped.
///
/// Through `i128` on the way, so that the check is a range check on a number
/// that was read correctly rather than a parse failure that happens to fire.
#[test]
fn a_number_too_large_for_the_column_is_refused() {
    assert!(Value::decimal_from_str("92233720368547758.08", 2).is_err());
    // And the largest one that does fit, to show the boundary is where it is.
    assert_eq!(
        Value::decimal_from_str("92233720368547758.07", 2),
        Ok(Value::Decimal(i64::MAX))
    );
    assert_eq!(
        Value::decimal_from_str("-92233720368547758.08", 2),
        Ok(Value::Decimal(i64::MIN))
    );
}

/// `i64::MIN` has no positive counterpart, so rendering it through a negation
/// is a panic in debug and a wrap in release.
#[test]
fn the_most_negative_value_renders() {
    assert_eq!(
        Value::decimal_to_string(i64::MIN, 2),
        "-92233720368547758.08"
    );
    assert_eq!(Value::decimal_to_string(i64::MIN, 0), i64::MIN.to_string());
}

#[test]
fn a_small_magnitude_keeps_its_leading_zero() {
    assert_eq!(Value::decimal_to_string(1, 2), "0.01");
    assert_eq!(Value::decimal_to_string(-1, 2), "-0.01");
    assert_eq!(Value::decimal_to_string(0, 4), "0.0000");
    assert_eq!(Value::decimal_to_string(50, 2), "0.50");
}

proptest! {
    /// Units → text → units, over the whole `i64` range and every scale a
    /// column may declare.
    ///
    /// The oracle here is the identity: whatever `decimal_to_string` writes,
    /// `decimal_from_str` must read back as the same count. That catches the
    /// cases nobody thinks of — a magnitude shorter than the scale, a negative
    /// number whose written form has a leading zero, `i64::MIN` — which is
    /// exactly what the three hand-written tests above found one at a time.
    #[test]
    fn decimal_text_round_trips(units: i64, scale in 0u8..=18) {
        let text = Value::decimal_to_string(units, scale);
        prop_assert_eq!(
            Value::decimal_from_str(&text, scale),
            Ok(Value::Decimal(units)),
            "{} at scale {}", text, scale
        );
    }

    /// And text → units → text, over written forms rather than counts.
    ///
    /// The other direction is not the same property: it is what says the
    /// rendering is *canonical*, so a value that goes through a form and comes
    /// back does not drift a digit at a time.
    #[test]
    fn a_written_decimal_renders_back_to_itself(
        whole in 0i64..1_000_000_000,
        fraction in 0u32..10_000,
        negative: bool,
        scale in 1u8..=4,
    ) {
        let places = usize::from(scale);
        let fraction = fraction % 10u32.pow(u32::from(scale));
        let text = format!(
            "{}{whole}.{fraction:0>places$}",
            if negative { "-" } else { "" }
        );
        let Ok(Value::Decimal(units)) = Value::decimal_from_str(&text, scale) else {
            panic!("{text} did not parse at scale {scale}");
        };
        // `-0.00` is the one form that does not render back to itself, and it
        // should not: zero has no sign. Everything else must.
        let expected = if units == 0 && negative {
            text.trim_start_matches('-').to_owned()
        } else {
            text
        };
        prop_assert_eq!(Value::decimal_to_string(units, scale), expected);
    }
}
