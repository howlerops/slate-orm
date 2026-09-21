//! `[[views]]`, resolved at startup and held beside the catalog.
//!
//! # What a view is here
//!
//! A name bound to a `QuerySpec` over one base table, and nothing else. See
//! `docs/views.md`, which settled the security shape before any of this
//! existed; the three sentences that matter to this file:
//!
//! - **§1** a view must not be a `TableDef`, because `row_filter_with` keys
//!   row-level security on a `TableId` and a view's own id has no policy;
//! - **§2** a view is not a privilege boundary — a caller needs the grant on
//!   the base table, and having it can read past the view;
//! - **§3a** a view kept out of the `Catalog` is refused by every path that
//!   resolves a table name, without a check, because there is nothing to find.
//!
//! §3a is why this module exists and the resolution lives nowhere near
//! `Catalog`. It is also why declaring views is safe before any read path
//! knows about them: right now every use of one is refused.
//!
//! # Why the subset is so small
//!
//! A view may carry a `WHERE` and nothing else. Everything a `SELECT` can
//! otherwise say is refused at load, by name, with a reason — and the
//! refusals are the design rather than an unfinished edge:
//!
//! - **A projection** would make the caller's ordinals *view* ordinals, and
//!   remapping them onto base ordinals is its own piece of work with its own
//!   way to be silently wrong: a column shifted by one returns the wrong data
//!   under the right header. Row-narrowing without column-narrowing is still
//!   what the use case asks for.
//! - **A sort, a limit or an offset** belong to a query, not to a name for
//!   one. `SELECT * FROM recent LIMIT 10` would then mean two limits, and the
//!   composition rule for those is not obvious enough to guess at.
//! - **A join, a chain, an aggregate or a window** each change what a row *is*,
//!   so a caller's filter over "the view" would be a filter over something
//!   with no base table to authorise against. That is §1 again, from the far
//!   side.
//!
//! Every one of these is a thing to add deliberately later. None is a thing to
//! half-support now.

use crate::config;
use crate::error::{Fault, Started};
use slate_schema::TableDef;
use slate_sql::QuerySpec;
use slate_sql::sql::{Schema, Statement, parse};
use std::collections::BTreeMap;

/// One resolved view: the base table's name, and the predicate to `AND` in.
#[derive(Debug, Clone)]
pub(crate) struct View {
    /// The table the view reads. A read path resolves *this* against the
    /// catalog, so authorisation and RLS see the base table — §1's whole
    /// point.
    pub(crate) table: String,
    /// The view's own predicate, already resolved against the catalog and so
    /// carrying ordinals rather than names.
    pub(crate) spec: QuerySpec,
}

/// Every view this node serves, by name.
///
/// A `BTreeMap` and not a `Catalog`: see the module docs. Nothing here is
/// reachable from `Catalog::table_by_name`, which is what refuses a view on
/// every path that has not opted in.
pub(crate) type Views = BTreeMap<String, View>;

/// Resolve `[[views]]` against the tables the catalog was built from.
///
/// Resolution happens **at startup**, not per query. A column a view names
/// that does not exist is a failure to boot with a byte offset into the SQL,
/// rather than a refusal the first time somebody reads through it — which is
/// the same reason indexes and checks are validated here.
pub(crate) fn views(declared: &[config::View], tables: &[TableDef]) -> Started<Views> {
    let mut out = Views::new();
    for view in declared {
        if view.name.trim().is_empty() {
            return Err(Fault::new("a view needs a name"));
        }
        // A view that shadows a table would be unreachable — every resolver
        // looks in the catalog first — so the name would silently do nothing.
        if tables.iter().any(|t| t.name() == view.name) {
            return Err(Fault::new(format!(
                "view `{}` has the same name as a table, and a table wins every \
                 lookup — the view would never be reached",
                view.name
            )));
        }
        if out.contains_key(&view.name) {
            return Err(Fault::new(format!("two views are named `{}`", view.name)));
        }

        let parsed = parse(&view.query, &Schema(tables)).map_err(|why| {
            Fault::new(format!(
                "view `{}` is not a statement this server can parse: {} (at byte {})",
                view.name, why.message, why.at
            ))
        })?;
        let spec = match parsed.statement {
            Statement::Select(spec) => spec,
            _ => {
                return Err(Fault::new(format!(
                    "view `{}` reads more than one table. A view here is a name for a \
                     `WHERE` over one table: a join or a chain changes what a row is, \
                     and there would be no base table to authorise the caller against. \
                     See `docs/views.md`",
                    view.name
                )));
            }
        };
        if let Some(why) = unsupported(&spec) {
            return Err(Fault::new(format!("view `{}` {why}", view.name)));
        }

        out.insert(
            view.name.clone(),
            View {
                table: spec.table.clone(),
                spec,
            },
        );
    }
    Ok(out)
}

/// The reason a spec is more than a view may be, or `None`.
///
/// Two layers, and the second is the one that matters. The named list below
/// exists for its *messages*: a refusal that says which clause is the problem
/// and why is the difference between a config an operator can fix and one they
/// bisect. But a hand-written list of fields is exactly the thing that goes
/// stale the day somebody adds a field to `QuerySpec`, in another crate, with
/// no reason to look here — and going stale here means quietly *accepting* a
/// clause nobody has thought about, in the one place whose whole job is to
/// refuse.
///
/// So the list does not decide. [`beyond_a_where`] does, by serialising the
/// spec and refusing any key that is not `table`, `filter` or `filters`. That
/// works because every other field on `QuerySpec` carries a
/// `skip_serializing_if` for its default — they were written that way for the
/// workbench's spec panel, which is a happy accident this leans on — so a
/// field that is set is a key that appears, including a field added next year.
/// The named list runs first only so the common cases get the better sentence.
fn unsupported(spec: &QuerySpec) -> Option<String> {
    let named = if !spec.columns.is_empty() {
        Some(
            "names its columns. A view here may narrow rows and not columns: the \
             caller's ordinals would become the view's, and remapping them onto the \
             base table's is a separate piece of work with its own way to be \
             silently wrong",
        )
    } else if !spec.sort.is_empty() {
        Some("sorts. A sort belongs to a query rather than to a name for one")
    } else if spec.limit.is_some() {
        Some(
            "has a LIMIT. A caller's own limit would then be the second one, and how \
             two compose is not obvious enough to guess at",
        )
    } else if spec.offset != 0 {
        Some("has an OFFSET, for the reason a LIMIT is refused")
    } else if !spec.having.is_empty() {
        Some("has a HAVING, which only a grouped read can have")
    } else if !spec.compute.is_empty() {
        Some(
            "computes a value. A computed column is not on the base table, so a \
             caller filtering on it would be filtering on something the base table's \
             policy has never seen",
        )
    } else if !spec.aggregates.is_empty() || !spec.group_by.is_empty() {
        Some(
            "groups or aggregates, which changes what a row is — there would be no \
             base row for a policy to admit",
        )
    } else if !spec.window.is_empty() {
        Some("computes a window, for the reason an aggregate is refused")
    } else {
        None
    };
    if let Some(why) = named {
        return Some(why.to_owned());
    }
    beyond_a_where(spec)
}

/// Every clause a view may carry, as the key it serialises to.
///
/// `filter` and `filters` are both here because the SQL front end may lower a
/// `WHERE` onto either, and which one is not this module's business.
const A_VIEW_MAY_SET: [&str; 3] = ["table", "filter", "filters"];

/// Refuse a spec that sets anything outside [`A_VIEW_MAY_SET`], by key name.
///
/// The backstop for [`unsupported`]'s named list. Its message is worse than a
/// named one on purpose: reaching it means a clause exists that nobody wrote a
/// sentence for, and "I do not know what this is, so no" is the only honest
/// thing to say about it. A serialisation that fails is refused too — a spec
/// this cannot inspect is a spec it cannot clear.
fn beyond_a_where(spec: &QuerySpec) -> Option<String> {
    let value = match serde_json::to_value(spec) {
        Ok(serde_json::Value::Object(fields)) => fields,
        _ => {
            return Some(
                "could not be inspected for clauses a view may not carry, so it is \
                 refused rather than assumed harmless"
                    .to_owned(),
            );
        }
    };
    let extra: Vec<&str> = value
        .keys()
        .map(String::as_str)
        .filter(|key| !A_VIEW_MAY_SET.contains(key))
        .collect();
    if extra.is_empty() {
        return None;
    }
    Some(format!(
        "sets `{}`, and a view here is a name for a WHERE over one table and \
         nothing else. See `docs/views.md`",
        extra.join("`, `")
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use slate_sql::{ComputeSpec, FilterSpec, SortSpec, WindowSpec};

    /// A spec carrying only the clauses a view may carry.
    fn allowed() -> QuerySpec {
        QuerySpec {
            table: "docs".to_owned(),
            ..QuerySpec::default()
        }
    }

    #[test]
    fn a_where_over_one_table_is_all_a_view_needs() {
        assert_eq!(unsupported(&allowed()), None);
    }

    /// `compute` and `window` are unreachable through the SQL front end — an
    /// expression in the select list is also a projection, and the projection
    /// refuses first — so the only way to exercise these two branches is to
    /// build the spec. `tests/refusals.rs` says the same thing from the SQL
    /// side and points here.
    #[test]
    fn the_branches_sql_cannot_reach_still_refuse() {
        let mut computed = allowed();
        computed.compute.push(ComputeSpec {
            function: "hour".to_owned(),
            input: 0,
            column: 0,
            offset: 0,
            zone: String::new(),
        });
        assert!(unsupported(&computed).unwrap().contains("computes a value"));

        let mut windowed = allowed();
        windowed.window.push(WindowSpec::default());
        assert!(unsupported(&windowed).unwrap().contains("window"));
    }

    /// The guarantee the named list cannot give.
    ///
    /// [`beyond_a_where`] is what stops a field added to `QuerySpec` later
    /// from being silently accepted, and the named list runs first, so nothing
    /// in the list exercises it. Calling it directly on a clause the list
    /// already knows about is the closest thing to a field that does not exist
    /// yet: if this stops naming the key, a future field is accepted in
    /// silence.
    #[test]
    fn the_backstop_refuses_a_clause_by_key_without_being_told_about_it() {
        let mut sorted = allowed();
        sorted.sort.push(SortSpec {
            column: 0,
            descending: false,
        });
        let why = beyond_a_where(&sorted).unwrap();
        assert!(why.contains("sort"), "{why}");

        // And it is not refusing everything: the allowed keys pass.
        assert_eq!(beyond_a_where(&allowed()), None);
        let mut filtered = allowed();
        filtered.filters.push(FilterSpec::default());
        assert_eq!(beyond_a_where(&filtered), None);
    }

    /// [`A_VIEW_MAY_SET`] must name the keys a full view actually serialises
    /// to — not the keys somebody believed it did.
    ///
    /// A rename in `slate-sql`'s serde attributes would otherwise make this
    /// list stale in the *other* direction from the one the backstop guards:
    /// not a clause silently accepted, but a legal view silently refused,
    /// because the key it now serialises to is not in the allowed set. Built
    /// by setting every clause a view may carry and comparing the key set
    /// rather than by restating the list, which would prove nothing.
    #[test]
    fn the_allowed_keys_are_the_keys_a_full_view_serialises_to() {
        let mut full = allowed();
        full.filter = Some(FilterSpec::default());
        full.filters.push(FilterSpec::default());

        let Ok(serde_json::Value::Object(fields)) = serde_json::to_value(&full) else {
            panic!("a spec should serialise to an object");
        };
        let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected: Vec<&str> = A_VIEW_MAY_SET.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected);

        // And the named list agrees with the backstop about this one.
        assert_eq!(unsupported(&full), None);
    }
}
