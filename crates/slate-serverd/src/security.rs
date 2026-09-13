//! Turning the `[security]` section into a [`SecurityCatalog`].
//!
//! # What a configured policy is, and what it is not
//!
//! [`Policy`] takes a Rust closure over a [`SecurityContext`], and the
//! repository's argument for that shape is worth restating before narrowing
//! it: a closure removes the parameter-substitution bug class that a template
//! language brings, and matches a design where schemas are code too.
//!
//! A configured policy is a closure as well — it has to be, that is the only
//! thing `Policy` accepts — but the closure is generated: it lowers a parsed
//! predicate against the caller. So a configured policy can say
//! `owner = :principal` and `region IN ('eu', 'uk') AND deleted IS NULL`, and
//! cannot say anything that is not an `Expr` over this row, two typed holes
//! and literals.
//!
//! The substitution objection does not reach the holes. `:principal` is not
//! text spliced into a predicate before parsing; it is a
//! [`Value`](slate_tuple::Value) taken off the principal and dropped into a
//! slot the parser has already decided is a value, after the whole expression
//! has been parsed and type-checked against the table. There is no
//! concatenation anywhere, so there is no string for a hostile id to escape
//! out of. What is lost is expressiveness — a policy that consults anything
//! but this row and this principal — and that is stated plainly in
//! [`crate::config`] rather than worked around.

use crate::config;
use crate::error::{Fault, Started};
use crate::lang::{TableScope, pred};
use slate_kernel::{Action, Grant, Policy, SecurityCatalog};
use slate_schema::{Catalog, TableDef};

/// Build the rules the head node enforces.
///
/// `warnings` collects things that are legal and probably not intended. They
/// are warnings rather than refusals because each has a real use — a node
/// brought up before its grants are written, a table deliberately superuser-
/// only — and a server that refuses to start over one would be a server people
/// work around.
pub(crate) fn catalog(
    security: &config::Security,
    catalog: &Catalog,
    warnings: &mut Vec<String>,
) -> Started<SecurityCatalog> {
    let mut built = SecurityCatalog::new();

    for grant in &security.grants {
        let actions = actions(&grant.actions, &format!("grant to `{}`", grant.role))?;
        for name in &grant.tables {
            let table = table(catalog, name, &format!("grant to `{}`", grant.role))?;
            built = built.grant(Grant::new(&grant.role, table.id(), actions.clone()));
        }
    }

    for spec in &security.policies {
        let place = format!("policy `{}`", spec.name);
        let table = table(catalog, &spec.table, &place)?;
        let actions = actions(&spec.actions, &place)?;

        let parsed = pred::parse(&spec.using, &TableScope::per_caller(table))
            .map_err(|why| Fault::at(format!("{place}, `using`"), why.render(&spec.using)))?;

        // A policy that does not read the caller is legal and sometimes right
        // — `year >= 2000` is a policy — but it is also what a misspelled
        // placeholder looks like, and the parser has already refused an
        // unknown one. Left silent on purpose: warning here would fire on
        // every correct constant policy.
        let mut policy = Policy::new(&spec.name, table.id(), actions, move |context: &_| {
            parsed.lower(context)
        });
        for role in &spec.roles {
            policy = policy.for_role(role);
        }
        built = built.policy(policy);
    }

    for name in &security.rls_enabled {
        let table = table(catalog, name, "`rls_enabled`")?;
        built = built.enable_rls(table.id());
    }

    if security.grants.is_empty() {
        warnings.push(
            "no `[[security.grants]]`: an empty security catalog denies every action to every non-superuser, so this node will refuse every request it authenticates".to_owned(),
        );
    }

    // A policy without a grant is unreachable: the RBAC check runs first and
    // denies before any row filter is built. Worth saying, because the symptom
    // is `PERMISSION_DENIED` on a table whose policy looks perfectly correct.
    for spec in &security.policies {
        let granted = security
            .grants
            .iter()
            .any(|g| g.tables.iter().any(|t| t == &spec.table));
        if !granted {
            warnings.push(format!(
                "policy `{}` is on table `{}`, which no grant mentions; the role check runs first, so the policy can never be reached",
                spec.name, spec.table
            ));
        }
    }

    Ok(built)
}

fn table<'a>(catalog: &'a Catalog, name: &str, place: &str) -> Started<&'a TableDef> {
    catalog.table_by_name(name).ok_or_else(|| {
        Fault::at(
            place.to_owned(),
            format!("`{name}` is not a table in this configuration"),
        )
    })
}

/// Parse an action list. `all` is a spelling of the four, because writing them
/// out is where a copied grant loses `delete` and nobody notices.
fn actions(names: &[String], place: &str) -> Started<Vec<Action>> {
    if names.is_empty() {
        return Err(Fault::at(
            place.to_owned(),
            "`actions` is empty, which grants nothing; write `actions = [\"all\"]` or list them",
        ));
    }
    let mut out = Vec::new();
    for name in names {
        match name.as_str() {
            "all" => out.extend(Action::ALL),
            "read" => out.push(Action::Read),
            "insert" => out.push(Action::Insert),
            "update" => out.push(Action::Update),
            "delete" => out.push(Action::Delete),
            other => {
                return Err(Fault::at(
                    place.to_owned(),
                    format!(
                        "`{other}` is not an action; there are read, insert, update, delete and all"
                    ),
                ));
            }
        }
    }
    Ok(out)
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
    use slate_kernel::{Expr, Principal, SecurityContext};
    use slate_schema::Row;
    use slate_tuple::Value;

    fn fixture(security_text: &str) -> Started<(Catalog, SecurityCatalog, Vec<String>)> {
        let document: config::Document = toml::from_str(&format!(
            r#"
[listen]
address = "127.0.0.1:0"
[storage]
backend = "memory"

[[tables]]
name = "users"
id = 1
columns = [
  {{ name = "tenant_id", type = "u64" }},
  {{ name = "id", type = "u64" }},
  {{ name = "owner", type = "u64" }},
  {{ name = "email", type = "str" }},
]
primary_key = ["tenant_id", "id"]
tenant_column = "tenant_id"

{security_text}
"#
        ))
        .unwrap_or_else(|e| panic!("{e}"));
        let tables = crate::schema::catalog(&document.tables)?;
        let mut warnings = Vec::new();
        let security = catalog(&document.security, &tables, &mut warnings)?;
        Ok((tables, security, warnings))
    }

    fn caller(id: u64, tenant: u64) -> SecurityContext {
        SecurityContext::new(
            Principal::new(Value::U64(id))
                .with_tenant(Value::U64(tenant))
                .with_role("app"),
        )
    }

    #[test]
    fn a_policy_reads_the_caller_and_filters_by_them() {
        let (tables, security, _) = fixture(
            r#"
[[security.grants]]
role = "app"
tables = ["users"]
actions = ["all"]

[[security.policies]]
name = "own_rows"
table = "users"
actions = ["all"]
using = "owner = :principal"
"#,
        )
        .unwrap();
        let users = tables.table_by_name("users").unwrap();

        let mine = Row::new(vec![
            Value::U64(1),
            Value::U64(1),
            Value::U64(7),
            Value::Str("a".into()),
        ]);
        let theirs = Row::new(vec![
            Value::U64(1),
            Value::U64(2),
            Value::U64(8),
            Value::Str("b".into()),
        ]);

        assert!(
            security
                .permits_row(&caller(7, 1), users, Action::Read, &mine)
                .unwrap()
        );
        assert!(
            !security
                .permits_row(&caller(7, 1), users, Action::Read, &theirs)
                .unwrap()
        );
        // The same policy, a different caller: the closure really is a
        // function of the context rather than a constant baked at startup.
        assert!(
            security
                .permits_row(&caller(8, 1), users, Action::Read, &theirs)
                .unwrap()
        );
    }

    #[test]
    fn the_tenant_restriction_survives_a_configured_policy() {
        let (tables, security, _) = fixture(
            r#"
[[security.grants]]
role = "app"
tables = ["users"]
actions = ["all"]

[[security.policies]]
name = "own_rows"
table = "users"
actions = ["all"]
using = "owner = :principal"
"#,
        )
        .unwrap();
        let users = tables.table_by_name("users").unwrap();
        let elsewhere = Row::new(vec![
            Value::U64(2),
            Value::U64(1),
            Value::U64(7),
            Value::Str("a".into()),
        ]);
        assert!(
            !security
                .permits_row(&caller(7, 1), users, Action::Read, &elsewhere)
                .unwrap(),
            "the tenant column is forced by the kernel, not by the policy"
        );
    }

    #[test]
    fn rls_with_no_policy_admits_nothing() {
        let (tables, security, _) = fixture(
            r#"
[[security.grants]]
role = "app"
tables = ["users"]
actions = ["all"]
[security]
rls_enabled = ["users"]
"#,
        )
        .unwrap();
        let users = tables.table_by_name("users").unwrap();
        let filter = security
            .row_filter(&caller(7, 1), users, Action::Read)
            .unwrap();
        let row = Row::new(vec![
            Value::U64(1),
            Value::U64(1),
            Value::U64(7),
            Value::Str("a".into()),
        ]);
        assert!(!filter.admits(&row));
    }

    #[test]
    fn a_role_scoped_policy_only_applies_to_that_role() {
        let (tables, security, _) = fixture(
            r#"
[[security.grants]]
role = "app"
tables = ["users"]
actions = ["all"]

[[security.policies]]
name = "app_only"
table = "users"
actions = ["read"]
roles = ["auditor"]
using = "owner = :principal"
"#,
        )
        .unwrap();
        let users = tables.table_by_name("users").unwrap();
        // RLS is on because a policy exists, and the caller holds no matching
        // role, so no policy applies and nothing is admitted.
        let filter = security
            .row_filter(&caller(7, 1), users, Action::Read)
            .unwrap();
        assert!(
            matches!(
                filter.conjuncts().last(),
                Some(Expr::Or(parts)) if parts.is_empty()
            ) || !filter.admits(&Row::new(vec![
                Value::U64(1),
                Value::U64(1),
                Value::U64(7),
                Value::Str("a".into()),
            ]))
        );
    }

    #[test]
    fn a_grant_on_a_table_that_does_not_exist_is_refused() {
        let error = fixture(
            r#"
[[security.grants]]
role = "app"
tables = ["nowhere"]
actions = ["all"]
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("not a table"), "{error}");
    }

    #[test]
    fn an_unknown_action_lists_the_ones_there_are() {
        let error = fixture(
            r#"
[[security.grants]]
role = "app"
tables = ["users"]
actions = ["select"]
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("read, insert, update, delete"), "{error}");
    }

    #[test]
    fn no_grants_at_all_is_a_warning_naming_the_consequence() {
        let (_, _, warnings) = fixture("").unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("denies every action")),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_policy_with_no_grant_behind_it_is_a_warning() {
        let (_, _, warnings) = fixture(
            r#"
[[security.policies]]
name = "own_rows"
table = "users"
actions = ["all"]
using = "owner = :principal"
"#,
        )
        .unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("can never be reached")),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_policy_predicate_error_names_the_policy() {
        let error = fixture(
            r#"
[[security.policies]]
name = "own_rows"
table = "users"
actions = ["all"]
using = "owner = 'seven'"
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("policy `own_rows`"), "{error}");
        assert!(error.contains("`using`"), "{error}");
    }
}
