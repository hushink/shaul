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

    /// Whether the header is one of the credential-bearing names
    /// (`authorization`, `cookie`, `proxy-authorization`, `x-api-key`),
    /// compared case-insensitively.
    pub fn is_sensitive(&self) -> bool {
        let name = self.name.to_ascii_lowercase();
        matches!(
            name.as_str(),
            "authorization" | "cookie" | "proxy-authorization" | "x-api-key"
        )
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

/// Per-task request overrides applied by every backend.
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
}

impl RequestOptions {
    /// Whether there is nothing to apply: no headers, no auth, and
    /// cross-origin auth not enabled.
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.auth.is_none() && !self.allow_cross_origin_auth
    }

    /// Merge global defaults with task overrides. Task headers win on name
    /// collisions (case-insensitive). Auth becomes an `Authorization` header
    /// unless the user already supplied one explicitly.
    pub fn merged(&self, defaults: &[Header]) -> Self {
        let mut headers = defaults.to_vec();
        for header in &self.headers {
            headers.retain(|existing| !existing.name.eq_ignore_ascii_case(&header.name));
            headers.push(header.clone());
        }

        let mut auth = self.auth.clone();
        let has_authorization = headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("authorization"));
        if has_authorization {
            auth = None;
        }

        Self {
            headers,
            auth,
            allow_cross_origin_auth: self.allow_cross_origin_auth,
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
}
