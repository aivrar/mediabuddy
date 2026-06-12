//! API-key probes for the three stock providers.
//!
//! Each probe runs a minimal query (1 result) and reports whether the key
//! is accepted, plus any rate-limit headers the provider exposes. Used by
//! the Settings tab "Test" buttons.

use std::sync::Arc;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::quota::{QuotaTracker, Source as QuotaSource};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Pixabay,
    Pexels,
    Unsplash,
}

impl Provider {
    fn as_quota(self) -> QuotaSource {
        match self {
            Provider::Pixabay => QuotaSource::Pixabay,
            Provider::Pexels => QuotaSource::Pexels,
            Provider::Unsplash => QuotaSource::Unsplash,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct KeyProbe {
    pub provider: String,
    pub valid: bool,
    pub status_code: Option<u16>,
    pub message: String,
    pub rate_limit: Option<u64>,
    pub rate_remaining: Option<u64>,
    pub reset_seconds: Option<u64>,
}

impl KeyProbe {
    fn err(provider: &str, message: impl Into<String>) -> Self {
        Self {
            provider: provider.to_string(),
            valid: false,
            status_code: None,
            message: message.into(),
            rate_limit: None,
            rate_remaining: None,
            reset_seconds: None,
        }
    }
}

pub async fn probe(
    client: &Client,
    provider: Provider,
    key: &str,
    tracker: Option<&Arc<QuotaTracker>>,
) -> KeyProbe {
    let key = key.trim();
    if key.is_empty() {
        return KeyProbe::err(name(provider), "Key is empty.");
    }
    let result = match provider {
        Provider::Pixabay => probe_pixabay(client, key, tracker).await,
        Provider::Pexels => probe_pexels(client, key, tracker).await,
        Provider::Unsplash => probe_unsplash(client, key, tracker).await,
    };
    let _ = provider.as_quota(); // suppress unused-import for non-tracker callers
    result
}

fn name(p: Provider) -> &'static str {
    match p {
        Provider::Pixabay => "Pixabay",
        Provider::Pexels => "Pexels",
        Provider::Unsplash => "Unsplash",
    }
}

fn header_u64(resp: &reqwest::Response, key: &str) -> Option<u64> {
    resp.headers()
        .get(key)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse().ok())
}

/// Truncate a provider response body for display, on a CHARACTER boundary.
/// (Byte-slicing `&s[..200]` panics if byte 200 splits a multi-byte UTF-8
/// codepoint — reachable from an attacker- or MITM-controlled response.)
fn clip(s: &str) -> String {
    const MAX_CHARS: usize = 200;
    let mut out: String = s.chars().take(MAX_CHARS).collect();
    if s.chars().count() > MAX_CHARS {
        out.push('…');
    }
    out
}

fn provider_error(body: &Value) -> Option<String> {
    body.get("error")
        .or_else(|| body.get("errors"))
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Array(arr) => Some(
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Value::Object(_) => Some(v.to_string()),
            _ => None,
        })
        .filter(|s| !s.trim().is_empty())
}

fn invalid_success_shape_probe(
    provider: &str,
    code: u16,
    body: &Value,
    rate_limit: Option<u64>,
    rate_remaining: Option<u64>,
    reset_seconds: Option<u64>,
) -> KeyProbe {
    let detail = provider_error(body).unwrap_or_else(|| "unexpected provider response".into());
    KeyProbe {
        provider: provider.into(),
        valid: false,
        status_code: Some(code),
        message: format!(
            "HTTP {code}, but not a valid {provider} API response: {}",
            clip(&detail)
        ),
        rate_limit,
        rate_remaining,
        reset_seconds,
    }
}

// ---------- Pixabay ----------
//
// Pixabay returns 200 on success and "[ERROR 400] ..." in the body on auth
// failure. Rate-limit headers: X-RateLimit-Limit / -Remaining / -Reset.
async fn probe_pixabay(
    client: &Client,
    key: &str,
    tracker: Option<&Arc<QuotaTracker>>,
) -> KeyProbe {
    let url = "https://pixabay.com/api/";
    let req = client
        .get(url)
        .query(&[("key", key), ("q", "test"), ("per_page", "3")])
        .timeout(std::time::Duration::from_secs(10));
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return KeyProbe::err("Pixabay", format!("Network error: {e}")),
    };
    if let Some(t) = tracker {
        t.record(QuotaSource::Pixabay, &resp);
    }
    let status = resp.status();
    let limit = header_u64(&resp, "x-ratelimit-limit");
    let remaining = header_u64(&resp, "x-ratelimit-remaining");
    let reset = header_u64(&resp, "x-ratelimit-reset");

    if status.is_success() {
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if body.get("hits").and_then(|v| v.as_array()).is_none()
            || body.get("total").and_then(|v| v.as_u64()).is_none()
        {
            return invalid_success_shape_probe(
                "Pixabay",
                status.as_u16(),
                &body,
                limit,
                remaining,
                reset,
            );
        }
        let total = body.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
        return KeyProbe {
            provider: "Pixabay".into(),
            valid: true,
            status_code: Some(status.as_u16()),
            message: format!("OK · {total} total hits for \"test\""),
            rate_limit: limit,
            rate_remaining: remaining,
            reset_seconds: reset,
        };
    }

    let code = status.as_u16();
    let body_txt = resp.text().await.unwrap_or_default();
    let trimmed = body_txt.trim();
    let msg = if trimmed.is_empty() {
        format!("HTTP {code}")
    } else {
        format!("HTTP {code}: {}", clip(trimmed))
    };
    KeyProbe {
        provider: "Pixabay".into(),
        valid: false,
        status_code: Some(code),
        message: msg,
        rate_limit: limit,
        rate_remaining: remaining,
        reset_seconds: reset,
    }
}

// ---------- Pexels ----------
//
// Pexels uses raw `Authorization: <KEY>` (no scheme prefix). 401 on
// invalid. Rate-limit: X-Ratelimit-Limit / -Remaining / -Reset (epoch).
async fn probe_pexels(client: &Client, key: &str, tracker: Option<&Arc<QuotaTracker>>) -> KeyProbe {
    let url = "https://api.pexels.com/v1/search";
    let req = client
        .get(url)
        .header("Authorization", key)
        .query(&[("query", "test"), ("per_page", "1")])
        .timeout(std::time::Duration::from_secs(10));
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return KeyProbe::err("Pexels", format!("Network error: {e}")),
    };
    if let Some(t) = tracker {
        t.record(QuotaSource::Pexels, &resp);
    }
    let status = resp.status();
    let limit = header_u64(&resp, "x-ratelimit-limit");
    let remaining = header_u64(&resp, "x-ratelimit-remaining");
    let reset = header_u64(&resp, "x-ratelimit-reset");

    if status.is_success() {
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if body.get("photos").and_then(|v| v.as_array()).is_some()
            && body.get("page").and_then(|v| v.as_u64()).is_some()
            && body.get("per_page").and_then(|v| v.as_u64()).is_some()
        {
            let count = body
                .get("photos")
                .and_then(|v| v.as_array())
                .map(|v| v.len())
                .unwrap_or(0);
            return KeyProbe {
                provider: "Pexels".into(),
                valid: true,
                status_code: Some(status.as_u16()),
                message: format!("OK - {count} photo result(s) for \"test\""),
                rate_limit: limit,
                rate_remaining: remaining,
                reset_seconds: reset,
            };
        }
        return invalid_success_shape_probe(
            "Pexels",
            status.as_u16(),
            &body,
            limit,
            remaining,
            reset,
        );
    }

    let code = status.as_u16();
    let body_txt = resp.text().await.unwrap_or_default();
    let trimmed = body_txt.trim();
    let msg = match code {
        401 => "Invalid key (HTTP 401 Unauthorized)".to_string(),
        429 => "Rate limit exceeded (HTTP 429)".to_string(),
        _ if trimmed.is_empty() => format!("HTTP {code}"),
        _ => format!("HTTP {code}: {}", clip(trimmed)),
    };
    KeyProbe {
        provider: "Pexels".into(),
        valid: false,
        status_code: Some(code),
        message: msg,
        rate_limit: limit,
        rate_remaining: remaining,
        reset_seconds: reset,
    }
}

// ---------- Unsplash ----------
//
// Auth: `Authorization: Client-ID <KEY>`. 401 on invalid.
// Rate-limit: X-Ratelimit-Limit / -Remaining (per-hour, no reset header).
async fn probe_unsplash(
    client: &Client,
    key: &str,
    tracker: Option<&Arc<QuotaTracker>>,
) -> KeyProbe {
    let url = "https://api.unsplash.com/photos";
    let auth = format!("Client-ID {key}");
    let req = client
        .get(url)
        .header("Authorization", &auth)
        .query(&[("per_page", "1")])
        .timeout(std::time::Duration::from_secs(10));
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return KeyProbe::err("Unsplash", format!("Network error: {e}")),
    };
    if let Some(t) = tracker {
        t.record(QuotaSource::Unsplash, &resp);
    }
    let status = resp.status();
    let limit = header_u64(&resp, "x-ratelimit-limit");
    let remaining = header_u64(&resp, "x-ratelimit-remaining");

    if status.is_success() {
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if body
            .as_array()
            .map(|photos| {
                photos.iter().all(|photo| {
                    photo.get("id").and_then(|v| v.as_str()).is_some()
                        && photo.get("urls").and_then(|v| v.as_object()).is_some()
                })
            })
            .unwrap_or(false)
        {
            return KeyProbe {
                provider: "Unsplash".into(),
                valid: true,
                status_code: Some(status.as_u16()),
                message: "OK".into(),
                rate_limit: limit,
                rate_remaining: remaining,
                reset_seconds: None,
            };
        }
        return invalid_success_shape_probe(
            "Unsplash",
            status.as_u16(),
            &body,
            limit,
            remaining,
            None,
        );
    }

    let code = status.as_u16();
    let body_txt = resp.text().await.unwrap_or_default();
    let trimmed = body_txt.trim();
    let msg = match code {
        401 => "Invalid key (HTTP 401 Unauthorized)".to_string(),
        403 => "Forbidden — key valid but missing scope or quota exceeded".to_string(),
        429 => "Rate limit exceeded (HTTP 429)".to_string(),
        _ if trimmed.is_empty() => format!("HTTP {code}"),
        _ => format!("HTTP {code}: {}", clip(trimmed)),
    };
    KeyProbe {
        provider: "Unsplash".into(),
        valid: false,
        status_code: Some(code),
        message: msg,
        rate_limit: limit,
        rate_remaining: remaining,
        reset_seconds: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn invalid_success_shape_marks_wrong_provider_payload_invalid() {
        let pixabay_body = json!({
            "total": 10,
            "totalHits": 10,
            "hits": []
        });
        let probe = invalid_success_shape_probe("Pexels", 200, &pixabay_body, None, None, None);
        assert!(!probe.valid);
        assert!(probe.message.contains("not a valid Pexels API response"));
    }

    #[test]
    fn provider_error_extracts_provider_error_messages() {
        assert_eq!(
            provider_error(&json!({ "error": "Authorization failed" })).as_deref(),
            Some("Authorization failed")
        );
        assert_eq!(
            provider_error(&json!({ "errors": ["bad key", "bad scope"] })).as_deref(),
            Some("bad key, bad scope")
        );
    }
}
