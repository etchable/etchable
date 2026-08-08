//! EasyEDA component endpoints — the two-call uuid route.
//!
//! `searchByNumbers` maps C-numbers to `{uuid, puuid, step}` in one batched
//! POST; `GET /api/components/{uuid}` returns the full component document
//! (symbol `dataStr` + nested `packageDetail` footprint) byte-identical to
//! the WAF-banned `/api/products/{C#}/components` route. The `step` uuid
//! from `searchByNumbers` matches the SVGNODE record's `attrs.uuid` and is
//! the ONLY downloadable 3D uuid — `head.uuid_3d` 404s.

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cache::Cache;
use crate::http::{easyeda_api_base, modules_bases, step_bucket_prefix, Client};
use crate::FetchError;

/// User decision: 3D models over 8 MB are skipped (a missing model is inert).
pub const MODEL_3D_CAP: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumberEntry {
    pub number: String,
    /// Symbol/component document uuid.
    pub uuid: String,
    /// Footprint/package document uuid.
    #[serde(default)]
    pub puuid: Option<String>,
    /// 3D model uuid — free here, no SVGNODE parsing needed.
    #[serde(default)]
    pub step: Option<String>,
}

/// C-numbers -> uuids. Body is `numbers=<urlencoded JSON array>`, NOT a
/// JSON body.
pub async fn search_by_numbers(
    client: &Client,
    cache: &Cache,
    numbers: &[&str],
) -> Result<Vec<NumberEntry>, FetchError> {
    let mut out = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    for n in numbers {
        match cache.get("numbers", n) {
            Some(hit) => match serde_json::from_slice::<NumberEntry>(&hit.bytes) {
                Ok(e) => out.push(e),
                Err(_) => missing.push(n),
            },
            None => missing.push(n),
        }
    }
    if missing.is_empty() {
        return Ok(out);
    }

    let payload = serde_json::to_string(&missing).expect("string array");
    let body = format!("numbers={}", urlencode(&payload));
    let bytes = client
        .request(
            &format!("{}/components/searchByNumbers", easyeda_api_base()),
            Some((
                "application/x-www-form-urlencoded; charset=UTF-8",
                body.into_bytes(),
            )),
        )
        .await?;
    let v: Value = serde_json::from_slice(&bytes).context("searchByNumbers: bad JSON")?;
    if v.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!("searchByNumbers failed: {v}").into());
    }
    let entries: Vec<NumberEntry> = serde_json::from_value(
        v.get("result").cloned().unwrap_or(Value::Array(vec![])),
    )
    .context("searchByNumbers: unexpected result shape")?;
    for e in &entries {
        let _ = cache.put("numbers", &e.number, serde_json::to_string(e).unwrap().as_bytes());
        out.push(e.clone());
    }
    Ok(out)
}

/// Full component document (`result`: symbol dataStr + packageDetail).
/// uuid-addressed and cached as immutable.
pub async fn component(client: &Client, cache: &Cache, uuid: &str) -> Result<Value, FetchError> {
    if let Some(hit) = cache.get("docs", uuid) {
        if let Ok(v) = serde_json::from_slice::<Value>(&hit.bytes) {
            return Ok(v);
        }
    }
    let bytes = client
        .request(&format!("{}/components/{uuid}", easyeda_api_base()), None)
        .await?;
    let v: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("component {uuid}: bad JSON"))?;
    if v.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(FetchError::NotFound(format!("EasyEDA component {uuid}")));
    }
    let result = v
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("component {uuid}: no result"))?;
    let _ = cache.put("docs", uuid, result.to_string().as_bytes());
    Ok(result)
}

/// STEP model bytes (mm, no scaling needed). Mirror failover; 8 MB cap
/// (over-cap = `Ok(None)` + warn, never an error). A bad uuid yields an OSS
/// XML `NoSuchKey` body, mapped to NotFound.
pub async fn model_step(
    client: &Client,
    cache: &Cache,
    uuid: &str,
) -> Result<Option<Vec<u8>>, FetchError> {
    let key = format!("{uuid}.step");
    if let Some(hit) = cache.get("models", &key) {
        return Ok(Some(hit.bytes));
    }
    let prefix = step_bucket_prefix();
    let mut last_err: Option<FetchError> = None;
    for base in modules_bases() {
        match client.request(&format!("{base}/{prefix}/{uuid}"), None).await {
            Ok(bytes) => {
                if looks_like_oss_error(&bytes) {
                    return Err(FetchError::NotFound(format!("3D model {uuid}")));
                }
                if bytes.len() > MODEL_3D_CAP {
                    tracing::warn!(
                        "3D model {uuid} is {} bytes (cap {}); skipping",
                        bytes.len(),
                        MODEL_3D_CAP
                    );
                    return Ok(None);
                }
                let _ = cache.put("models", &key, &bytes);
                return Ok(Some(bytes));
            }
            Err(e @ (FetchError::Offline | FetchError::Blocked { .. })) => return Err(e),
            Err(e) => {
                tracing::debug!("3D fetch from {base} failed: {e}; trying mirror");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("no module hosts configured").into()))
}

/// Aliyun OSS answers a bad key with 200 + XML `<Error><Code>NoSuchKey</Code>`.
fn looks_like_oss_error(bytes: &[u8]) -> bool {
    bytes.len() < 4096
        && bytes.starts_with(b"<?xml")
        && bytes.windows(9).any(|w| w == b"NoSuchKey")
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_matches_form_encoding() {
        assert_eq!(urlencode(r#"["C2040"]"#), "%5B%22C2040%22%5D");
    }

    #[test]
    fn oss_error_detection() {
        assert!(looks_like_oss_error(
            b"<?xml version=\"1.0\"?><Error><Code>NoSuchKey</Code></Error>"
        ));
        assert!(!looks_like_oss_error(b"ISO-10303-21;\nHEADER;"));
    }
}
