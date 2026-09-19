//! Turning the `[[tables]]` sections into a [`Catalog`].
//!
//! # Two passes, because a name has to become an ordinal
//!
//! A `CHECK`, a partial index's predicate and an index expression all name
//! columns, and the kernel wants [`Ordinal`]s. Resolving a name needs a built
//! [`TableDef`], and building one evaluates the index arguments — which is the
//! cycle [`IndexBuilder::only_where`]'s own documentation warns about, having
//! been hit by a test in that crate.
//!
//! So each table is built twice. The first build declares only columns, the
//! primary key and the tenant column; it is thrown away except as the thing
//! that answers "what ordinal is `size`, and what does it hold". The second
//! adds the indexes, checks and foreign keys, with every name already
//! resolved.
//!
//! The obvious cheaper alternative — take the ordinal to be the column's
//! position in the file — is what the schema layer does internally, and would
//! work today. It was rejected because it is an assumption about another
//! crate's behaviour with no compiler check behind it: the day a column is
//! reordered or synthesised during `build`, every predicate in every
//! configuration file silently names a different column. The extra build is
//! one pass over a handful of columns at startup.

use crate::config;
use crate::error::{Fault, Started};
use crate::lang::{TableScope, pred, scalar};
use crate::value;
use slate_kernel::Expr;
use slate_schema::{
    Catalog, CheckDef, ForeignKeyDef, IndexDef, IndexId, Managed, ReferentialAction, TableDef,
    TableId,
};
use slate_tuple::{Direction, ValueType};
use std::collections::BTreeMap;

/// Build the catalog the head node serves.
pub(crate) fn catalog(tables: &[config::Table]) -> Started<Catalog> {
    if tables.is_empty() {
        return Err(Fault::new(
            "this configuration declares no tables, so the server would answer every request with `unknown table`. Add at least one `[[tables]]`",
        ));
    }

    // Names to ids, for a foreign key's parent. Built before any table is, so
    // a forward reference works and a typo is caught by name rather than
    // showing up as a dangling id much later.
    let mut ids: BTreeMap<&str, TableId> = BTreeMap::new();
    for table in tables {
        if ids.insert(&table.name, TableId(table.id)).is_some() {
            return Err(Fault::new(format!(
                "two tables are called `{}`; a table name is how a client asks for it",
                table.name
            )));
        }
    }
    // Ids are the key prefix, so a duplicate is two tables sharing a keyspace:
    // rows of one decode as rows of the other. `Catalog::from_tables` catches
    // it too; caught here as well because the message can name both tables.
    let mut by_id: BTreeMap<u32, &str> = BTreeMap::new();
    for table in tables {
        if let Some(other) = by_id.insert(table.id, &table.name) {
            return Err(Fault::new(format!(
                "tables `{other}` and `{}` both have id {}; the id is the key prefix, so they would share a keyspace",
                table.name, table.id
            )));
        }
    }

    // Index ids are a *global* keyspace: an index entry's key is
    // `0x02 <index id>` with no table id in it (`slate_kernel::keys`). Two
    // tables sharing an index id therefore share a key range, and a scan of
    // one would walk over the other's entries and decode them as its own rows.
    //
    // The schema layer does not catch this. `TableBuilder::build` rejects a
    // duplicate id *within* a table and `Catalog::from_tables` does not look
    // across them, so the check has to be here — and `clients/python/
    // testserver` gives five of its six tables an index with id 1, which is
    // how this was noticed. Reported upstream rather than fixed here; a
    // configuration file at least cannot express it.
    let mut index_ids: BTreeMap<u32, (&str, &str)> = BTreeMap::new();
    for table in tables {
        for index in &table.indexes {
            if let Some((other_table, other_index)) =
                index_ids.insert(index.id, (&table.name, &index.name))
            {
                return Err(Fault::new(format!(
                    "index `{}` on `{}` and index `{other_index}` on `{other_table}` both have id {}; an index entry's key carries the index id and not the table's, so the two would share a range of the keyspace",
                    index.name, table.name, index.id
                )));
            }
        }
    }

    let mut built = Vec::with_capacity(tables.len());
    for table in tables {
        built.push(
            one(table, &ids).map_err(|fault| fault.within(format!("table `{}`", table.name)))?,
        );
    }

    Catalog::from_tables(built)
        .map_err(|why| Fault::new(format!("the catalog is not valid: {why}")))
}

/// Build one table, both passes.
fn one(table: &config::Table, ids: &BTreeMap<&str, TableId>) -> Started<TableDef> {
    let shape = columns_only(table)?;

    let mut builder = columns_builder(table)?;
    if let Some(version) = table.schema_version {
        builder = builder.schema_version(version);
    }

    for index in &table.indexes {
        builder = builder.index(
            one_index(index, &shape)
                .map_err(|fault| fault.within(format!("index `{}`", index.name)))?,
        );
    }

    for check in &table.checks {
        let expression = constant_predicate(&check.predicate, &shape)
            .map_err(|fault| fault.within(format!("check `{}`, `predicate`", check.name)))?;
        builder = builder.check(CheckDef::new(&check.name, expression));
    }

    for key in &table.foreign_keys {
        let parent = *ids.get(key.parent.as_str()).ok_or_else(|| {
            Fault::at(
                format!("foreign key `{}`", key.name),
                format!(
                    "`parent = \"{}\"` names no table in this configuration",
                    key.parent
                ),
            )
        })?;
        let action = match key.on_delete.as_str() {
            "restrict" => ReferentialAction::Restrict,
            "cascade" => ReferentialAction::Cascade,
            other => {
                return Err(Fault::at(
                    format!("foreign key `{}`", key.name),
                    format!(
                        "`on_delete = \"{other}\"` is not an action; there are restrict and cascade"
                    ),
                ));
            }
        };
        let mut foreign_key = ForeignKeyDef::builder(&key.name, parent).on_delete(action);
        for column in &key.columns {
            foreign_key = foreign_key.column(column);
        }
        builder = builder.foreign_key(foreign_key);
    }

    builder
        .build()
        .map_err(|why| Fault::new(format!("this table is not valid: {why}")))
}

/// Pass one: the table with nothing that names a column by name.
fn columns_only(table: &config::Table) -> Started<TableDef> {
    let mut builder = columns_builder(table)?;
    if let Some(version) = table.schema_version {
        builder = builder.schema_version(version);
    }
    builder.build().map_err(|why| {
        Fault::new(format!(
            "this table's columns and primary key are not valid: {why}"
        ))
    })
}

/// The declaration shared by both passes.
fn columns_builder(table: &config::Table) -> Started<slate_schema::TableBuilder> {
    let mut builder = TableDef::builder(&table.name, TableId(table.id));

    for column in &table.columns {
        let declared = value::value_type(
            &column.value_type,
            &format!("column `{}`'s `type`", column.name),
        )?;
        match (declared, column.scale) {
            (ValueType::Decimal, None) => {
                return Err(Fault::new(format!(
                    "column `{}` is a decimal and has no `scale`; a decimal value is a count of the column's smallest unit, so without a scale nothing can say whether `1250` is 12.50 or 1250",
                    column.name
                )));
            }
            (other, Some(_)) if other != ValueType::Decimal => {
                return Err(Fault::new(format!(
                    "column `{}` has a `scale` and holds {other}, which has no scale; only a decimal column does",
                    column.name
                )));
            }
            _ => {}
        }

        let default = column
            .default
            .as_ref()
            .map(|value| {
                value::from_toml(
                    value,
                    declared,
                    &format!("column `{}`'s `default`", column.name),
                )
            })
            .transpose()?;

        builder = match (column.added_in, column.nullable, default.clone()) {
            (Some(added_in), false, Some(value)) => {
                builder.added_column_with_default(&column.name, declared, added_in, value)
            }
            // `added_column` forces nullability, so a non-nullable column
            // added in a later version and given no default would silently
            // become nullable. Refused instead: a row written before the
            // column existed has no bytes for it and nothing to read back.
            (Some(_), false, None) => {
                return Err(Fault::new(format!(
                    "column `{}` is `added_in` a later version, is not nullable and has no `default`; a row written before it existed carries no value for it, so it must be one or the other",
                    column.name
                )));
            }
            (Some(added_in), true, _) => builder.added_column(&column.name, declared, added_in),
            (None, true, _) => builder.nullable_column(&column.name, declared),
            (None, false, _) => builder.column(&column.name, declared),
        };

        // After the arms above, because a decimal's scale belongs to the
        // column and every one of those builders takes only a type. The
        // `decimal_column` pair is the scale-carrying entry point, and using
        // it would mean duplicating the whole `added_in`/`nullable`/`default`
        // matrix a second time for one type.
        if let Some(scale) = column.scale {
            builder = builder.scale_for(&column.name, scale);
        }

        // Same placement and the same reason as the scale: a managed column is
        // a property of the column, and every builder arm above takes only a
        // type. The schema layer refuses a managed column that is not an
        // `i64`, is nullable, or is in the primary key — all three at
        // `build()`, which is where the key is known.
        if let Some(managed) = column.managed.as_deref() {
            let which = match managed {
                "created_at" => Managed::CreatedAt,
                "updated_at" => Managed::UpdatedAt,
                other => {
                    return Err(Fault::new(format!(
                        "column `{}` has `managed = \"{other}\"`, which is not a thing this \
                         store writes; the two are `created_at` and `updated_at`",
                        column.name
                    )));
                }
            };
            builder = builder.managed_for(&column.name, which);
        }

        // `added_column_with_default` has already applied it; applying it
        // twice is harmless but the second `ColumnChange` would be noise in
        // any future debugging.
        let already_applied = matches!((column.added_in, column.nullable), (Some(_), false));
        if let Some(value) = default
            && !already_applied
        {
            builder = builder.default_for(&column.name, value);
        }

        for previous in &column.previous_names {
            builder = builder.renamed_column(&column.name, previous);
        }
        if let Some(dropped_in) = column.dropped_in {
            builder = builder.drop_column(&column.name, dropped_in);
        }
    }

    builder = builder.primary_key(table.primary_key.iter().map(String::as_str));
    if let Some(column) = &table.soft_delete {
        builder = builder.soft_delete(column);
    }
    if let Some(tenant) = &table.tenant_column {
        builder = builder.tenant_column(tenant);
    }
    Ok(builder)
}

/// Build one index against the resolved table shape.
fn one_index(index: &config::Index, shape: &TableDef) -> Started<slate_schema::IndexBuilder> {
    let mut builder = IndexDef::builder(&index.name, IndexId(index.id));
    if index.unique {
        builder = builder.unique();
    }

    match (&index.expression, index.columns.is_empty()) {
        (Some(_), false) => {
            return Err(Fault::new(
                "an index has either `columns` or `expression`, not both: an expression index keys on exactly the one value it computes",
            ));
        }
        (None, true) => {
            return Err(Fault::new(
                "this index has no `columns` and no `expression`",
            ));
        }
        (Some(source), true) => {
            let produces = index.produces.as_ref().ok_or_else(|| {
                Fault::new(
                    "an `expression` index needs `produces`, the type the expression yields. Nothing here can run the expression without a row, and the decoder needs the type before it has one",
                )
            })?;
            let produces = value::value_type(produces, "`produces`")?;
            let direction = direction(index.direction.as_deref(), "`direction`")?;
            let computed = scalar::parse(source, &TableScope::constant(shape))
                .map_err(|why| Fault::at("`expression`", why.render(source)))?;
            builder = builder.expression_with(computed, produces, direction);
        }
        (None, false) => {
            if index.produces.is_some() || index.direction.is_some() {
                return Err(Fault::new(
                    "`produces` and `direction` describe an `expression` index; a column index takes its direction per column, as `{ column = \"kind\", direction = \"desc\" }`",
                ));
            }
            for column in &index.columns {
                let (name, direction_text) = match column {
                    config::IndexColumn::Named(name) => (name, None),
                    config::IndexColumn::Directed { column, direction } => {
                        (column, Some(direction.as_str()))
                    }
                };
                // Checked here rather than left to `build`, so the message can
                // list the columns that do exist.
                if shape.ordinal_of(name).is_none() {
                    return Err(Fault::new(format!(
                        "there is no column `{name}` on this table; it has {}",
                        pred::list_columns(
                            &shape
                                .columns()
                                .iter()
                                .map(|c| c.name().to_owned())
                                .collect::<Vec<_>>()
                        )
                    )));
                }
                builder = builder.column_with(name, direction(direction_text, "`direction`")?);
            }
        }
    }

    if let Some(source) = &index.predicate {
        let expression =
            constant_predicate(source, shape).map_err(|fault| fault.within("`where`"))?;
        builder = builder.only_where(expression);
    }

    Ok(builder)
}

/// Parse a predicate that has no caller, and refuse one that wants one.
fn constant_predicate(source: &str, shape: &TableDef) -> Started<Expr> {
    let parsed = pred::parse(source, &TableScope::constant(shape))
        .map_err(|why| Fault::new(why.render(source)))?;
    // The scope already refuses `:principal`; this is the assertion that the
    // two agree, and it costs a tree walk at startup.
    if !parsed.is_constant() {
        return Err(Fault::new(
            "this predicate depends on the caller, and it is evaluated with no caller",
        ));
    }
    Ok(parsed.lower(&slate_kernel::SecurityContext::superuser()))
}

fn direction(text: Option<&str>, field: &str) -> Started<Direction> {
    match text {
        None | Some("asc") => Ok(Direction::Asc),
        Some("desc") => Ok(Direction::Desc),
        Some(other) => Err(Fault::new(format!(
            "`{field} = \"{other}\"` is not a direction; there are asc and desc"
        ))),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use slate_schema::{Ordinal, Row};
    use slate_tuple::{Value, ValueType};

    fn tables(toml_text: &str) -> Started<Catalog> {
        let document: config::Document = toml::from_str(&format!(
            "[listen]\naddress = \"127.0.0.1:0\"\n[storage]\nbackend = \"memory\"\n{toml_text}"
        ))
        .unwrap_or_else(|e| panic!("{e}"));
        catalog(&document.tables)
    }

    const DOCS: &str = r#"
[[tables]]
name = "docs"
id = 1
columns = [
  { name = "id", type = "u64" },
  { name = "kind", type = "str" },
  { name = "size", type = "i64" },
  { name = "note", type = "str", nullable = true },
]
primary_key = ["id"]
"#;

    #[test]
    fn a_plain_table_builds() {
        let catalog = tables(DOCS).unwrap();
        let docs = catalog.table_by_name("docs").unwrap();
        assert_eq!(docs.columns().len(), 4);
        assert_eq!(docs.ordinal_of("size"), Some(Ordinal(2)));
    }

    #[test]
    fn a_partial_index_keeps_a_predicate_the_planner_can_read() {
        let catalog = tables(&format!(
            "{DOCS}\n[[tables.indexes]]\nname = \"by_kind\"\nid = 1\ncolumns = [\"kind\"]\nwhere = \"size > 0\"\n"
        ))
        .unwrap();
        let docs = catalog.table_by_name("docs").unwrap();
        let index = docs.index_by_name("by_kind").unwrap();
        let predicate = index.predicate().expect("a partial index has one");
        // The planner reads a partial index's predicate through `as_any` and
        // will not choose the index at all if it cannot. A configured index
        // that were opaque here would be an index nothing ever uses.
        let downcast = predicate
            .as_any()
            .and_then(|any| any.downcast_ref::<Expr>())
            .expect("the predicate must be readable as an `Expr`");
        assert_eq!(
            downcast,
            &Expr::Compare {
                column: Ordinal(2),
                op: slate_kernel::CmpOp::Gt,
                value: Value::I64(0)
            }
        );
        // And it admits the rows it says it does.
        let big = Row::new(vec![
            Value::U64(1),
            Value::Str("a".into()),
            Value::I64(5),
            Value::Null,
        ]);
        let small = Row::new(vec![
            Value::U64(2),
            Value::Str("a".into()),
            Value::I64(0),
            Value::Null,
        ]);
        assert!(index.admits(&big));
        assert!(!index.admits(&small));
    }

    #[test]
    fn an_expression_index_computes_its_key() {
        let catalog = tables(&format!(
            "{DOCS}\n[[tables.indexes]]\nname = \"by_lower_kind\"\nid = 2\nexpression = \"lower(kind)\"\nproduces = \"str\"\n"
        ))
        .unwrap();
        let docs = catalog.table_by_name("docs").unwrap();
        let index = docs.index_by_name("by_lower_kind").unwrap();
        let expression = index.expression().expect("an expression index has one");
        assert_eq!(expression.produces(), ValueType::Str);
        let row = Row::new(vec![
            Value::U64(1),
            Value::Str("Kind-A".into()),
            Value::I64(5),
            Value::Null,
        ]);
        assert_eq!(expression.value(&row), Value::Str("kind-a".into()));
    }

    #[test]
    fn an_expression_index_without_produces_is_refused() {
        let error = tables(&format!(
            "{DOCS}\n[[tables.indexes]]\nname = \"x\"\nid = 2\nexpression = \"lower(kind)\"\n"
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("needs `produces`"), "{error}");
    }

    #[test]
    fn columns_and_expression_together_are_refused() {
        let error = tables(&format!(
            "{DOCS}\n[[tables.indexes]]\nname = \"x\"\nid = 2\ncolumns = [\"kind\"]\nexpression = \"lower(kind)\"\nproduces = \"str\"\n"
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("not both"), "{error}");
    }

    #[test]
    fn a_check_can_be_a_regular_expression() {
        // `docs/validation.md` claimed this was the one hole in the rule
        // language — that format validation was reachable only through `LIKE`
        // — on the strength of `title matches '…'` being refused. `matches` is
        // not the syntax; `~` is, and it was there all along. This test is the
        // withdrawal, and it is here rather than in `lang/pred.rs` because the
        // parser's own test proves the *parse* and the claim was about what a
        // `CHECK` can express.
        let catalog = tables(&format!(
            "{DOCS}\n[[tables.checks]]\nname = \"title_length\"\npredicate = \"kind ~ '^.{{1,8}}$'\"\n"
        ))
        .unwrap();
        let docs = catalog.table_by_name("docs").unwrap();
        let check = docs.checks().first().expect("one check");
        let row = |kind: &str| {
            Row::new(vec![
                Value::U64(1),
                Value::Str(kind.into()),
                Value::I64(0),
                Value::Null,
            ])
        };
        assert!(check.satisfied_by(&row("ok")));
        // The bound LIKE cannot express: too long, not merely empty.
        assert!(!check.satisfied_by(&row("far too long for eight")));
        assert!(!check.satisfied_by(&row("")));
    }

    #[test]
    fn the_four_regex_operators_differ_in_case_and_sense() {
        // Written because a mutation made `~` case-insensitive and every
        // assertion above still passed: the pattern was `^.{1,8}$`, which has
        // no letter in it. A test that cannot tell `~` from `~*` is not
        // testing the operator, only that *some* regex ran.
        let check_for = |predicate: &str| {
            let catalog = tables(&format!(
                "{DOCS}\n[[tables.checks]]\nname = \"c\"\npredicate = \"{predicate}\"\n"
            ))
            .unwrap();
            catalog
                .table_by_name("docs")
                .unwrap()
                .checks()
                .first()
                .expect("one check")
                .clone()
        };
        let row = |kind: &str| {
            Row::new(vec![
                Value::U64(1),
                Value::Str(kind.into()),
                Value::I64(0),
                Value::Null,
            ])
        };

        let sensitive = check_for("kind ~ '^[a-z]+$'");
        assert!(sensitive.satisfied_by(&row("abc")));
        assert!(!sensitive.satisfied_by(&row("ABC")), "`~` is case-sensitive");

        let insensitive = check_for("kind ~* '^[a-z]+$'");
        assert!(insensitive.satisfied_by(&row("abc")));
        assert!(insensitive.satisfied_by(&row("ABC")), "`~*` folds case");

        // The negated pair, which is the half a `LIKE`-only language cannot
        // reach at all: "must not look like this".
        let not_sensitive = check_for("kind !~ '^[a-z]+$'");
        assert!(!not_sensitive.satisfied_by(&row("abc")));
        assert!(not_sensitive.satisfied_by(&row("ABC")));

        let not_insensitive = check_for("kind !~* '^[a-z]+$'");
        assert!(!not_insensitive.satisfied_by(&row("abc")));
        assert!(!not_insensitive.satisfied_by(&row("ABC")));
        assert!(not_insensitive.satisfied_by(&row("123")));
    }

    #[test]
    fn a_check_regular_expression_that_does_not_compile_is_refused() {
        // At query time an uncompilable pattern matches nothing; in a
        // configuration file that is a check that silently passes everything.
        let error = tables(&format!(
            "{DOCS}\n[[tables.checks]]\nname = \"bad\"\npredicate = \"kind ~ '('\"\n"
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("not a valid regular expression"), "{error}");
    }

    #[test]
    fn a_check_is_enforced_by_the_definition_it_produces() {
        let catalog = tables(&format!(
            "{DOCS}\n[[tables.checks]]\nname = \"positive\"\npredicate = \"size >= 0\"\n"
        ))
        .unwrap();
        let docs = catalog.table_by_name("docs").unwrap();
        let check = docs.checks().first().expect("one check");
        let ok = Row::new(vec![
            Value::U64(1),
            Value::Str("a".into()),
            Value::I64(0),
            Value::Null,
        ]);
        let bad = Row::new(vec![
            Value::U64(1),
            Value::Str("a".into()),
            Value::I64(-1),
            Value::Null,
        ]);
        assert!(check.satisfied_by(&ok));
        assert!(!check.satisfied_by(&bad));
    }

    #[test]
    fn a_foreign_key_resolves_its_parent_by_name() {
        let catalog = tables(&format!(
            r#"{DOCS}
[[tables.foreign_keys]]
name = "fk"
parent = "owners"
columns = ["id"]

[[tables]]
name = "owners"
id = 2
columns = [{{ name = "id", type = "u64" }}]
primary_key = ["id"]
"#
        ))
        .unwrap();
        let docs = catalog.table_by_name("docs").unwrap();
        assert_eq!(docs.foreign_keys().len(), 1);
    }

    #[test]
    fn a_foreign_key_to_a_table_that_is_not_here_is_refused() {
        let error = tables(&format!(
            "{DOCS}\n[[tables.foreign_keys]]\nname = \"fk\"\nparent = \"nowhere\"\ncolumns = [\"id\"]\n"
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("names no table"), "{error}");
    }

    #[test]
    fn two_tables_with_one_index_id_are_refused() {
        // Not a stylistic rule: the index keyspace has no table id in it, so
        // this is two tables writing entries into one range.
        let error = tables(&format!(
            "{DOCS}\n[[tables.indexes]]\nname = \"by_kind\"\nid = 1\ncolumns = [\"kind\"]\n\
             [[tables]]\nname = \"other\"\nid = 2\ncolumns = [{{ name = \"id\", type = \"u64\" }}, {{ name = \"tag\", type = \"str\" }}]\nprimary_key = [\"id\"]\n\
             [[tables.indexes]]\nname = \"by_tag\"\nid = 1\ncolumns = [\"tag\"]\n"
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("share a range of the keyspace"), "{error}");
        assert!(
            error.contains("by_kind") && error.contains("by_tag"),
            "{error}"
        );
    }

    #[test]
    fn two_tables_with_one_id_are_refused_by_name() {
        let error = tables(&format!(
            "{DOCS}\n[[tables]]\nname = \"other\"\nid = 1\ncolumns = [{{ name = \"id\", type = \"u64\" }}]\nprimary_key = [\"id\"]\n"
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("share a keyspace"), "{error}");
        assert!(
            error.contains("`docs`") && error.contains("`other`"),
            "{error}"
        );
    }

    #[test]
    fn a_decimal_column_carries_its_scale() {
        let catalog = tables(
            r#"
[[tables]]
name = "prices"
id = 1
columns = [
  { name = "id", type = "u64" },
  { name = "amount", type = "decimal", scale = 2 },
]
primary_key = ["id"]
"#,
        )
        .expect("a decimal column is declarable");
        let table = catalog.table_by_name("prices").expect("prices");
        let amount = &table.columns()[1];
        assert_eq!(amount.value_type(), ValueType::Decimal);
        assert_eq!(amount.scale(), Some(2));
        // And it is `None` for the column beside it, so a caller cannot read a
        // scale off a type that does not have one.
        assert_eq!(table.columns()[0].scale(), None);
    }

    #[test]
    fn a_decimal_column_without_a_scale_is_refused() {
        // Not defaulted to 0. A decimal is a count of the column's smallest
        // unit, so a column that meant scale 2 and got 0 stores every value a
        // hundred times too large and nothing anywhere says so.
        let error = tables(
            r#"
[[tables]]
name = "prices"
id = 1
columns = [
  { name = "id", type = "u64" },
  { name = "amount", type = "decimal" },
]
primary_key = ["id"]
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("has no `scale`"), "{error}");
        assert!(error.contains("12.50"), "{error}");
    }

    #[test]
    fn a_scale_on_a_column_that_is_not_a_decimal_is_refused() {
        // The same mistake from the other side: somebody believes this column
        // holds a fixed-point number. Accepting the scale and ignoring it
        // would confirm the belief.
        let error = tables(
            r#"
[[tables]]
name = "prices"
id = 1
columns = [
  { name = "id", type = "u64" },
  { name = "amount", type = "i64", scale = 2 },
]
primary_key = ["id"]
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("only a decimal column does"), "{error}");
    }

    #[test]
    fn a_decimal_default_is_its_units() {
        // An integer, not `12.50`. TOML would parse a float default as a
        // binary double and round it before this code saw it — the exact loss
        // the type exists to avoid — so the units are what a default names.
        let catalog = tables(
            r#"
[[tables]]
name = "prices"
id = 1
columns = [
  { name = "id", type = "u64" },
  { name = "amount", type = "decimal", scale = 2, default = 1250 },
]
primary_key = ["id"]
"#,
        )
        .expect("a decimal default is declarable");
        let table = catalog.table_by_name("prices").expect("prices");
        assert_eq!(
            table.columns()[1].default_value(),
            Some(&Value::Decimal(1250))
        );
    }

    #[test]
    fn no_tables_at_all_is_refused() {
        let error = catalog(&[]).unwrap_err().to_string();
        assert!(error.contains("declares no tables"), "{error}");
    }

    #[test]
    fn a_column_added_later_must_be_nullable_or_have_a_default() {
        let error = tables(
            r#"
[[tables]]
name = "docs"
id = 1
columns = [
  { name = "id", type = "u64" },
  { name = "region", type = "str", added_in = 2 },
]
primary_key = ["id"]
schema_version = 2
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("must be one or the other"), "{error}");
    }

    #[test]
    fn a_column_added_later_with_a_default_stays_non_nullable() {
        let catalog = tables(
            r#"
[[tables]]
name = "docs"
id = 1
columns = [
  { name = "id", type = "u64" },
  { name = "region", type = "str", added_in = 2, default = "eu" },
]
primary_key = ["id"]
schema_version = 2
"#,
        )
        .unwrap();
        let region = catalog
            .table_by_name("docs")
            .and_then(|t| t.column(Ordinal(1)))
            .unwrap();
        assert!(!region.is_nullable());
        assert_eq!(region.default_value(), Some(&Value::Str("eu".into())));
    }

    #[test]
    fn a_dropped_column_keeps_its_ordinal_and_the_ones_after_it() {
        // The two-pass build has to resolve names against the *same* ordinals
        // the final table has. A dropped column in the middle is where a
        // position-counting shortcut would go wrong.
        let catalog = tables(
            r#"
[[tables]]
name = "docs"
id = 1
columns = [
  { name = "id", type = "u64" },
  { name = "old", type = "str", nullable = true, dropped_in = 2 },
  { name = "size", type = "i64" },
]
primary_key = ["id"]
schema_version = 2
[[tables.checks]]
name = "positive"
predicate = "size >= 0"
"#,
        )
        .unwrap();
        let docs = catalog.table_by_name("docs").unwrap();
        assert_eq!(docs.ordinal_of("size"), Some(Ordinal(2)));
        let bad = Row::new(vec![Value::U64(1), Value::Null, Value::I64(-1)]);
        assert!(!docs.checks()[0].satisfied_by(&bad));
    }

    #[test]
    fn a_renamed_column_still_answers_to_its_old_name() {
        let catalog = tables(
            r#"
[[tables]]
name = "docs"
id = 1
columns = [
  { name = "id", type = "u64" },
  { name = "kind", type = "str", previous_names = ["category"] },
]
primary_key = ["id"]
"#,
        )
        .unwrap();
        let docs = catalog.table_by_name("docs").unwrap();
        assert_eq!(docs.ordinal_of("category"), Some(Ordinal(1)));
    }

    #[test]
    fn a_predicate_error_is_rendered_under_the_expression_with_its_place() {
        let error = tables(&format!(
            "{DOCS}\n[[tables.indexes]]\nname = \"by_kind\"\nid = 1\ncolumns = [\"kind\"]\nwhere = \"size > 'x'\"\n"
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("table `docs`"), "{error}");
        assert!(error.contains("index `by_kind`"), "{error}");
        assert!(error.contains("`where`"), "{error}");
        assert!(error.contains('^'), "{error}");
    }

    #[test]
    fn a_policy_placeholder_in_a_check_is_refused() {
        let error = tables(&format!(
            "{DOCS}\n[[tables.checks]]\nname = \"mine\"\npredicate = \"id = :principal\"\n"
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("security.policies"), "{error}");
    }
}
