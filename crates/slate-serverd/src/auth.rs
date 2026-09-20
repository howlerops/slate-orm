//! Deciding who is asking, from a file.
//!
//! This is the part of a head node that a hand-rolled binary gets wrong, and
//! the reason it gets wrong is structural rather than careless:
//! [`Head::new`](slate_server::Head::new) requires an
//! [`Authenticator`](slate_server::Authenticator) and the only one shipped is
//! `MetadataIdentity::trusting_the_caller_completely()`, which is correct
//! behind a proxy and an open door anywhere else. Every binary written to get
//! a server running reaches for it, because it is the one that compiles.
//!
//! # The rule this module enforces
//!
//! **There is no default, and no configuration is accidentally insecure.**
//! Concretely:
//!
//! 1. **No `[auth]` section is a refusal to start.** Not a deny-all fallback,
//!    which would be safe but would teach an operator that the section is
//!    optional; not loopback-only, which would be safe and would make the
//!    server mysteriously unreachable. The message names the three modes and
//!    what each is for.
//! 2. **A mode whose safety depends on something outside this process must
//!    name that thing, and only when it matters.** `trusted-header` is correct
//!    exactly when a proxy sets the identity headers and strips the client's;
//!    `token` sends a bearer secret in the clear unless something upstream has
//!    already encrypted the connection. Off loopback, each requires the
//!    operator to write down the assumption. On loopback neither is required,
//!    because the assumption is not being made: nothing but this machine can
//!    connect.
//! 3. **A field belonging to another mode is a refusal**, so `mode =
//!    "deny-all"` with a list of tokens under it fails rather than silently
//!    serving nobody, and `mode = "token"` with `header_source` fails rather
//!    than silently ignoring it.
//! 4. **Nothing here can produce a superuser.** `SecurityContext::superuser`
//!    bypasses every check and is deliberately one grep away; this module does
//!    not call it, and a test asserts that no configuration reaches it.
//!
//! # Why a "state your assumption" flag rather than refusing outright
//!
//! Refusing `trusted-header` off loopback entirely was the first design and is
//! wrong: it is the correct configuration for the commonest deployment there
//! is, a mesh sidecar that has already authenticated the caller. Refusing it
//! would push operators to the one remaining mode whether or not it fits,
//! which is how a safety rail becomes the reason for a workaround.
//!
//! The flag is not a formality either. `header_source = "trusted-proxy"` is a
//! sentence an operator has to mean, sitting in a file a reviewer reads, next
//! to the address that makes it necessary. That is the same instrument
//! `trusting_the_caller_completely()` uses — an uncomfortable name in a place
//! people look — moved from a Rust call site to the file that replaced it.
//!
//! # Why bearer tokens at all
//!
//! Because otherwise the only mode that works off loopback is one that trusts
//! the caller, and "run it behind a mesh" is not an answer for the client
//! author who is the reason this binary exists. A shared secret per client is
//! the smallest thing that is actually an authenticator: no key distribution,
//! no clock, no revocation list, and an identity the operator wrote down.
//!
//! What it is not is a token *system*. There is no expiry, no rotation, no
//! introspection endpoint and no signature — those need an issuer, and an
//! issuer is a second system this file has no business inventing. A deployment
//! that has one writes an `Authenticator` that verifies its JWTs and links
//! `slate-server` directly; this mode exists so that the deployment that has
//! none is not forced to choose between that and trusting everybody.

use crate::config;
use crate::error::{Fault, Started};
use crate::value;
use slate_kernel::{Principal, SecurityContext};
use slate_server::{Authenticator, DenyEveryone, MetadataIdentity};
use std::net::SocketAddr;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tonic::metadata::MetadataMap;
use tonic::{Code, Status};

/// The metadata key a bearer token arrives under.
const AUTHORIZATION: &str = "authorization";

/// The shortest secret this server will accept, in characters.
///
/// A bearer token here has no expiry and no rate limit in front of it, so its
/// only defence is being too long to guess. Thirty-two characters of any
/// alphabet is far past what an online attacker reaches, and it is short
/// enough that `openssl rand -hex 16` satisfies it. The point of the floor is
/// not the exact number: it is that `secret_env` pointing at a variable
/// holding `dev` is refused at startup rather than deployed.
const MINIMUM_SECRET: usize = 32;

/// How this node decides who is asking, and what to say about it at startup.
#[derive(Debug)]
pub(crate) struct Chosen {
    /// The authenticator itself.
    pub(crate) authenticator: Arc<dyn Authenticator>,
    /// One line for the startup banner, so the mode in force is visible in a
    /// log rather than only in a file somebody may not have on hand.
    pub(crate) description: String,
}

/// Choose an authenticator, or refuse to start.
pub(crate) fn choose(
    auth: Option<&config::Auth>,
    address: &SocketAddr,
    warnings: &mut Vec<String>,
) -> Started<Chosen> {
    let Some(auth) = auth else {
        return Err(Fault::new(missing_section_message()));
    };
    let loopback = address.ip().is_loopback();

    match auth.mode.as_str() {
        "deny-all" => {
            forbid_fields(
                auth,
                &["header_source", "transport", "require_tenant", "tokens"],
            )?;
            warnings.push(
                "`[auth] mode = \"deny-all\"`: this node authenticates nobody and will refuse every request".to_owned(),
            );
            Ok(Chosen {
                authenticator: Arc::new(DenyEveryone),
                description: "deny-all (every request is refused)".to_owned(),
            })
        }

        "trusted-header" => {
            forbid_fields(auth, &["transport", "tokens"])?;
            if !loopback && auth.header_source.is_none() {
                return Err(Fault::new(format!(
                    "`[auth] mode = \"trusted-header\"` takes the caller's identity from headers the caller sends, so it is correct only behind a proxy that authenticates the caller, sets `slate-principal`, `slate-tenant` and `slate-roles` itself, and strips any the client supplied.\n\
                     This node binds {address}, which is not loopback, so that assumption has to be written down: add `header_source = \"trusted-proxy\"` under `[auth]`.\n\
                     If there is no such proxy, this mode is an open door — use `mode = \"token\"` instead."
                )));
            }
            match auth.header_source.as_deref() {
                None | Some("trusted-proxy") => {}
                Some(other) => {
                    return Err(Fault::new(format!(
                        "`[auth] header_source = \"{other}\"` is not something this server knows; the only value is `\"trusted-proxy\"`, which states that a proxy sets the identity headers and strips the client's"
                    )));
                }
            }
            let mut identity = MetadataIdentity::trusting_the_caller_completely();
            let require_tenant = auth.require_tenant.unwrap_or(false);
            if require_tenant {
                identity = identity.requiring_a_tenant();
            }
            if loopback && auth.header_source.is_none() {
                warnings.push(format!(
                    "`[auth] mode = \"trusted-header\"` on {address}: the caller's own headers are trusted. That is safe here only because nothing off this machine can connect"
                ));
            }
            Ok(Chosen {
                authenticator: Arc::new(identity),
                description: format!(
                    "trusted-header (identity from `slate-principal`; tenant {})",
                    if require_tenant {
                        "required"
                    } else {
                        "optional"
                    }
                ),
            })
        }

        "token" => {
            forbid_fields(auth, &["header_source", "require_tenant"])?;
            if !loopback && auth.transport.is_none() {
                return Err(Fault::new(format!(
                    "`[auth] mode = \"token\"` sends a shared secret in the request metadata, and this server terminates no TLS of its own — `slate-server` leaves that to whatever fronts it.\n\
                     This node binds {address}, which is not loopback, so the secret would cross the network as it is written unless something upstream has already encrypted the connection. Add `transport = \"tls-terminated-upstream\"` under `[auth]` to state that it has."
                )));
            }
            match auth.transport.as_deref() {
                None | Some("tls-terminated-upstream") => {}
                Some(other) => {
                    return Err(Fault::new(format!(
                        "`[auth] transport = \"{other}\"` is not something this server knows; the only value is `\"tls-terminated-upstream\"`"
                    )));
                }
            }
            let identity = TokenIdentity::build(&auth.tokens)?;
            let count = identity.tokens.len();
            Ok(Chosen {
                authenticator: Arc::new(identity),
                description: format!(
                    "token ({count} bearer token{})",
                    if count == 1 { "" } else { "s" }
                ),
            })
        }

        other => Err(Fault::new(format!(
            "`[auth] mode = \"{other}\"` is not a mode this server knows.\n{MODES}"
        ))),
    }
}

const MODES: &str = "\
  mode = \"deny-all\"        authenticate nobody; the node starts and refuses every request.
                          For bringing a node up before its identities exist.
  mode = \"trusted-header\"  take the identity from `slate-principal`, `slate-tenant` and
                          `slate-roles`. Correct only behind a proxy that sets them and
                          strips the client's; off loopback it must say so with
                          `header_source = \"trusted-proxy\"`.
  mode = \"token\"           accept `authorization: Bearer <secret>` against a list of
                          tokens, each standing for one principal. Off loopback it must
                          say so with `transport = \"tls-terminated-upstream\"`.";

const MISSING_SECTION: &str = "\
this configuration has no `[auth]` section, and this server has no default for one.

Every default is wrong: a permissive default is a hole, and a rejecting default is a
server that looks broken until somebody turns the check off to make it work. So the
decision is made once, in the file, where a reviewer can see it.

Add one of:

";

/// Refuse a field that belongs to a different mode.
///
/// Without this, `mode = "deny-all"` with a `[[auth.tokens]]` block under it
/// starts and serves nobody, and the operator debugs their client. serde's
/// `deny_unknown_fields` cannot catch it: the field is known, just not here.
fn forbid_fields(auth: &config::Auth, fields: &[&str]) -> Started<()> {
    let present: Vec<&str> = fields
        .iter()
        .copied()
        .filter(|field| match *field {
            "header_source" => auth.header_source.is_some(),
            "transport" => auth.transport.is_some(),
            "require_tenant" => auth.require_tenant.is_some(),
            "tokens" => !auth.tokens.is_empty(),
            _ => false,
        })
        .collect();
    if present.is_empty() {
        return Ok(());
    }
    Err(Fault::new(format!(
        "`[auth] mode = \"{}\"` does not use {}; leaving it here would mean it was quietly ignored, and the mode is probably not the one that was meant.\n{MODES}",
        auth.mode,
        present
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(" or ")
    )))
}

/// One accepted bearer token and the identity it stands for.
struct Bearer {
    name: String,
    secret: Vec<u8>,
    principal: Principal,
}

/// Accepts `authorization: Bearer <secret>` against a fixed list.
pub(crate) struct TokenIdentity {
    tokens: Vec<Bearer>,
}

impl core::fmt::Debug for TokenIdentity {
    /// Names, never secrets. `Head`'s own `Debug` prints the authenticator's,
    /// and a panic message or a trace that carried the tokens would be a
    /// credential leak into a log nobody thought of as sensitive.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TokenIdentity")
            .field(
                "tokens",
                &self.tokens.iter().map(|t| &t.name).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl TokenIdentity {
    fn build(tokens: &[config::Token]) -> Started<Self> {
        if tokens.is_empty() {
            return Err(Fault::new(
                "`[auth] mode = \"token\"` with no `[[auth.tokens]]` accepts nobody. If that is what you want, write `mode = \"deny-all\"`, which says so",
            ));
        }
        let mut built: Vec<Bearer> = Vec::with_capacity(tokens.len());
        for token in tokens {
            let place = format!("`[[auth.tokens]]` named `{}`", token.name);
            let secret = read_secret(token, &place)?;

            // Two identities behind one secret is not a configuration, it is a
            // mistake with a security consequence: whichever comes first wins
            // every request and the other principal is unreachable. Compared
            // in constant time out of habit rather than need — this runs at
            // startup against secrets the operator already holds.
            if let Some(clash) = built
                .iter()
                .find(|other| bool::from(other.secret.ct_eq(secret.as_slice())))
            {
                return Err(Fault::new(format!(
                    "tokens `{}` and `{}` have the same secret, so one of the two identities is unreachable",
                    clash.name, token.name
                )));
            }
            if built.iter().any(|other| other.name == token.name) {
                return Err(Fault::new(format!(
                    "two `[[auth.tokens]]` are named `{}`",
                    token.name
                )));
            }

            let id = value::tagged(&token.principal, &format!("{place}: `principal`"))?;
            let mut principal = Principal::new(id);
            if let Some(tenant) = &token.tenant {
                principal =
                    principal.with_tenant(value::tagged(tenant, &format!("{place}: `tenant`"))?);
            }
            for role in &token.roles {
                principal = principal.with_role(role);
            }

            built.push(Bearer {
                name: token.name.clone(),
                secret,
                principal,
            });
        }
        Ok(Self { tokens: built })
    }
}

impl Authenticator for TokenIdentity {
    fn authenticate(&self, metadata: &MetadataMap) -> Result<SecurityContext, Status> {
        // A repeated `authorization` is refused rather than resolved, for the
        // reason finding 6 of the security review gives about the trusted
        // header mode: taking the first trusts a proxy that replaces, taking
        // the last trusts one that appends, and the server cannot tell which
        // it is behind. `get` takes the first, which is the *caller's* copy
        // exactly when the proxy appends.
        //
        // The same fix landed in `slate_server::auth::text` and stopped
        // there, because that closed the finding as written and this is the
        // other implementation of the same trait. Measured here before this
        // block existed: with the caller's token first and the proxy's second,
        // the request authenticated as the caller's principal.
        //
        // No privilege escalation was demonstrated — a caller needs a valid
        // token either way, so the usual arrangement only ever downgrades them
        // to themselves. It defeats a proxy that *downscopes* by replacing the
        // caller's token with a narrower one, and it is an ambiguity resolved
        // by a rule the finding explicitly rejected as unsafe to rely on.
        let mut headers = metadata.get_all(AUTHORIZATION).iter();
        let Some(header) = headers.next() else {
            return Err(Status::new(
                Code::Unauthenticated,
                "no `authorization` in the request metadata; this server expects `authorization: Bearer <token>`",
            ));
        };
        if headers.next().is_some() {
            // Naming neither value: the message is read by whatever collects
            // this server's errors, and a bearer token in a log is a bearer
            // token in a log.
            return Err(Status::new(
                Code::Unauthenticated,
                "`authorization` appears more than once in the request metadata; \
                 the proxy in front of this server must replace this header rather \
                 than append to it",
            ));
        }
        let header = header.to_str().map_err(|_| {
            Status::new(
                Code::Unauthenticated,
                "`authorization` is not valid ASCII metadata",
            )
        })?;
        let Some(presented) = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
        else {
            return Err(Status::new(
                Code::Unauthenticated,
                "`authorization` is not a bearer token; this server expects `authorization: Bearer <token>`",
            ));
        };

        // Every token is compared, and the whole list is walked whether or not
        // one matched. Returning on the first hit would make the time taken
        // depend on the position of the matching token, which over enough
        // requests names it; `ct_eq` keeps each individual comparison from
        // leaking a prefix the same way. It is a linear scan for the same
        // reason it is not a hash map: a map lookup's timing depends on the
        // secret.
        //
        // What is *not* hidden is length. `ct_eq` on slices of different
        // lengths is false without comparing, so an attacker learns whether
        // their guess is as long as some configured secret. That is worth
        // little against a 32-character minimum and is stated rather than
        // papered over.
        let presented = presented.as_bytes();
        let mut found: Option<&Bearer> = None;
        for token in &self.tokens {
            if bool::from(token.secret.ct_eq(presented)) {
                found = Some(token);
            }
        }

        found
            .map(|token| SecurityContext::new(token.principal.clone()))
            .ok_or_else(|| {
                // Unauthenticated rather than PermissionDenied: the caller has not
                // established who they are. The message names no token, because
                // "no such token" and "wrong secret" are the same fact here and
                // distinguishing them would be an oracle.
                Status::new(
                    Code::Unauthenticated,
                    "the bearer token is not one this server accepts",
                )
            })
    }
}

/// Read a token's secret from the environment or a file.
///
/// Never from the configuration file itself. The file is the artefact that
/// gets committed, copied into a ticket and pasted into a chat window; a
/// secret in it is a secret in all three. An inline `secret = "…"` was
/// considered for development convenience and rejected: development
/// configurations become production configurations, and the convenient path
/// has to be the safe one.
fn read_secret(token: &config::Token, place: &str) -> Started<Vec<u8>> {
    let raw = match (&token.secret_env, &token.secret_file) {
        (Some(_), Some(_)) => {
            return Err(Fault::new(format!(
                "{place} has both `secret_env` and `secret_file`; it takes one"
            )));
        }
        (None, None) => {
            return Err(Fault::new(format!(
                "{place} has no secret. Write `secret_env = \"SOME_VARIABLE\"` or `secret_file = \"/path\"` — a secret is never written in this file, which is the file that gets committed"
            )));
        }
        (Some(variable), None) => std::env::var(variable).map_err(|_| {
            Fault::new(format!(
                "{place}: the environment variable `{variable}` is not set"
            ))
        })?,
        (None, Some(path)) => std::fs::read_to_string(path)
            .map_err(|why| Fault::new(format!("{place}: cannot read `{path}`: {why}")))?,
    };

    // Trimmed at both ends, because `echo secret > token` leaves a newline and
    // an operator who hit that would have a token that never matches and no
    // way to see why.
    let secret = raw.trim();

    if secret.chars().count() < MINIMUM_SECRET {
        return Err(Fault::new(format!(
            "{place}: the secret is {} characters and the minimum is {MINIMUM_SECRET}. A bearer token here has no expiry and nothing rate-limiting it, so length is the whole of its strength",
            secret.chars().count()
        )));
    }
    if !secret.is_ascii() || secret.bytes().any(|b| b.is_ascii_whitespace()) {
        return Err(Fault::new(format!(
            "{place}: the secret has a space or a character that is not ASCII, and gRPC metadata carries neither. Use hexadecimal or base64 — `openssl rand -hex 32`"
        )));
    }
    Ok(secret.as_bytes().to_vec())
}

/// Rendered separately from [`MISSING_SECTION`] so the mode list is written
/// once.
pub(crate) fn missing_section_message() -> String {
    format!("{MISSING_SECTION}{MODES}")
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
    use slate_tuple::Value;

    fn auth(text: &str) -> config::Auth {
        toml::from_str(text).unwrap_or_else(|e| panic!("{e}"))
    }

    fn at(address: &str) -> SocketAddr {
        address.parse().unwrap_or_else(|e| panic!("{address}: {e}"))
    }

    fn choose_at(text: &str, address: &str) -> Started<Chosen> {
        let mut warnings = Vec::new();
        choose(Some(&auth(text)), &at(address), &mut warnings)
    }

    fn bearer(secret: &str) -> MetadataMap {
        let mut metadata = MetadataMap::new();
        if let Ok(value) = format!("Bearer {secret}").parse() {
            metadata.insert(AUTHORIZATION, value);
        }
        metadata
    }

    /// Every `impl Authenticator for` in the workspace.
    ///
    /// `scripts/check_handlers.py` fails if one exists that is not here, so a
    /// new authenticator cannot quietly skip
    /// `no_authenticator_resolves_a_duplicated_identity_key`.
    const AUTHENTICATORS: [&str; 3] = ["MetadataIdentity", "DenyEveryone", "TokenIdentity"];

    /// The header `MetadataIdentity` reads. Spelled out rather than imported
    /// because `slate-server` does not export the constant, and a literal is
    /// safe here for a reason worth stating: if the header were renamed, this
    /// would send a key that authenticator ignores, it would refuse with "no
    /// `…` in the request metadata", and the "more than once" assertion below
    /// would fail. The test breaks loudly rather than passing vacuously.
    const PRINCIPAL_KEY: &str = "slate-principal";

    const GOOD: &str = "0123456789abcdef0123456789abcdef";
    /// A second, equally valid token belonging to a *different* principal, so
    /// a test about which copy wins can name the winner.
    const OTHER: &str = "fedcba9876543210fedcba9876543210";

    fn two_token_config(directory: &std::path::Path) -> String {
        let first = directory.join("first");
        let second = directory.join("second");
        std::fs::write(&first, GOOD).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(&second, OTHER).unwrap_or_else(|e| panic!("{e}"));
        format!(
            "mode = \"token\"\n\
             [[tokens]]\nname = \"caller\"\nsecret_file = \"{}\"\n\
             principal = \"u64:7\"\ntenant = \"u64:1\"\nroles = [\"app\"]\n\
             [[tokens]]\nname = \"proxy\"\nsecret_file = \"{}\"\n\
             principal = \"u64:9\"\ntenant = \"u64:1\"\nroles = [\"app\"]\n",
            first.display(),
            second.display()
        )
    }

    #[test]
    fn no_auth_section_refuses_to_start_and_names_every_mode() {
        let mut warnings = Vec::new();
        let error = choose(None, &at("127.0.0.1:1"), &mut warnings)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no `[auth]` section"), "{error}");
        for mode in ["deny-all", "trusted-header", "token"] {
            assert!(error.contains(mode), "{error} is missing {mode}");
        }
    }

    #[test]
    fn trusted_header_off_loopback_must_state_its_proxy() {
        let error = choose_at("mode = \"trusted-header\"", "0.0.0.0:50051")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("header_source = \"trusted-proxy\""),
            "{error}"
        );
        assert!(error.contains("0.0.0.0:50051"), "{error}");
    }

    #[test]
    fn trusted_header_off_loopback_is_allowed_once_it_does() {
        let chosen = choose_at(
            "mode = \"trusted-header\"\nheader_source = \"trusted-proxy\"",
            "10.0.0.4:50051",
        )
        .unwrap();
        assert!(chosen.description.contains("trusted-header"));
    }

    #[test]
    fn trusted_header_on_loopback_needs_no_statement_but_says_so() {
        let mut warnings = Vec::new();
        let chosen = choose(
            Some(&auth("mode = \"trusted-header\"")),
            &at("127.0.0.1:0"),
            &mut warnings,
        )
        .unwrap();
        assert!(chosen.description.contains("trusted-header"));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("nothing off this machine")),
            "{warnings:?}"
        );
    }

    #[test]
    fn an_unspecified_bind_address_is_not_loopback() {
        // `0.0.0.0` includes loopback and everything else, and the everything
        // else is the point. A check that treated it as loopback would be the
        // single most likely way to get this wrong.
        assert!(choose_at("mode = \"trusted-header\"", "0.0.0.0:1").is_err());
        assert!(choose_at("mode = \"trusted-header\"", "[::]:1").is_err());
        assert!(choose_at("mode = \"trusted-header\"", "[::1]:1").is_ok());
        assert!(choose_at("mode = \"trusted-header\"", "127.0.0.5:1").is_ok());
    }

    #[test]
    fn token_mode_off_loopback_must_state_its_transport() {
        // The transport check runs before the tokens are read, which is what
        // this asserts: the refusal is about the address, not about a missing
        // environment variable.
        let text = "mode = \"token\"\n[[tokens]]\nname = \"a\"\nsecret_env = \"SLATE_TEST_TOKEN\"\nprincipal = \"u64:1\"\n";
        let error = choose_at(text, "10.0.0.4:50051").unwrap_err().to_string();
        assert!(error.contains("tls-terminated-upstream"), "{error}");
    }

    fn token_config(directory: &std::path::Path, secret: &str) -> String {
        let path = directory.join("secret");
        std::fs::write(&path, secret).unwrap_or_else(|e| panic!("{e}"));
        format!(
            "mode = \"token\"\n[[tokens]]\nname = \"reporting\"\nsecret_file = \"{}\"\nprincipal = \"u64:7\"\ntenant = \"u64:1\"\nroles = [\"app\"]\n",
            path.display()
        )
    }

    #[test]
    fn a_token_authenticates_the_principal_it_names() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let chosen = choose_at(&token_config(dir.path(), GOOD), "127.0.0.1:0").unwrap();
        let context = chosen.authenticator.authenticate(&bearer(GOOD)).unwrap();
        assert_eq!(context.principal().id, Value::U64(7));
        assert_eq!(context.principal().tenant, Some(Value::U64(1)));
        assert!(context.principal().roles.contains("app"));
        assert!(!context.is_superuser());
    }

    #[test]
    fn a_wrong_token_is_unauthenticated_and_names_nothing() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let chosen = choose_at(&token_config(dir.path(), GOOD), "127.0.0.1:0").unwrap();
        let status = chosen
            .authenticator
            .authenticate(&bearer("f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0"))
            .unwrap_err();
        assert_eq!(status.code(), Code::Unauthenticated);
        assert!(!status.message().contains("reporting"), "{status}");
    }

    #[test]
    fn no_token_at_all_is_unauthenticated() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let chosen = choose_at(&token_config(dir.path(), GOOD), "127.0.0.1:0").unwrap();
        let status = chosen
            .authenticator
            .authenticate(&MetadataMap::new())
            .unwrap_err();
        assert_eq!(status.code(), Code::Unauthenticated);
    }

    #[test]
    fn a_trailing_newline_in_a_secret_file_is_trimmed() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let chosen = choose_at(
            &token_config(dir.path(), &format!("{GOOD}\n")),
            "127.0.0.1:0",
        )
        .unwrap();
        assert!(chosen.authenticator.authenticate(&bearer(GOOD)).is_ok());
    }

    /// Finding 6's shape, in the daemon's *other* authenticator.
    ///
    /// The review's finding 6 was that `MetadataIdentity` took the first copy
    /// of a repeated identity header, which is the client's exactly when the
    /// proxy appends rather than replaces. That was fixed there, in
    /// `slate_server::auth::text`. `TokenIdentity` is the second
    /// implementation of the same trait and reached for `metadata.get`, which
    /// is also the first copy — so the fix covered one of two.
    ///
    /// Two tokens with different principals, so the assertion can say *which*
    /// one won rather than merely that something did. Before the fix this
    /// authenticated as principal 7, the caller's own copy, over the one the
    /// proxy appended.
    #[test]
    fn a_duplicated_authorization_header_is_refused_rather_than_resolved() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let chosen = choose_at(&two_token_config(dir.path()), "127.0.0.1:0").unwrap();

        let mut metadata = MetadataMap::new();
        // The caller's own header arrives first...
        metadata.append(AUTHORIZATION, format!("Bearer {GOOD}").parse().unwrap());
        // ...and the proxy appends its own after it.
        metadata.append(AUTHORIZATION, format!("Bearer {OTHER}").parse().unwrap());

        let status = chosen
            .authenticator
            .authenticate(&metadata)
            .expect_err("a duplicated authorization header must not be resolved");
        assert_eq!(status.code(), Code::Unauthenticated);
        assert!(
            status.message().contains("more than once"),
            "the error should name the duplicate: {}",
            status.message()
        );
        // The refusal must not echo either secret, which would turn a
        // misconfiguration into a token disclosure in whatever reads the logs.
        assert!(
            !status.message().contains(GOOD) && !status.message().contains(OTHER),
            "the refusal must not echo a token: {}",
            status.message()
        );
    }

    /// The control: one copy still authenticates, and as the right principal.
    ///
    /// Without this the test above passes for an authenticator that refuses
    /// every request.
    #[test]
    fn a_single_authorization_header_still_authenticates() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let chosen = choose_at(&two_token_config(dir.path()), "127.0.0.1:0").unwrap();
        let context = chosen
            .authenticator
            .authenticate(&bearer(OTHER))
            .expect("one copy authenticates");
        assert_eq!(context.principal().id, Value::U64(9));
    }

    /// Every implementation of `Authenticator`, against the hole finding 6
    /// found in one of them.
    ///
    /// The finding was that `metadata.get` returns the *first* value for a
    /// repeated key, which is the caller's copy exactly when the proxy appends
    /// rather than replaces. It was fixed in `MetadataIdentity`, and
    /// `TokenIdentity` went on doing it because the trait says nothing about
    /// duplicated keys and nothing looked at the other implementation.
    ///
    /// Both are right now. This is here so a *third* one cannot be wrong
    /// quietly: `AUTHENTICATORS` names every implementation, and
    /// `scripts/check_handlers.py` fails if an `impl Authenticator for` exists
    /// that the list does not name. A new authenticator therefore arrives with
    /// a failing check rather than with a hole.
    ///
    /// Each reads a different key, so each case names its own. `DenyEveryone`
    /// reads none and refuses regardless — included anyway, because the list
    /// has to be complete for the check to mean anything, and a case that is
    /// trivially true is cheaper than an exception that has to be argued.
    #[test]
    fn no_authenticator_resolves_a_duplicated_identity_key() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let chosen = choose_at(&two_token_config(dir.path()), "127.0.0.1:0").unwrap();

        // Bound so the trait objects below outlive the vector: a
        // `&MetadataIdentity::…()` inline borrows a temporary.
        let headers = MetadataIdentity::trusting_the_caller_completely();
        let nobody = DenyEveryone;
        // A struct rather than a five-tuple: clippy calls the tuple a "very
        // complex type" and CI runs `-D warnings`, but the better reason is
        // that `names_the_duplicate` reads as a field rather than as the fifth
        // element of something.
        // Only the authenticator borrows locally; the rest are literals and
        // must say so. Tying them all to one `'a` makes inference unify it
        // with `'static` through `MetadataMap::append`, which then demands a
        // `'static` authenticator — three "does not live long enough" errors
        // about the wrong thing.
        struct Case<'a> {
            name: &'static str,
            authenticator: &'a dyn Authenticator,
            /// The metadata key this implementation reads.
            key: &'static str,
            /// Two different values, sent under that one key.
            values: [&'static str; 2],
            /// Whether the refusal should name the duplicate. False only for
            /// an authenticator that refuses before looking at anything.
            names_the_duplicate: bool,
        }

        let cases = vec![
            Case {
                name: "MetadataIdentity",
                authenticator: &headers,
                key: PRINCIPAL_KEY,
                values: ["u64:666", "u64:1"],
                names_the_duplicate: true,
            },
            Case {
                name: "DenyEveryone",
                authenticator: &nobody,
                key: PRINCIPAL_KEY,
                values: ["u64:1", "u64:2"],
                names_the_duplicate: false,
            },
            Case {
                name: "TokenIdentity",
                authenticator: chosen.authenticator.as_ref(),
                key: "authorization",
                values: [
                    "Bearer 0123456789abcdef0123456789abcdef",
                    "Bearer fedcba9876543210fedcba9876543210",
                ],
                names_the_duplicate: true,
            },
        ];
        assert_eq!(
            cases.len(),
            AUTHENTICATORS.len(),
            "every implementation in AUTHENTICATORS needs a case here"
        );

        for case in cases {
            let Case {
                name,
                authenticator,
                key,
                values,
                names_the_duplicate,
            } = case;
            let mut metadata = MetadataMap::new();
            for value in values {
                metadata.append(key, value.parse().unwrap());
            }
            let status = authenticator
                .authenticate(&metadata)
                .err()
                .unwrap_or_else(|| {
                    panic!("{name} resolved a duplicated `{key}` instead of refusing it")
                });
            assert_eq!(status.code(), Code::Unauthenticated, "{name}");
            if names_the_duplicate {
                assert!(
                    status.message().contains("more than once"),
                    "{name} refused for some other reason: {}",
                    status.message()
                );
            }
            // Whatever the reason, the refusal must not echo what the caller
            // sent — for a token that would put a bearer token into whatever
            // collects this server's errors.
            for value in values {
                assert!(
                    !status.message().contains(value),
                    "{name} echoed what it rejected: {}",
                    status.message()
                );
            }
        }
    }

    #[test]
    fn a_short_secret_is_refused_with_the_reason() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let error = choose_at(&token_config(dir.path(), "hunter2"), "127.0.0.1:0")
            .unwrap_err()
            .to_string();
        assert!(error.contains("minimum is 32"), "{error}");
    }

    #[test]
    fn two_tokens_sharing_a_secret_are_refused() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("s");
        std::fs::write(&path, GOOD).unwrap_or_else(|e| panic!("{e}"));
        let text = format!(
            "mode = \"token\"\n\
             [[tokens]]\nname = \"a\"\nsecret_file = \"{p}\"\nprincipal = \"u64:1\"\n\
             [[tokens]]\nname = \"b\"\nsecret_file = \"{p}\"\nprincipal = \"u64:2\"\n",
            p = path.display()
        );
        let error = choose_at(&text, "127.0.0.1:0").unwrap_err().to_string();
        assert!(error.contains("unreachable"), "{error}");
    }

    #[test]
    fn a_secret_written_in_the_file_is_not_a_field_at_all() {
        let error = toml::from_str::<config::Auth>(
            "mode = \"token\"\n[[tokens]]\nname = \"a\"\nsecret = \"x\"\nprincipal = \"u64:1\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("secret"), "{error}");
    }

    #[test]
    fn token_mode_with_no_tokens_points_at_deny_all() {
        let error = choose_at("mode = \"token\"", "127.0.0.1:0")
            .unwrap_err()
            .to_string();
        assert!(error.contains("deny-all"), "{error}");
    }

    #[test]
    fn a_field_from_another_mode_is_refused() {
        let error = choose_at(
            "mode = \"deny-all\"\n[[tokens]]\nname = \"a\"\nsecret_env = \"X\"\nprincipal = \"u64:1\"\n",
            "127.0.0.1:0",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("does not use `tokens`"), "{error}");

        let error = choose_at(
            "mode = \"trusted-header\"\ntransport = \"tls-terminated-upstream\"",
            "127.0.0.1:0",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("does not use `transport`"), "{error}");
    }

    #[test]
    fn an_unknown_mode_lists_the_modes() {
        let error = choose_at("mode = \"none\"", "127.0.0.1:0")
            .unwrap_err()
            .to_string();
        assert!(error.contains("deny-all"), "{error}");
        assert!(error.contains("trusted-header"), "{error}");
        assert!(error.contains("token"), "{error}");
    }

    #[test]
    fn deny_all_authenticates_nobody() {
        let chosen = choose_at("mode = \"deny-all\"", "127.0.0.1:0").unwrap();
        assert!(chosen.authenticator.authenticate(&bearer(GOOD)).is_err());
        assert!(
            chosen
                .authenticator
                .authenticate(&MetadataMap::new())
                .is_err()
        );
    }

    #[test]
    fn debug_never_prints_a_secret() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let chosen = choose_at(&token_config(dir.path(), GOOD), "127.0.0.1:0").unwrap();
        let rendered = format!("{:?}", chosen.authenticator);
        assert!(rendered.contains("reporting"), "{rendered}");
        assert!(
            !rendered.contains(GOOD),
            "the secret is in the Debug output"
        );
    }

    #[test]
    fn no_mode_can_produce_a_superuser() {
        // The one property worth asserting across every mode at once: a
        // superuser bypasses RBAC, the tenant restriction and every policy, so
        // a configuration file that could name one would undo the whole
        // security layer.
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let configurations = [
            "mode = \"deny-all\"".to_owned(),
            "mode = \"trusted-header\"".to_owned(),
            token_config(dir.path(), GOOD),
        ];
        let mut principal = MetadataMap::new();
        if let Ok(value) = "u64:1".parse() {
            principal.insert("slate-principal", value);
        }
        for text in configurations {
            let chosen = choose_at(&text, "127.0.0.1:0").unwrap();
            for metadata in [&bearer(GOOD), &principal, &MetadataMap::new()] {
                if let Ok(context) = chosen.authenticator.authenticate(metadata) {
                    assert!(
                        !context.is_superuser(),
                        "{text} produced a superuser context"
                    );
                }
            }
        }
    }
}
