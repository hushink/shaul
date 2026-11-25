use std::time::Duration;

use base64::Engine;
use reqwest::header::{AUTHORIZATION, COOKIE, HeaderMap, HeaderName, HeaderValue, LOCATION};
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use shaul_core::{Auth, Config, Error, ProxySetting, RequestOptions, Result, expand_env};
use tokio_util::sync::CancellationToken;
use url::Url;

const MAX_REDIRECTS: usize = 10;
const MAX_MPD_BYTES: usize = 16 * 1024 * 1024;

enum FetchError {
    Retryable(Error),
    Fatal(Error),
}

/// HTTP client used to fetch DASH MPD documents.
#[derive(Clone)]
pub struct DashClient {
    client: Client,
    timeout: Duration,
    retries: u32,
}

impl DashClient {
    /// Build a client from the proxy, timeout and user-agent settings in
    /// `config`.
    pub fn new(config: &Config) -> Result<Self> {
        let mut builder = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(
                config.network.connect_timeout_secs.max(1),
            ))
            .user_agent(config.network.user_agent.clone());
        if let Some(proxy) = proxy_from_config(config)? {
            builder = builder.proxy(proxy);
        } else if matches!(config.proxy_setting(), ProxySetting::Disabled) {
            builder = builder.no_proxy();
        }
        let client = builder
            .build()
            .map_err(|error| Error::Backend(format!("failed to build DASH client: {error}")))?;
        Ok(Self {
            client,
            timeout: Duration::from_secs(config.network.timeout_secs.max(1)),
            retries: config.network.retries,
        })
    }

    /// Fetch an MPD document with the task's headers and authentication.
    /// Returns the body and the final URL after redirects, which is the base
    /// for relative BaseURLs.
    pub async fn fetch_text(
        &self,
        url: &Url,
        request: &RequestOptions,
        cancel: &CancellationToken,
    ) -> Result<(String, Url)> {
        let (bytes, final_url) = self.fetch_body(url, request, cancel).await?;
        let text = String::from_utf8(bytes)
            .map_err(|_| Error::Backend("DASH manifest is not valid UTF-8".to_string()))?;
        Ok((text, final_url))
    }

    async fn fetch_body(
        &self,
        url: &Url,
        request: &RequestOptions,
        cancel: &CancellationToken,
    ) -> Result<(Vec<u8>, Url)> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::Backend(format!(
                "DASH URL must use http or https: {url}"
            )));
        }
        let mut attempt = 0u32;
        loop {
            match self.fetch_once(url, request, cancel).await {
                Ok(result) => return Ok(result),
                Err(FetchError::Fatal(error)) => return Err(error),
                Err(FetchError::Retryable(error)) => {
                    if attempt >= self.retries {
                        return Err(error);
                    }
                    tracing::warn!(attempt, error = %error, "DASH request failed; retrying");
                    let delay = Duration::from_millis(200u64.saturating_mul(1 << attempt.min(5)));
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(Error::Cancelled),
                        _ = tokio::time::sleep(delay) => {}
                    }
                    attempt += 1;
                }
            }
        }
    }

    async fn fetch_once(
        &self,
        url: &Url,
        request: &RequestOptions,
        cancel: &CancellationToken,
    ) -> std::result::Result<(Vec<u8>, Url), FetchError> {
        let mut current = url.clone();
        let mut headers = build_headers(request).map_err(FetchError::Fatal)?;
        for _ in 0..MAX_REDIRECTS {
            let builder = self.client.get(current.clone()).headers(headers.clone());
            let response = tokio::select! {
                _ = cancel.cancelled() => return Err(FetchError::Fatal(Error::Cancelled)),
                result = tokio::time::timeout(self.timeout, builder.send()) => match result {
                    Ok(response) => response.map_err(|error| {
                        FetchError::Retryable(Error::Backend(format!(
                            "DASH request failed: {}",
                            error.without_url()
                        )))
                    })?,
                    Err(_) => {
                        return Err(FetchError::Retryable(Error::Backend(
                            "DASH request timed out while waiting for response headers"
                                .to_string(),
                        )));
                    }
                },
            };
            let status = response.status();
            if matches!(
                status,
                StatusCode::MOVED_PERMANENTLY
                    | StatusCode::FOUND
                    | StatusCode::SEE_OTHER
                    | StatusCode::TEMPORARY_REDIRECT
                    | StatusCode::PERMANENT_REDIRECT
            ) {
                let Some(location) = response.headers().get(LOCATION) else {
                    return Err(FetchError::Fatal(Error::Backend(
                        "DASH redirect response is missing Location".to_string(),
                    )));
                };
                let location = location.to_str().map_err(|_| {
                    FetchError::Fatal(Error::Backend(
                        "DASH redirect Location is not valid UTF-8".to_string(),
                    ))
                })?;
                let next = current.join(location).map_err(|error| {
                    FetchError::Fatal(Error::Backend(format!(
                        "invalid DASH redirect URL: {error}"
                    )))
                })?;
                if !same_origin(&current, &next) && !request.allow_cross_origin_auth {
                    for name in request.sensitive_header_names() {
                        headers.remove(name.as_str());
                    }
                    headers.remove(AUTHORIZATION);
                    headers.remove(COOKIE);
                }
                current = next;
                continue;
            }
            if status.is_server_error()
                || status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::TOO_MANY_REQUESTS
            {
                return Err(FetchError::Retryable(Error::Backend(format!(
                    "HTTP {status}"
                ))));
            }
            if !status.is_success() {
                return Err(FetchError::Fatal(Error::Backend(format!("HTTP {status}"))));
            }
            if let Some(length) = response.content_length()
                && length > MAX_MPD_BYTES as u64
            {
                return Err(FetchError::Fatal(Error::Backend(format!(
                    "DASH manifest is too large: {length} bytes"
                ))));
            }
            let bytes = tokio::select! {
                _ = cancel.cancelled() => return Err(FetchError::Fatal(Error::Cancelled)),
                result = tokio::time::timeout(self.timeout, response.bytes()) => match result {
                    Ok(bytes) => bytes.map_err(|error| {
                        FetchError::Retryable(Error::Backend(format!(
                            "could not read DASH response: {}",
                            error.without_url()
                        )))
                    })?,
                    Err(_) => {
                        return Err(FetchError::Retryable(Error::Backend(
                            "DASH request timed out while reading the response".to_string(),
                        )));
                    }
                },
            };
            if bytes.len() > MAX_MPD_BYTES {
                return Err(FetchError::Fatal(Error::Backend(format!(
                    "DASH manifest is too large: {} bytes",
                    bytes.len()
                ))));
            }
            return Ok((bytes.to_vec(), current));
        }
        Err(FetchError::Fatal(Error::Backend(
            "too many DASH redirects".to_string(),
        )))
    }
}

fn build_headers(request: &RequestOptions) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for header in &request.headers {
        if !is_reserved(&header.name) {
            insert_header(&mut headers, &header.name, &expand_env(&header.value)?)?;
        }
    }
    if !headers.contains_key(AUTHORIZATION) {
        match &request.auth {
            Some(Auth::Bearer { token }) => {
                insert_header(
                    &mut headers,
                    "authorization",
                    &format!("Bearer {}", expand_env(token)?),
                )?;
            }
            Some(Auth::Basic { username, password }) => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(format!(
                    "{}:{}",
                    expand_env(username)?,
                    expand_env(password)?
                ));
                insert_header(&mut headers, "authorization", &format!("Basic {encoded}"))?;
            }
            Some(Auth::Header(header)) => {
                insert_header(&mut headers, &header.name, &expand_env(&header.value)?)?;
            }
            None => {}
        }
    }
    Ok(headers)
}

fn insert_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<()> {
    let name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| Error::Config(format!("invalid header name: {name}")))?;
    let value = HeaderValue::from_str(value)
        .map_err(|_| Error::Config(format!("invalid value for header: {name}")))?;
    headers.insert(name, value);
    Ok(())
}

fn is_reserved(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "range"
            | "if-range"
            | "host"
            | "content-length"
            | "accept-encoding"
            | "connection"
            | "transfer-encoding"
    )
}

pub(crate) fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn proxy_from_config(config: &Config) -> Result<Option<reqwest::Proxy>> {
    let ProxySetting::Url(url) = config.proxy_setting() else {
        return Ok(None);
    };
    let mut proxy = reqwest::Proxy::all(&url)
        .map_err(|error| Error::Config(format!("invalid proxy URL: {error}")))?;
    if let Some(value) = config.network.no_proxy.as_deref()
        && let Some(no_proxy) = reqwest::NoProxy::from_string(value)
    {
        proxy = proxy.no_proxy(Some(no_proxy));
    }
    Ok(Some(proxy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaul_core::Header;

    #[test]
    fn builds_bearer_and_basic_headers() {
        let request = RequestOptions {
            auth: Some(Auth::bearer("secret")),
            ..RequestOptions::default()
        };
        let headers = build_headers(&request).unwrap();
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer secret");

        let request = RequestOptions {
            auth: Some(Auth::basic("user", "pass")),
            ..RequestOptions::default()
        };
        let headers = build_headers(&request).unwrap();
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Basic dXNlcjpwYXNz");
    }

    #[test]
    fn task_authorization_header_wins_over_auth() {
        let request = RequestOptions {
            headers: vec![Header::new("Authorization", "Bearer header").unwrap()],
            auth: Some(Auth::bearer("auth")),
            ..RequestOptions::default()
        };
        let headers = build_headers(&request).unwrap();
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer header");
    }

    #[test]
    fn compares_origins_including_the_default_port() {
        let left = Url::parse("https://example.com/a").unwrap();
        let right = Url::parse("https://example.com:443/b").unwrap();
        let other = Url::parse("https://cdn.example.com/b").unwrap();
        assert!(same_origin(&left, &right));
        assert!(!same_origin(&left, &other));
    }
}
