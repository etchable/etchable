//! The fetch/convert boundary. [`fetch_part`] is the only function that
//! composes network calls; it returns a [`RawPart`] of unparsed upstream
//! payloads. Conversion is pure — [`RawPart::from_parts`] builds fixtures
//! for tests that never touch the wire.

use serde_json::Value;

use crate::cache::Cache;
use crate::easyeda;
use crate::http::Client;
use crate::jlc;
use crate::FetchError;

/// Unparsed upstream payloads for one LCSC part.
#[derive(Debug, Clone)]
pub struct RawPart {
    /// The C-number this was fetched by.
    pub lcsc: String,
    /// EasyEDA component `result`: symbol `dataStr` + nested `packageDetail`.
    pub component: Value,
    /// JLC detail `data` (ref prefix, MSL, assembly info); absent when the
    /// JLC endpoint failed — it is enrichment, not a requirement.
    pub jlc_detail: Option<Value>,
    /// STEP bytes; None = not requested, over the 8 MB cap, or upstream
    /// had none.
    pub step: Option<Vec<u8>>,
    /// The 3D model uuid from `searchByNumbers` (the downloadable one).
    pub step_uuid: Option<String>,
    /// Unix seconds at fetch time.
    pub fetched_at: u64,
}

impl RawPart {
    /// Fixture constructor — conversion tests build parts from checked-in
    /// JSON with zero network.
    pub fn from_parts(
        lcsc: &str,
        component: Value,
        jlc_detail: Option<Value>,
        step: Option<Vec<u8>>,
    ) -> Self {
        Self {
            lcsc: lcsc.to_string(),
            component,
            jlc_detail,
            step,
            step_uuid: None,
            fetched_at: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FetchOptions {
    pub include_3d: bool,
}

pub async fn fetch_part(
    client: &Client,
    cache: &Cache,
    lcsc: &str,
    opts: &FetchOptions,
) -> Result<RawPart, FetchError> {
    let entries = easyeda::api::search_by_numbers(client, cache, &[lcsc]).await?;
    let entry = entries
        .into_iter()
        .find(|e| e.number.eq_ignore_ascii_case(lcsc))
        .ok_or_else(|| FetchError::NotFound(format!("LCSC part {lcsc} has no EasyEDA data")))?;

    let component = easyeda::api::component(client, cache, &entry.uuid).await?;

    // Enrichment only: a JLC hiccup must not fail the part.
    let jlc_detail = match jlc::detail(client, cache, lcsc).await {
        Ok(v) => Some(v),
        Err(FetchError::Offline) | Err(FetchError::Blocked { .. }) => None,
        Err(e) => {
            tracing::warn!("jlc detail for {lcsc} failed: {e}");
            None
        }
    };

    let step = match (&entry.step, opts.include_3d) {
        (Some(uuid), true) => match easyeda::api::model_step(client, cache, uuid).await {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!("3D model for {lcsc} failed: {e}");
                None
            }
        },
        _ => None,
    };

    Ok(RawPart {
        lcsc: lcsc.to_string(),
        component,
        jlc_detail,
        step,
        step_uuid: entry.step,
        fetched_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    })
}
