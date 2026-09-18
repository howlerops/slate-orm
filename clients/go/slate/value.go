// Package slate is a Go client for a slate head node.
//
// The shape mirrors the Python client in clients/python, deliberately: the two
// speak the same protocol, and a difference between them is a bug in one of
// them rather than a dialect. Where Go's idiom differs — errors as values, no
// context manager — this follows Go.
package slate

import (
	"encoding/binary"
	"fmt"
	"strconv"

	pb "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
)

// Value is one column's value.
//
// A closed set, matching the wire's `oneof kind` exactly. `any` would be
// shorter and would move every type error from the call site to the server:
// an `int` that should have been a `uint64` is a different value to this
// server — `Value`'s order is type-first — so guessing is not available.
type Value interface {
	toProto() *pb.Value
}

// Null is the absence of a value, which is not the same as a zero.
type Null struct{}

// Bool is a boolean.
type Bool bool

// Bytes is a byte string.
type Bytes []byte

// String is a UTF-8 string.
type String string

// Int is a signed 64-bit integer.
type Int int64

// Uint is an unsigned 64-bit integer.
type Uint uint64

// Float is a double.
type Float float64

// Units is a count of a decimal column's smallest unit.
//
// The same type the Rust surface has, and for the same reason: the *scale*
// lives in the schema, not in the value. Units(1250) in a column declared
// scale 2 is 12.50, and the identical value in a scale-0 column is 1250.
// Nothing on the wire says which, because the protocol publishes no schema.
//
// So this is not a decimal number type and does no arithmetic. [Units.String]
// takes the scale as an argument, because a value does not have one.
type Units int64

// UUID is a UUID, held as its sixteen bytes.
type UUID [16]byte

// Vector is a dense f32 vector, for embeddings.
type Vector []float32

func (Null) toProto() *pb.Value {
	return &pb.Value{Kind: &pb.Value_NullValue{NullValue: pb.NullValue_NULL_VALUE}}
}
func (v Bool) toProto() *pb.Value {
	return &pb.Value{Kind: &pb.Value_BoolValue{BoolValue: bool(v)}}
}
func (v Bytes) toProto() *pb.Value {
	return &pb.Value{Kind: &pb.Value_BytesValue{BytesValue: []byte(v)}}
}
func (v String) toProto() *pb.Value {
	return &pb.Value{Kind: &pb.Value_StringValue{StringValue: string(v)}}
}
func (v Int) toProto() *pb.Value {
	return &pb.Value{Kind: &pb.Value_Int64Value{Int64Value: int64(v)}}
}
func (v Uint) toProto() *pb.Value {
	return &pb.Value{Kind: &pb.Value_Uint64Value{Uint64Value: uint64(v)}}
}
func (v Float) toProto() *pb.Value {
	return &pb.Value{Kind: &pb.Value_DoubleValue{DoubleValue: float64(v)}}
}
func (v Units) toProto() *pb.Value {
	return &pb.Value{Kind: &pb.Value_DecimalValue{DecimalValue: int64(v)}}
}
func (v UUID) toProto() *pb.Value {
	b := make([]byte, 16)
	copy(b, v[:])
	return &pb.Value{Kind: &pb.Value_UuidValue{UuidValue: b}}
}
func (v Vector) toProto() *pb.Value {
	return &pb.Value{Kind: &pb.Value_VectorValue{VectorValue: &pb.Vector{Elements: []float32(v)}}}
}

// StringWithScale renders the units against a scale, as a decimal string.
//
// Mirrors slate_orm::Units::to_string_with_scale, and the conformance corpus
// compares the two. A negative scale is treated as zero rather than panicking:
// this is a rendering helper, and a caller who got a scale wrong wants a
// number they can see is wrong, not a crash in a log line.
func (v Units) StringWithScale(scale int) string {
	if scale <= 0 {
		return strconv.FormatInt(int64(v), 10)
	}
	divisor := int64(1)
	for range scale {
		divisor *= 10
	}
	sign := ""
	// The magnitude is a uint64 because math.MinInt64's does not fit in an
	// int64 at all. Which side the conversion goes on does not matter —
	// measured, and -uint64(v) and uint64(-v) produce identical bits for every
	// int64, because Go defines signed negation as two's-complement wrapping.
	// So this is not the clever spelling of the pair, it is the readable one,
	// and mutating it to the other is an equivalent mutation.
	magnitude := uint64(v)
	if v < 0 {
		sign = "-"
		magnitude = -uint64(v)
	}
	whole := magnitude / uint64(divisor)
	part := magnitude % uint64(divisor)
	return fmt.Sprintf("%s%d.%0*d", sign, whole, scale, part)
}

// String renders a UUID in the usual hyphenated form.
func (v UUID) String() string {
	return fmt.Sprintf("%x-%x-%x-%x-%x", v[0:4], v[4:6], v[6:8], v[8:10], v[10:16])
}

// valueFromProto decodes one wire value.
//
// An unset `kind` is an error rather than a null: the two mean different
// things — a server that sent nothing is a server this client does not
// understand, and reading it as null would turn that into a wrong answer
// somewhere further away.
func valueFromProto(v *pb.Value) (Value, error) {
	if v == nil {
		return nil, fmt.Errorf("slate: a column carried no value at all")
	}
	switch k := v.Kind.(type) {
	case *pb.Value_NullValue:
		return Null{}, nil
	case *pb.Value_BoolValue:
		return Bool(k.BoolValue), nil
	case *pb.Value_BytesValue:
		return Bytes(k.BytesValue), nil
	case *pb.Value_StringValue:
		return String(k.StringValue), nil
	case *pb.Value_Int64Value:
		return Int(k.Int64Value), nil
	case *pb.Value_Uint64Value:
		return Uint(k.Uint64Value), nil
	case *pb.Value_DoubleValue:
		return Float(k.DoubleValue), nil
	case *pb.Value_DecimalValue:
		return Units(k.DecimalValue), nil
	case *pb.Value_UuidValue:
		if len(k.UuidValue) != 16 {
			return nil, fmt.Errorf(
				"slate: a uuid value carried %d bytes, not 16", len(k.UuidValue))
		}
		var u UUID
		copy(u[:], k.UuidValue)
		return u, nil
	case *pb.Value_VectorValue:
		if k.VectorValue == nil {
			return Vector(nil), nil
		}
		out := make(Vector, len(k.VectorValue.Elements))
		copy(out, k.VectorValue.Elements)
		return out, nil
	default:
		return nil, fmt.Errorf("slate: a column carried a value kind this client does not know")
	}
}

// UUIDFromBytes builds a UUID from exactly sixteen big-endian bytes.
func UUIDFromBytes(b []byte) (UUID, error) {
	var u UUID
	if len(b) != 16 {
		return u, fmt.Errorf("slate: a uuid is 16 bytes, got %d", len(b))
	}
	copy(u[:], b)
	return u, nil
}

// unused, but keeps the binary import honest if a future value kind needs it.
var _ = binary.BigEndian
