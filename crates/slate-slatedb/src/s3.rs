//! S3-compatible object storage: AWS S3, MinIO, Tigris, Cloudflare R2.
//!
//! SlateDB stores everything in an object store, so anything speaking the S3
//! API is a valid substrate. The differences between providers are small and all
//! live in this module: an endpoint override, whether the URL is path-style or
//! virtual-hosted, whether plain HTTP is allowed, and how credentials are found.
//!
//! ```no_run
//! use slate_slatedb::{S3Config, SlateStore};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // MinIO on a developer machine.
//! let store = SlateStore::open_s3(
//!     "/records",
//!     S3Config::new("records")
//!         .with_endpoint("http://127.0.0.1:9000")
//!         .with_credentials("minioadmin", "minioadmin")
//!         .allow_http(true),
//! )
//! .await?;
//! # let _ = store;
//! # Ok(())
//! # }
//! ```
//!
//! Tigris and Cloudflare R2 are reached the same way — set
//! [`S3Config::with_endpoint`] to the endpoint their console gives you and
//! supply the access key pair. Both are HTTPS and accept the default
//! path-style addressing, so no other change is needed. Plain AWS S3 needs no
//! endpoint at all: leave it unset and credentials resolve the usual way
//! (environment, profile, or instance role).

use slate_kernel::error::{KernelError, Result, StorageError};
use slatedb::object_store::ObjectStore;
use slatedb::object_store::aws::AmazonS3Builder;
use std::sync::Arc;

/// How to reach an S3-compatible bucket.
///
/// Only [`S3Config::bucket`] is required. Everything else has a default that
/// works against real AWS S3; the setters exist for the ways other providers
/// differ.
#[derive(Debug, Clone)]
pub struct S3Config {
    bucket: String,
    endpoint: Option<String>,
    region: Option<String>,
    credentials: Option<Credentials>,
    allow_http: bool,
    virtual_hosted_style: bool,
    skip_signature: bool,
}

/// A static access key pair.
#[derive(Clone)]
struct Credentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

impl core::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Never let a secret reach a log line through a Debug impl.
        f.debug_struct("Credentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl S3Config {
    /// Target `bucket` on AWS S3, resolving credentials from the environment.
    #[must_use]
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
            endpoint: None,
            region: None,
            credentials: None,
            allow_http: false,
            virtual_hosted_style: false,
            skip_signature: false,
        }
    }

    /// Point at an S3-compatible service instead of AWS.
    ///
    /// Include the scheme and port, for example `http://127.0.0.1:9000`.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Set the region.
    ///
    /// Services that do not have regions still usually want a placeholder;
    /// `us-east-1` is the conventional one and is what this defaults to when an
    /// endpoint is set.
    #[must_use]
    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Use a static access key pair rather than the environment.
    #[must_use]
    pub fn with_credentials(
        mut self,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
    ) -> Self {
        self.credentials = Some(Credentials {
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            session_token: None,
        });
        self
    }

    /// Attach a session token to the credentials set by
    /// [`S3Config::with_credentials`]. Ignored if no key pair was set.
    #[must_use]
    pub fn with_session_token(mut self, token: impl Into<String>) -> Self {
        if let Some(credentials) = &mut self.credentials {
            credentials.session_token = Some(token.into());
        }
        self
    }

    /// Permit plain HTTP.
    ///
    /// Needed for a local MinIO and for the in-process server the tests use.
    /// Hosted providers are HTTPS, so leave this off for them.
    #[must_use]
    pub const fn allow_http(mut self, allow: bool) -> Self {
        self.allow_http = allow;
        self
    }

    /// Use virtual-hosted-style URLs (`bucket.host/key`) rather than path-style
    /// (`host/bucket/key`).
    ///
    /// Path-style is the default because it is what self-hosted services accept
    /// without DNS setup.
    #[must_use]
    pub const fn virtual_hosted_style(mut self, virtual_hosted: bool) -> Self {
        self.virtual_hosted_style = virtual_hosted;
        self
    }

    /// Send requests unsigned.
    ///
    /// Only for a server that does not check signatures. Never for a real
    /// provider — unsigned requests are either rejected or, on a misconfigured
    /// bucket, anonymous.
    #[must_use]
    pub const fn skip_signature(mut self, skip: bool) -> Self {
        self.skip_signature = skip;
        self
    }

    /// The bucket this config targets.
    #[must_use]
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Read a config from `SLATE_S3_*` environment variables.
    ///
    /// Returns `None` when `SLATE_S3_BUCKET` is unset, which is how the
    /// integration tests decide to skip rather than fail on a machine with no
    /// object store configured.
    ///
    /// | variable | meaning |
    /// |---|---|
    /// | `SLATE_S3_BUCKET` | bucket name; required |
    /// | `SLATE_S3_ENDPOINT` | endpoint URL, for anything but AWS |
    /// | `SLATE_S3_REGION` | region |
    /// | `SLATE_S3_ACCESS_KEY_ID` | access key |
    /// | `SLATE_S3_SECRET_ACCESS_KEY` | secret key |
    /// | `SLATE_S3_ALLOW_HTTP` | `true` to permit plain HTTP |
    /// | `SLATE_S3_VIRTUAL_HOSTED_STYLE` | `true` for `bucket.host/key` URLs |
    #[must_use]
    pub fn from_env() -> Option<Self> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// [`S3Config::from_env`] against an arbitrary lookup.
    ///
    /// Reading the environment through a closure keeps this testable without
    /// mutating process-global state, which is both unsafe in this edition and
    /// unsound to do from a test running beside others.
    #[must_use]
    pub fn from_vars(lookup: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let mut config = Self::new(lookup("SLATE_S3_BUCKET")?);

        if let Some(endpoint) = lookup("SLATE_S3_ENDPOINT") {
            config = config.with_endpoint(endpoint);
        }
        if let Some(region) = lookup("SLATE_S3_REGION") {
            config = config.with_region(region);
        }
        if let (Some(key), Some(secret)) = (
            lookup("SLATE_S3_ACCESS_KEY_ID"),
            lookup("SLATE_S3_SECRET_ACCESS_KEY"),
        ) {
            config = config.with_credentials(key, secret);
        }
        if let Some(token) = lookup("SLATE_S3_SESSION_TOKEN") {
            config = config.with_session_token(token);
        }
        config = config.allow_http(is_true(lookup("SLATE_S3_ALLOW_HTTP").as_deref()));
        config = config
            .virtual_hosted_style(is_true(lookup("SLATE_S3_VIRTUAL_HOSTED_STYLE").as_deref()));
        Some(config)
    }

    /// Build the object store this config describes.
    ///
    /// # Errors
    /// If the bucket, endpoint or credentials are not a usable combination.
    pub fn build(&self) -> Result<Arc<dyn ObjectStore>> {
        let mut builder = AmazonS3Builder::from_env().with_bucket_name(&self.bucket);

        if let Some(endpoint) = &self.endpoint {
            builder = builder.with_endpoint(endpoint);
        }
        // A region is mandatory for request signing; anything self-hosted
        // ignores the value but still needs one present.
        let region = self.region.clone().unwrap_or_else(|| {
            if self.endpoint.is_some() {
                "us-east-1".to_owned()
            } else {
                // Let the environment decide for real AWS.
                std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_owned())
            }
        });
        builder = builder.with_region(region);

        if let Some(credentials) = &self.credentials {
            builder = builder
                .with_access_key_id(&credentials.access_key_id)
                .with_secret_access_key(&credentials.secret_access_key);
            if let Some(token) = &credentials.session_token {
                builder = builder.with_token(token);
            }
        }

        builder = builder
            .with_allow_http(self.allow_http)
            .with_virtual_hosted_style_request(self.virtual_hosted_style)
            .with_skip_signature(self.skip_signature);

        let store = builder
            .build()
            .map_err(|e| KernelError::Storage(StorageError::new(e)))?;
        Ok(Arc::new(store))
    }
}

fn is_true(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        let v = v.trim().to_ascii_lowercase();
        v == "1" || v == "true" || v == "yes"
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn no_bucket_means_absent_rather_than_empty() {
        // The integration suite relies on this to choose between a configured
        // service and its own in-process server.
        assert!(S3Config::from_vars(vars(&[])).is_none());
        assert!(S3Config::from_vars(vars(&[("SLATE_S3_ENDPOINT", "http://x")])).is_none());
    }

    #[test]
    fn a_minio_style_environment_parses() {
        let config = S3Config::from_vars(vars(&[
            ("SLATE_S3_BUCKET", "records"),
            ("SLATE_S3_ENDPOINT", "http://127.0.0.1:9000"),
            ("SLATE_S3_ACCESS_KEY_ID", "minioadmin"),
            ("SLATE_S3_SECRET_ACCESS_KEY", "minioadmin"),
            ("SLATE_S3_ALLOW_HTTP", "true"),
        ]))
        .expect("bucket is set");

        assert_eq!(config.bucket(), "records");
        assert!(config.allow_http);
        assert!(
            !config.virtual_hosted_style,
            "path style suits self-hosting"
        );
        assert!(config.credentials.is_some());
        assert!(config.build().is_ok());
    }

    #[test]
    fn flags_only_count_as_set_when_they_say_so() {
        let flag = |v: &str| {
            S3Config::from_vars(vars(&[
                ("SLATE_S3_BUCKET", "b"),
                ("SLATE_S3_ALLOW_HTTP", v),
            ]))
            .expect("bucket is set")
            .allow_http
        };
        assert!(flag("true") && flag("1") && flag("YES"));
        assert!(!flag("false") && !flag("0") && !flag(""));
    }

    #[test]
    fn credentials_are_not_printed_by_debug() {
        let config = S3Config::new("b").with_credentials("AKIAEXAMPLE", "super-secret");
        let rendered = format!("{config:?}");
        assert!(rendered.contains("AKIAEXAMPLE"), "key id is useful in logs");
        assert!(
            !rendered.contains("super-secret"),
            "secret leaked through Debug: {rendered}"
        );
    }

    #[test]
    fn plain_aws_needs_no_endpoint() {
        assert!(S3Config::new("b").build().is_ok());
    }
}
