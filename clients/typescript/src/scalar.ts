import { type Column, columnWire } from "./join.js";
import { type Expr, type Ordinal } from "./query.js";
import { type Value, valueToWire } from "./value.js";

/**
 * A value computed from a row, rather than read out of one.
 *
 * A query's computed values are appended after its table's own columns, and a
 * filter, a sort key, a GROUP BY key or an aggregate names one with
 * {@link computedAt} — so none of them has to learn what an expression is. That
 * is the kernel's arrangement; this module only builds the expressions.
 *
 * Two rules the server enforces and this module surfaces rather than
 * duplicates:
 *
 * - A later computed value may read an earlier one; reading forwards is
 *   refused. The values are numbered in the order `compute` declares them, so
 *   `computedAt(0, 0)` inside computed value 1 is the natural thing to write
 *   and the illegal direction is the awkward one.
 * - An input's computed value has no slot in a *joined* row, because the kernel
 *   packs a joined row by declared table width. Naming one across inputs is
 *   refused by the server with that reason. {@link joinComputed} names the
 *   other kind — a value belonging to the join itself, evaluated over the whole
 *   joined row and able to read any input — which does have a slot, past every
 *   input's columns.
 */
export interface Scalar {
  readonly wire: Record<string, unknown>;
}

/** Reads a column of the query's own table. */
export const col = (ordinal: Ordinal): Scalar => ({ wire: { column: { column: ordinal } } });

/**
 * Reads any reference: a column of an input, a computed value, or one of the
 * join's own. See `at`, `computedAt` and `joinComputed`.
 */
export const ref = (c: Column): Scalar => ({ wire: { column: columnWire(c) } });

/**
 * A constant.
 *
 * A {@link Value} rather than a bare number, for the reason `Value` exists: a
 * literal has no declared type and the wire has two integer widths that do not
 * compare equal, so `2` would have to guess which one the column is.
 */
export const lit = (value: Value): Scalar => ({ wire: { literal: valueToWire(value) } });

const pair = (op: string, left: Scalar, right: Scalar): Scalar => ({
  wire: { [op]: { left: left.wire, right: right.wire } },
});

/** `a + b`, on numbers. String concatenation is {@link concat}. */
export const add = (a: Scalar, b: Scalar): Scalar => pair("add", a, b);
/** `a - b`. */
export const sub = (a: Scalar, b: Scalar): Scalar => pair("sub", a, b);
/** `a * b`. */
export const mul = (a: Scalar, b: Scalar): Scalar => pair("mul", a, b);
/** `a / b`. Integer division on two integers, as the kernel's is. */
export const div = (a: Scalar, b: Scalar): Scalar => pair("div", a, b);

/** The length of a string in characters, or of a byte string in bytes. */
export const length = (s: Scalar): Scalar => ({ wire: { length: s.wire } });
/** Lowercases a string. */
export const lower = (s: Scalar): Scalar => ({ wire: { lower: s.wire } });
/** Uppercases a string. */
export const upper = (s: Scalar): Scalar => ({ wire: { upper: s.wire } });

/**
 * Rounds a number to the nearest integer, halves away from zero.
 *
 * Returns an integer, so it can be a group key without the grouping depending
 * on float equality.
 */
export const round = (s: Scalar): Scalar => ({ wire: { round: s.wire } });

/** Joins strings end to end. */
export const concat = (...parts: Scalar[]): Scalar => ({
  wire: { concat: { scalars: parts.map((p) => p.wire) } },
});

/** The first part that is not null. */
export const coalesce = (...parts: Scalar[]): Scalar => ({
  wire: { coalesce: { scalars: parts.map((p) => p.wire) } },
});

/**
 * Which part of a timestamp, for {@link extract} and {@link dateTrunc}.
 *
 * Timestamps are seconds since the epoch in an integer column: there is no date
 * type to be more precise about. Every member here is a fixed number of
 * seconds, which is what lets `dateTrunc` be defined as arithmetic — a month is
 * not, and lives on {@link CalendarPart} instead.
 */
export type TimeUnit = "second" | "minute" | "hour" | "day";

const TIME_UNITS: Record<TimeUnit, string> = {
  second: "TIME_UNIT_SECOND",
  minute: "TIME_UNIT_MINUTE",
  hour: "TIME_UNIT_HOUR",
  day: "TIME_UNIT_DAY",
};

/**
 * A calendar field of a timestamp, which {@link TimeUnit} cannot name.
 *
 * Separate from `TimeUnit` because that one promises a fixed number of seconds
 * and a month has none. A `"month"` member there would give it a length that is
 * a lie, and the lie would be silent.
 *
 * `day-of-week` is 0 for Sunday, matching ClickHouse, MySQL and SQLite rather
 * than ISO. `day-of-month` is named in full because `"day"` on `TimeUnit`
 * counts days since the epoch and the two are both plausible-looking integers.
 */
export type CalendarPart = "year" | "month" | "day-of-month" | "day-of-week";

const CALENDAR_PARTS: Record<CalendarPart, string> = {
  year: "CALENDAR_PART_YEAR",
  month: "CALENDAR_PART_MONTH",
  "day-of-month": "CALENDAR_PART_DAY_OF_MONTH",
  "day-of-week": "CALENDAR_PART_DAY_OF_WEEK",
};

/** One part of a timestamp, as a number. */
export const extract = (unit: TimeUnit, value: Scalar): Scalar => ({
  wire: { extract: { unit: TIME_UNITS[unit], value: value.wire } },
});

/** A timestamp truncated to `unit`. */
export const dateTrunc = (unit: TimeUnit, value: Scalar): Scalar => ({
  wire: { dateTrunc: { unit: TIME_UNITS[unit], value: value.wire } },
});

/**
 * A calendar field of a timestamp: the year, the day of the week.
 *
 * Timestamps are seconds since the epoch in an integer column, read in UTC.
 * There is no timezone here and no date type to carry one; shifting to another
 * fixed offset is `calendarPart(part, add(column, lit(int(3600 * hours))))`,
 * which is what such a conversion is. A region name is not offered, because
 * doing it correctly needs the IANA database and approximating it is wrong for
 * a third of the year.
 */
export const calendarPart = (part: CalendarPart, value: Scalar): Scalar => ({
  wire: { calendarPart: { part: CALENDAR_PARTS[part], value: value.wire } },
});

/** The year. Proleptic Gregorian, negative before 1 CE. */
export const year = (value: Scalar): Scalar => calendarPart("year", value);
/** The month, 1 to 12. */
export const month = (value: Scalar): Scalar => calendarPart("month", value);
/** The day of the month, 1 to 31. */
export const dayOfMonth = (value: Scalar): Scalar => calendarPart("day-of-month", value);
/** The day of the week, 0 for Sunday through 6 for Saturday. */
export const dayOfWeek = (value: Scalar): Scalar => calendarPart("day-of-week", value);

/**
 * A calendar boundary {@link calendarTrunc} can floor a timestamp to.
 *
 * Separate from {@link TimeUnit} for the reason {@link CalendarPart} is: those
 * are all a fixed number of seconds and a month is not, so `dateTrunc` is a
 * division while this decodes the date, drops the fields below the boundary and
 * encodes it again.
 *
 * A day is absent on purpose — it *is* a fixed number of seconds, so
 * `dateTrunc("day", t)` already means it.
 */
export type CalendarUnit = "month" | "year";

const CALENDAR_UNITS: Record<CalendarUnit, string> = {
  month: "CALENDAR_UNIT_MONTH",
  year: "CALENDAR_UNIT_YEAR",
};

/**
 * The first instant of the month or year containing `value`, in UTC.
 *
 * Floors, including below the epoch: an instant in December 1969 truncates to
 * 1969-12-01 rather than forward to 1970-01-01.
 */
export const calendarTrunc = (unit: CalendarUnit, value: Scalar): Scalar => ({
  wire: { calendarTrunc: { unit: CALENDAR_UNITS[unit], value: value.wire } },
});

/** The first instant of the month, in UTC. */
export const monthStart = (value: Scalar): Scalar => calendarTrunc("month", value);
/** The first instant of the year, in UTC. */
export const yearStart = (value: Scalar): Scalar => calendarTrunc("year", value);

/** One `WHEN ... THEN ...` of a {@link caseWhen}. */
export interface CaseBranch {
  readonly when: Expr;
  readonly then: Scalar;
}

/**
 * SQL's `CASE WHEN`.
 *
 * `otherwise` is required, because SQL's `CASE` with no `ELSE` produces null and
 * an explicit null literal says so — where an absent field would be a client
 * that forgot. The wire makes the same demand.
 *
 * Named `caseWhen` rather than `case`, which is a reserved word.
 */
export const caseWhen = (branches: CaseBranch[], otherwise: Scalar): Scalar => ({
  wire: {
    case: {
      branches: branches.map((b) => ({ when: b.when.wire, then: b.then.wire })),
      otherwise: otherwise.wire,
    },
  },
});

/** How to measure the distance between two vectors. */
export type Metric = "l2" | "l2-squared" | "cosine" | "negative-inner-product";

const METRICS: Record<Metric, string> = {
  l2: "METRIC_L2",
  "l2-squared": "METRIC_L2_SQUARED",
  cosine: "METRIC_COSINE",
  "negative-inner-product": "METRIC_NEGATIVE_INNER_PRODUCT",
};

/** The distance between two vectors. */
export const distance = (left: Scalar, right: Scalar, metric: Metric): Scalar => ({
  wire: { distance: { left: left.wire, right: right.wire, metric: METRICS[metric] } },
});

/**
 * Replaces every match of `pattern` in `value`.
 *
 * Capture references may be written `$1` or `\1`; the server accepts both,
 * because every SQL dialect spells it with a backslash and inserting the
 * literal text would be a wrong answer that looks like a right one.
 */
export const regexpReplace = (
  value: Scalar,
  pattern: string,
  replacement: string,
): Scalar => ({
  wire: { regexpReplace: { value: value.wire, pattern, replacement } },
});

/** Renders a list of computed values for a request. */
export const scalarsToWire = (scalars: Scalar[] | undefined): Record<string, unknown>[] =>
  (scalars ?? []).map((s) => s.wire);
