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
const FNV_OFFSET = 0xcbf29ce484222325n;
const FNV_PRIME = 0x00000100000001b3n;
const MASK = (1n << 64n) - 1n;
class Fnv {
    #state = FNV_OFFSET;
    bytes(data) {
        for (const byte of data) {
            this.#state ^= BigInt(byte);
            this.#state = (this.#state * FNV_PRIME) & MASK;
        }
    }
    ascii(text) {
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
    text(value) {
        const encoded = new TextEncoder().encode(value);
        this.ascii(String(encoded.length));
        this.ascii(":");
        this.bytes(encoded);
    }
    number(value) {
        this.ascii(String(value));
        this.ascii(";");
    }
    get value() {
        return this.#state;
    }
}
/** The position of a named column, or -1. */
export function ordinalOf(table, name) {
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
export function fingerprint(table) {
    const hash = new Fnv();
    hash.ascii("slate.v1.schema/1");
    hash.text(table.name);
    table.columns.forEach((column, ordinal) => {
        hash.number(ordinal);
        hash.text(column.name);
        hash.text(column.type);
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
export function claimFor(schemas, table) {
    const def = schemas?.[table];
    if (!def)
        return undefined;
    return { columns: def.columns.length, fingerprint: fingerprint(def).toString() };
}
