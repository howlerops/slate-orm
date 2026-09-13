//! Deciding who is asking.
//!
//! The kernel enforces security given a [`SecurityContext`]. It cannot decide
//! whose context to use, and neither can a wire protocol: an identity in a
//! request body is not an identity, it is a request to be trusted. So the
//! `.proto` has no principal field anywhere, and building one is this module's
//! job — which means it is a deployment's job, because only a deployment knows
//! what its transport has already proved.
//!
//! # Why this is a trait with no default
//!
//! [`Head`](crate::Head) takes an [`Authenticator`] as a required argument.
//! There is no default, because every default is wrong: a permissive one is a
//! hole, and a rejecting one is a server that appears broken until somebody
//! disables the check to make it work. Making it a required argument means the
//! decision is made once, visibly, where the server is constructed.
//!
//! # Superuser is not reachable from here
//!
//! [`SecurityContext::superuser`] bypasses every check, and the kernel's own
//! docs point out that its value is being one grep away. Nothing in this crate
//! calls it, and no authenticator shipped here can produce one. A deployment
//! that genuinely needs an administrative door writes its own `Authenticator`,
//! and then the grep finds it in their code rather than losing it in ours.

use slate_kernel::security::{Principal, SecurityContext};
use slate_tuple::Value;
use std::fmt;
use tonic::metadata::MetadataMap;
use tonic::{Code, Status};
use uuid::Uuid;

/// Turns a request's transport metadata into the identity it runs under.
pub trait Authenticator: fmt::Debug + Send + Sync + 'static {
    /// Who is asking, or why the request cannot be served.
    ///
    /// Return `UNAUTHENTICATED` when the caller has not established who they
    /// are, and `PERMISSION_DENIED` when they have and it is not enough.
    fn authenticate(&self, metadata: &MetadataMap) -> Result<SecurityContext, Status>;
}

/// The metadata key carrying the principal's id.
pub const PRINCIPAL_KEY: &str = "slate-principal";
/// The metadata key carrying the principal's tenant.
pub const TENANT_KEY: &str = "slate-tenant";
/// The metadata key carrying the principal's roles, comma-separated.
pub const ROLES_KEY: &str = "slate-roles";

/// Takes the caller's identity from request metadata, unverified.
///
/// **This trusts whatever the client sends.** It is correct in exactly one
/// arrangement: behind a proxy or mesh that authenticates the caller, sets
/// these three headers itself, and strips any copies the client supplied. In
/// any other arrangement it is an open door, which is why the only constructor
/// says so in its name.
///
/// The one part of that arrangement this type can check, it does: a proxy that
/// *appends* its headers rather than replacing them leaves two copies of each,
/// and a request carrying a repeated key is refused rather than resolved. That
/// misconfiguration — `add_header` where `proxy_set_header` was meant — used to
/// hand the caller whatever identity they asked for, because the client's copy
/// arrives first. Everything else about the arrangement remains the
/// deployment's to get right; this only closes the failure that looks like it
/// is working.
///
/// It is shipped rather than left as an exercise because that arrangement is
/// the common one — the sidecar has already done the work, and a head node
/// re-authenticating would be inventing a second identity system — and because
/// the tests need something to authenticate with.
#[derive(Debug, Clone, Copy)]
pub struct MetadataIdentity {
    require_tenant: bool,
}

impl MetadataIdentity {
    /// Trust the caller's own headers.
    ///
    /// Named at length on purpose: this appears in the source of every
    /// deployment that uses it, and it should be uncomfortable to read there
    /// unless a proxy really is setting the headers.
    #[must_use]
    pub const fn trusting_the_caller_completely() -> Self {
        Self {
            require_tenant: false,
        }
    }

    /// Refuse a request that names no tenant.
    ///
    /// The kernel already refuses a tenant-scoped *table* to a context with no
    /// tenant. This refuses the request outright, which a deployment where
    /// every table is tenant-scoped will want: the error then names the
    /// missing header rather than a table the caller has never heard of.
    #[must_use]
    pub const fn requiring_a_tenant(mut self) -> Self {
        self.require_tenant = true;
        self
    }
}

impl Authenticator for MetadataIdentity {
    fn authenticate(&self, metadata: &MetadataMap) -> Result<SecurityContext, Status> {
        let Some(id) = text(metadata, PRINCIPAL_KEY)? else {
            return Err(Status::new(
                Code::Unauthenticated,
                format!("no `{PRINCIPAL_KEY}` in the request metadata"),
            ));
        };
        let id = parse_value(&id)
            .map_err(|why| Status::new(Code::Unauthenticated, format!("{PRINCIPAL_KEY}: {why}")))?;

        let mut principal = Principal::new(id);

        match text(metadata, TENANT_KEY)? {
            Some(tenant) => {
                let tenant = parse_value(&tenant).map_err(|why| {
                    Status::new(Code::Unauthenticated, format!("{TENANT_KEY}: {why}"))
                })?;
                principal = principal.with_tenant(tenant);
            }
            None if self.require_tenant => {
                return Err(Status::new(
                    Code::Unauthenticated,
                    format!("no `{TENANT_KEY}` in the request metadata"),
                ));
            }
            None => {}
        }

        if let Some(roles) = text(metadata, ROLES_KEY)? {
            for role in roles.split(',').map(str::trim).filter(|r| !r.is_empty()) {
                principal = principal.with_role(role);
            }
        }

        Ok(SecurityContext::new(principal))
    }
}

fn text(metadata: &MetadataMap, key: &str) -> Result<Option<String>, Status> {
    // A repeated key is refused rather than resolved, because there is no safe
    // way to pick one: taking the first trusts a proxy that replaces, taking
    // the last trusts one that appends, and the server cannot tell which it is
    // behind. `get` takes the first, which is the client's copy exactly when
    // the proxy appends — `add_header` written where `proxy_set_header` was
    // meant — so the one arrangement `MetadataIdentity` is documented as safe
    // in is the one where the old behaviour handed the caller someone else's
    // identity. Refusing costs a misconfigured deployment a clear error and an
    // attacker the whole door.
    let mut values = metadata.get_all(key).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(Status::new(
            Code::Unauthenticated,
            format!(
                "`{key}` appears more than once in the request metadata; the proxy in front of \
                 this server must replace these headers rather than append to them"
            ),
        ));
    }
    value.to_str().map(|s| Some(s.to_owned())).map_err(|_| {
        Status::new(
            Code::Unauthenticated,
            format!("`{key}` is not valid ASCII metadata"),
        )
    })
}

/// Parse a `<type>:<text>` identity value.
///
/// The tag is required rather than inferred. A principal id of `7` could be
/// [`Value::U64`] or [`Value::Str`], and [`Value`]'s order is type-first, so
/// the two are different principals that compare unequal to each other and to
/// whatever is stored in the rows. Guessing would make a policy match or not
/// match depending on whether an id happened to look numeric — which for a
/// user id is precisely the case that changes over time.
fn parse_value(text: &str) -> Result<Value, String> {
    let Some((tag, rest)) = text.split_once(':') else {
        return Err(format!(
            "`{text}` has no type tag; write one of str:, i64:, u64:, bool: or uuid: in front of it"
        ));
    };
    match tag {
        "str" => Ok(Value::Str(rest.to_owned())),
        "i64" => rest
            .parse::<i64>()
            .map(Value::I64)
            .map_err(|_| format!("`{rest}` is not an i64")),
        "u64" => rest
            .parse::<u64>()
            .map(Value::U64)
            .map_err(|_| format!("`{rest}` is not a u64")),
        "bool" => rest
            .parse::<bool>()
            .map(Value::Bool)
            .map_err(|_| format!("`{rest}` is not a bool")),
        "uuid" => Uuid::parse_str(rest)
            .map(Value::Uuid)
            .map_err(|_| format!("`{rest}` is not a uuid")),
        other => Err(format!("`{other}:` is not a type tag this server knows")),
    }
}

/// Refuses every request.
///
/// For a deployment that has not wired up authentication yet, and wants the
/// server to say so rather than to be open while somebody gets round to it.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyEveryone;

impl Authenticator for DenyEveryone {
    fn authenticate(&self, _metadata: &MetadataMap) -> Result<SecurityContext, Status> {
        Err(Status::new(
            Code::Unauthenticated,
            "this server has no authenticator configured and serves nobody",
        ))
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

    /// Build a metadata map by appending, which is what a proxy configured
    /// with nginx's `add_header` (rather than `proxy_set_header`) produces
    /// when the client supplied the header too.
    fn appended(pairs: &[(&str, &str)]) -> MetadataMap {
        use tonic::metadata::MetadataKey;
        let mut map = MetadataMap::new();
        for (key, value) in pairs {
            let key = MetadataKey::from_bytes(key.as_bytes()).expect("ascii metadata key");
            map.append(key, value.parse().expect("ascii metadata value"));
        }
        map
    }

    fn identity() -> MetadataIdentity {
        MetadataIdentity::trusting_the_caller_completely()
    }

    #[test]
    fn a_single_copy_of_each_header_authenticates() {
        let context = identity()
            .authenticate(&appended(&[
                (PRINCIPAL_KEY, "str:alice"),
                (TENANT_KEY, "str:acme"),
                (ROLES_KEY, "reader"),
            ]))
            .expect("one copy of each is the supported arrangement");
        assert_eq!(context.principal().id, Value::Str("alice".to_owned()));
    }

    /// The finding. Two principals means the client sent one and the proxy
    /// appended the real one; `get` would have taken the client's.
    #[test]
    fn a_duplicated_principal_is_refused_rather_than_resolved() {
        let status = identity()
            .authenticate(&appended(&[
                (PRINCIPAL_KEY, "str:attacker"),
                (PRINCIPAL_KEY, "str:alice"),
            ]))
            .expect_err("a repeated identity header must not be resolved");
        assert_eq!(status.code(), Code::Unauthenticated);
        assert!(
            status.message().contains("more than once"),
            "the error should name the cause, got: {}",
            status.message()
        );
    }

    /// Tenant is the isolation boundary itself: winning this one puts the
    /// caller inside another tenant's rows rather than merely under another
    /// name.
    #[test]
    fn a_duplicated_tenant_is_refused() {
        let status = identity()
            .authenticate(&appended(&[
                (PRINCIPAL_KEY, "str:alice"),
                (TENANT_KEY, "str:victim"),
                (TENANT_KEY, "str:acme"),
            ]))
            .expect_err("a repeated tenant header must not be resolved");
        assert_eq!(status.code(), Code::Unauthenticated);
    }

    /// Roles are comma-joined from one header, so an appended copy is a
    /// privilege grant rather than a substitution.
    #[test]
    fn a_duplicated_roles_header_is_refused() {
        let status = identity()
            .authenticate(&appended(&[
                (PRINCIPAL_KEY, "str:alice"),
                (ROLES_KEY, "admin"),
                (ROLES_KEY, "reader"),
            ]))
            .expect_err("a repeated roles header must not be resolved");
        assert_eq!(status.code(), Code::Unauthenticated);
    }

    /// The refusal must not depend on which copy is the odd one out: a
    /// duplicate whose values are identical is still a misconfigured proxy,
    /// and treating it as benign would mean the check passes or fails
    /// depending on what the attacker chose to send.
    #[test]
    fn two_identical_copies_are_still_refused() {
        let status = identity()
            .authenticate(&appended(&[
                (PRINCIPAL_KEY, "str:alice"),
                (PRINCIPAL_KEY, "str:alice"),
            ]))
            .expect_err("a repeated header is a misconfiguration whatever it carries");
        assert_eq!(status.code(), Code::Unauthenticated);
    }

    /// `requiring_a_tenant` must not turn a duplicate into a different error:
    /// the duplicate is found first, so the message names the real problem.
    #[test]
    fn requiring_a_tenant_still_reports_a_duplicate_as_a_duplicate() {
        let status = MetadataIdentity::trusting_the_caller_completely()
            .requiring_a_tenant()
            .authenticate(&appended(&[
                (PRINCIPAL_KEY, "str:alice"),
                (TENANT_KEY, "str:victim"),
                (TENANT_KEY, "str:acme"),
            ]))
            .expect_err("a repeated tenant header must not be resolved");
        assert!(
            status.message().contains("more than once"),
            "got: {}",
            status.message()
        );
    }
}
