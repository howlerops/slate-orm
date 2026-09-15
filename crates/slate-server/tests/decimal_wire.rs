//! What happens to a decimal on a wire that has no decimal field.
//!
//! `Value::Decimal` exists in the kernel and the ORM; the gRPC protocol does
//! not carry one yet. That is a gap rather than a bug, but the *behaviour* at
//! the boundary is a decision, and this is what makes it checkable.
//!
//! These are **pins, not endorsements**. When the proto grows the field they
//! should fail, and the right response is to rewrite them for the real
//! encoding rather than to relax them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use slate_server::convert::value_to_proto;
use slate_tuple::Value;

#[test]
fn a_decimal_is_reported_rather_than_dropped() {
    let rendered = value_to_proto(&Value::Decimal(1250));
    let text = format!("{rendered:?}");
    // The `_` arm of `value_to_proto` exists for exactly this: a type the
    // proto cannot carry becomes a visible marker rather than a null, because
    // sending it as a null would be silent data loss and a client would show
    // an empty cell where money used to be.
    assert!(text.contains("unrepresentable"), "{text}");
    assert!(text.contains("decimal"), "{text}");
    assert!(
        !text.contains("NullValue"),
        "a decimal must not reach a client as a null: {text}"
    );
}

#[test]
fn the_types_the_wire_does_carry_are_unaffected() {
    // The comparison that keeps the test above honest: the marker is the
    // fallback for *unknown* types, not something every value now gets.
    let integer = format!("{:?}", value_to_proto(&Value::I64(1250)));
    assert!(integer.contains("Int64Value"), "{integer}");
    assert!(!integer.contains("unrepresentable"), "{integer}");
}
