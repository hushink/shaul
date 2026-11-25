use std::collections::BTreeSet;

use http::{HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A validated HTTP header pair that can be persisted in a task sidecar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// Header name, trimmed and validated as an HTTP header name.
    pub name: String,
    /// Header value, trimmed. May contain `${env:NAME}` placeholders that
    /// backends expand when building the request.
    pub value: String,
}

impl Header {
    /// Trim and validate the pair, rejecting malformed HTTP header names and
    /// values with [`Error::Config`].
    pub fn new(name: impl AsRef<str>, value: impl AsRef<str>) -> Result<Self> {
        let name = name.as_ref().trim().to_string();
        let value = value.as_ref().trim().to_string();
        HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| Error::Config(format!("invalid header name: {name}")))?;
        HeaderValue::from_str(&value)
            .map_err(|_| Error::Config(format!("invalid value for header {name}")))?;
        Ok(Self { name, value })
    }

    /// Whether the header is one of the credential-bearing names, compared
    /// case-insensitively. See [`is_sensitive_header_name`].
    pub fn is_sensitive(&self) -> bool {
        is_sensitive_header_name(&self.name)
    }

    /// Clone the header, replacing the value with `<redacted>` when
    /// [`is_sensitive`](Self::is_sensitive) is true.
    pub fn redacted(&self) -> Self {
        if self.is_sensitive() {
            Self {
                name: self.name.clone(),
                value: "<redacted>".to_string(),
            }
        } else {
            self.clone()
        }
    }
}

/// Whether a header name conventionally carries credentials. Used for
/// sidecar/JSON redaction, `config get/show/set` and cross-origin redirect
/// stripping.
pub fn is_sensitive_header_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "cookie"
            | "proxy-authorization"
            | "x-api-key"
            | "api-key"
            | "apikey"
            | "x-auth-token"
            | "x-access-token"
            | "x-api-token"
            | "x-session-token"
            | "private-token"
            | "x-gitlab-token"
            | "x-amz-security-token"
            | "x-goog-api-key"
    )
}

/// Expand `${env:NAME}` placeholders so secrets can be referenced by name in
/// task sidecars and config files instead of being persisted as literal
/// values. Missing variables are an error rather than an empty header.
pub fn expand_env(value: &str) -> Result<String> {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${env:") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 6..];
        let Some(end) = after.find('}') else {
            return Err(Error::Config(
                "unterminated ${env:...} placeholder in header value".to_string(),
            ));
        };
        let name = after[..end].trim();
        let resolved = std::env::var(name)
            .map_err(|_| Error::Config(format!("environment variable is not set: {name}")))?;
        output.push_str(&resolved);
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

/// Authentication supplied for a task. Backends translate this into headers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Auth {
    /// Bearer-token authentication.
    Bearer {
        /// Token sent as the `Authorization: Bearer` credential.
        token: String,
    },
    /// HTTP Basic authentication.
    Basic {
        /// User name for the Basic credentials.
        username: String,
        /// Password for the Basic credentials.
        password: String,
    },
    /// An arbitrary header supplied as the credential, for schemes that do
    /// not fit the variants above.
    Header(Header),
}

impl Auth {
    /// Construct a [`Auth::Bearer`] from a token.
    pub fn bearer(token: impl Into<String>) -> Self {
        Self::Bearer {
            token: token.into(),
        }
    }

    /// Construct an [`Auth::Basic`] from a username and password.
    pub fn basic(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self::Basic {
            username: username.into(),
            password: password.into(),
        }
    }

    /// Short label naming the scheme without revealing its secret: `bearer`,
    /// `basic:<username>`, or `header:<name>`.
    pub fn describe(&self) -> String {
        match self {
            Auth::Bearer { .. } => "bearer".to_string(),
            Auth::Basic { username, .. } => format!("basic:{username}"),
            Auth::Header(header) => format!("header:{}", header.name),
        }
    }
}

/// Per-task request overrides. Headers, authentication and the cross-origin
/// policy are applied by every backend; `method` and `body` only by the direct
/// HTTP backend.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestOptions {
    /// Additional headers applied to every request of the task.
    #[serde(default)]
    pub headers: Vec<Header>,
    /// Authentication that backends translate into request headers.
    #[serde(default)]
    pub auth: Option<Auth>,
    /// Whether credential headers may follow redirects to another origin.
    /// Defaults to `false`, which strips them on cross-origin redirects.
    #[serde(default)]
    pub allow_cross_origin_auth: bool,
    /// HTTP method for direct HTTP(S) downloads. `None` means `GET`.
    ///
    /// Non-GET methods are handled by the direct HTTP backend only. The Hub,
    /// BitTorrent and FTP backends reject tasks that set a method or body. A
    /// request with a non-GET method or a body is sent as a single request:
    /// it is not segmented, does not resume and does not use mirror fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Optional request body for direct HTTP(S) downloads.
    ///
    /// A body without an explicit `method` is sent as `POST`. The body may
    /// contain `${env:NAME}` placeholders, which are expanded when the request
    /// is built. Like headers, it is persisted in the task sidecar (mode 0600
    /// on Unix) and redacted from `--json` output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

impl RequestOptions {
    /// Whether there is nothing to apply: no headers, auth, cross-origin
    /// policy, HTTP method or body.
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
            && self.auth.is_none()
            && !self.allow_cross_origin_auth
            && self.method.is_none()
            && self.body.is_none()
    }

    /// Merge global defaults with task overrides. Task headers win on name
    /// collisions (case-insensitive). Task-level auth wins over a default
    /// `Authorization` header, while an explicit task `Authorization` header
    /// wins over task-level auth.
    pub fn merged(&self, defaults: &[Header]) -> Self {
        let mut headers = defaults.to_vec();
        for header in &self.headers {
            headers.retain(|existing| !existing.name.eq_ignore_ascii_case(&header.name));
            headers.push(header.clone());
        }

        let task_supplied_authorization = self
            .headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("authorization"));
        let mut auth = self.auth.clone();
        if task_supplied_authorization {
            auth = None;
        }
        if auth.is_some() {
            // A task-level credential replaces a config default Authorization
            // header; otherwise the default would silently suppress the CLI
            // value and break the documented precedence.
            headers.retain(|header| !header.name.eq_ignore_ascii_case("authorization"));
        }

        Self {
            headers,
            auth,
            allow_cross_origin_auth: self.allow_cross_origin_auth,
            method: self.method.clone(),
            body: self.body.clone(),
        }
    }

    /// Header names that must never be forwarded across origins.
    pub fn sensitive_header_names(&self) -> BTreeSet<String> {
        self.headers
            .iter()
            .filter(|header| header.is_sensitive())
            .map(|header| header.name.to_ascii_lowercase())
            .chain(["authorization".to_string(), "cookie".to_string()])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_environment_placeholders() {
        let expanded = expand_env("prefix-${env:PATH}-suffix").unwrap();
        assert!(expanded.starts_with("prefix-"));
        assert!(expanded.ends_with("-suffix"));
        assert!(expand_env("${env:SHAUL_TEST_MISSING_VARIABLE}").is_err());
    }

    #[test]
    fn accepts_placeholder_header_values() {
        let header = Header::new("Authorization", "Bearer ${env:PATH}").unwrap();
        assert!(header.is_sensitive());
    }

    #[test]
    fn recognizes_common_credential_headers() {
        for name in [
            "Authorization",
            "Cookie",
            "Proxy-Authorization",
            "X-Api-Key",
            "Private-Token",
            "X-Auth-Token",
            "X-Gitlab-Token",
        ] {
            assert!(is_sensitive_header_name(name), "{name}");
        }
        assert!(!is_sensitive_header_name("Content-Type"));
        assert!(!is_sensitive_header_name("User-Agent"));
    }

    #[test]
    fn task_auth_overrides_a_default_authorization_header() {
        let defaults = vec![Header::new("Authorization", "Bearer config").unwrap()];
        let request = RequestOptions {
            auth: Some(Auth::bearer("cli")),
            ..RequestOptions::default()
        };
        let merged = request.merged(&defaults);
        assert!(merged.headers.is_empty());
        assert_eq!(merged.auth, Some(Auth::bearer("cli")));
    }

    #[test]
    fn explicit_task_authorization_wins_over_task_auth() {
        let request = RequestOptions {
            headers: vec![Header::new("Authorization", "Bearer header").unwrap()],
            auth: Some(Auth::bearer("cli")),
            ..RequestOptions::default()
        };
        let merged = request.merged(&[]);
        assert_eq!(merged.auth, None);
        assert_eq!(merged.headers.len(), 1);
        assert_eq!(merged.headers[0].value, "Bearer header");
    }

    #[test]
    fn default_authorization_is_inherited_without_task_credentials() {
        let defaults = vec![Header::new("Authorization", "Bearer config").unwrap()];
        let merged = RequestOptions::default().merged(&defaults);
        assert_eq!(merged.auth, None);
        assert_eq!(merged.headers.len(), 1);
        assert_eq!(merged.headers[0].value, "Bearer config");
    }

    #[test]
    fn merging_defaults_preserves_http_method_and_body() {
        let request = RequestOptions {
            method: Some("POST".to_string()),
            body: Some("{\"key\":\"${env:VALUE}\"}".to_string()),
            ..RequestOptions::default()
        };
        let merged = request.merged(&[Header::new("X-Test", "yes").unwrap()]);
        assert_eq!(merged.method.as_deref(), Some("POST"));
        assert_eq!(merged.body, request.body);
    }

    #[test]
    fn missing_method_and_body_default_to_none() {
        let options: RequestOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(options, RequestOptions::default());
        let encoded = serde_json::to_string(&RequestOptions::default()).unwrap();
        assert!(!encoded.contains("method"), "{encoded}");
        assert!(!encoded.contains("body"), "{encoded}");
    }

    #[test]
    fn empty_options_include_http_method_and_body() {
        assert!(RequestOptions::default().is_empty());
        assert!(
            !RequestOptions {
                method: Some("GET".to_string()),
                ..RequestOptions::default()
            }
            .is_empty()
        );
        assert!(
            !RequestOptions {
                body: Some("payload".to_string()),
                ..RequestOptions::default()
            }
            .is_empty()
        );
    }
}
