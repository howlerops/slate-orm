/**
 * Table declarations, and the fingerprint the server checks them by.
 *
 * Optional. Everything in this package works without one — the wire carries
 * ordinals and this client resolves nothing. What a declaration buys is the
 * *check*: a request carrying one is refused if the server's catalog disagrees,
 * instead of being answered with the wrong column.
 *
 * That matters most for the mistake ordinals make easy. Declaring
 * `{id, title, year}` for a table that is really `{id, year, title}` produces a
 * client that reads titles as years, silently and forever. With a declaration
 * the first request says so.
 */

/** A column's declared type, as the catalog spells it. */
export type ColumnType =
  | "bool"
  | "bytes"
  | "string"
  | "i64"
  | "u64"
  | "f64"
  | "uuid"
  | "vector"
  | "decimal";

/** One column of a [TableDef]. */
export interface ColumnDef {
  readonly name: string;
  readonly type: ColumnType;
  /**
   * Digits after the decimal point, for a `"decimal"` column. Absent and
   * meaningless for every other type.
   *
   * Part of the fingerprint, and the one property in it that addresses no
   * column. A client that has an ordinal wrong reads the wrong column and
   * usually notices; one that has a scale wrong reads the *right* column and
   * renders every value a power of ten out, for ever, with nothing anywhere
   * reporting it — the wire carries units and never the scale, so the
   * fingerprint is the only place this can be caught.
   */
  readonly scale?: number;
}

/** This client's declaration of a table. */
export interface TableDef {
  /** The name, as the catalog spells it. */
  readonly name: string;
  /** Columns in ordinal order. `columns[2]` *is* ordinal 2. */
  readonly columns: ColumnDef[];
  /** The key columns, in key order. */
  readonly primaryKey: string[];
}

/** Declarations by table name. A table with no entry sends no claim. */
export type Schemas = Record<string, TableDef>;

const FNV_OFFSET = 0xcbf2_9ce4_8422_2325n;
const FNV_PRIME = 0x0000_0100_0000_01b3n;
const MASK = (1n << 64n) - 1n;

class Fnv {
  #state = FNV_OFFSET;

  bytes(data: Uint8Array): void {
    for (const byte of data) {
      this.#state ^= BigInt(byte);
      this.#state = (this.#state * FNV_PRIME) & MASK;
    }
  }

  ascii(text: string): void {
    this.bytes(new TextEncoder().encode(text));
  }

  /**
   * A string, length-prefixed rather than delimited, so that no column name
   * can be spelled to look like the end of a field — `a\nb` and two columns
   * must not hash alike.
   *
   * The length is the *byte* length, not the JavaScript string length: a name
   * outside the BMP counts as its UTF-8 bytes on every other client, and a
   * `.length` here would count UTF-16 code units and disagree.
   */
  text(value: string): void {
    const encoded = new TextEncoder().encode(value);
    this.ascii(String(encoded.length));
    this.ascii(":");
    this.bytes(encoded);
  }

  number(value: number): void {
    this.ascii(String(value));
    this.ascii(";");
  }

  get value(): bigint {
    return this.#state;
  }
}

/** The position of a named column, or -1. */
export function ordinalOf(table: TableDef, name: string): number {
  return table.columns.findIndex((column) => column.name === name);
}

/**
 * This client's claim about the table, in the server's canonical form.
 *
 * Only what a client can address and can be wrong about: the table's name, and
 * per ordinal the column's name and declared type, plus the primary key and the
 * column count. Nullability, `DEFAULT`, `CHECK`, foreign keys and indexes
 * address no column and are deliberately absent — hashing them would make an
 * unrelated migration break every client, which is the failure mode that makes
 * a fingerprint worse than none.
 */
export function fingerprint(table: TableDef): bigint {
  const hash = new Fnv();
  hash.ascii("slate.v1.schema/1");
  hash.text(table.name);
  table.columns.forEach((column, ordinal) => {
    hash.number(ordinal);
    hash.text(column.name);
    hash.text(column.type);
    // A decimal's scale, and only a decimal's. It addresses no column -- the
    // test every other excluded property fails -- and is hashed anyway because
    // the failure it prevents is worse: a wrong ordinal reads the wrong column
    // and usually shows, a wrong scale reads the right column and renders
    // every value a power of ten out, for ever, with nothing anywhere
    // reporting it. The wire carries units and never the scale.
    if (column.type === "decimal") {
      hash.number(column.scale ?? 0);
    }
  });
  hash.ascii("key");
  hash.number(table.primaryKey.length);
  for (const name of table.primaryKey) {
    const ordinal = ordinalOf(table, name);
    if (ordinal < 0) {
      // A key naming a column this declaration does not have is a broken
      // declaration. Hashing the miss as ordinal 0 would make it collide with
      // a correct declaration whose key is the first column, so it hashes as
      // a value no real ordinal takes.
      hash.ascii("?");
      continue;
    }
    hash.number(ordinal);
  }
  hash.ascii("columns");
  hash.number(table.columns.length);
  return hash.value;
}

/** The wire form of a claim, or undefined for an undeclared table. */
export function claimFor(
  schemas: Schemas | undefined,
  table: string,
): Record<string, unknown> | undefined {
  const def = schemas?.[table];
  if (!def) return undefined;
  return { columns: def.columns.length, fingerprint: fingerprint(def).toString() };
}

/**
 * One `CHECK` constraint, as the catalog publishes it.
 *
 * Data, not behaviour. Nothing here evaluates a predicate: that would be a
 * second implementation of the server's expression language, and two
 * implementations of one rule disagree. This is what a caller shows a person
 * before they submit, and what maps the `check` key in a refusal's details
 * back to a field.
 */
export interface CheckRule {
  /** The column the rule is about, or null when it is about several. */
  column: string | null;
  /** The sentence to show a person, or null when the schema wrote none. */
  message: string | null;
  /** The text the predicate was parsed from, or null if it was built in Rust. */
  predicate: string | null;
}

/**
 * One foreign key, as the catalog publishes it.
 *
 * Data, like {@link CheckRule}, and for the same reason: nothing here enforces
 * anything, because the server does. What it carries is the one fact a caller
 * cannot derive — which table a `Relation` read as `"parents"` answers with.
 *
 * A `Relation` names a relationship by the child table and the key's name and
 * stops there, deliberately: a client that described the relationship could
 * describe it differently from the next client. But `related` also needs the
 * table its rows decode as, which for `"parents"` is the *parent* and is
 * nowhere in the client. Before this it was a string the caller typed from
 * memory.
 *
 * What the wrong one costs was measured rather than assumed, in Go, by making
 * `Answers` return the child either way and running the three-SDK conformance
 * suite: the server *refuses* it, because `related` sends the named table's
 * declaration and the schema check sees one table's columns claimed for
 * another. That is the good failure, and it holds only while the two
 * declarations differ — two that fingerprint alike would be decoded
 * positionally against each other with nothing said.
 */
export interface ForeignKey {
  /** The key's name, which is what `Relation.through` wants. */
  readonly name: string;
  /** The table holding the key. `Relation.on`, either direction. */
  readonly child: string;
  /** The table it points at. The table a `"parents"` read answers with. */
  readonly parent: string;
  /**
   * `"restrict"` or `"cascade"`, as the catalog spells it.
   *
   * Data only: the server applies it and this client never does.
   */
  readonly onDelete: string;
}

/** The table a read of `key` this way decodes as. */
export function answers(key: ForeignKey, way: "children" | "parents"): string {
  return way === "parents" ? key.parent : key.child;
}
