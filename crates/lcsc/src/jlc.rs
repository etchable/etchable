//! JLCPCB parts search + detail — anonymous, unthrottled, and it *is* the
//! assembly library we target. Search results carry stock, price ladder,
//! datasheet URL, and Basic-vs-Extended class.

use anyhow::{anyhow, Context};
use serde::Serialize;
use serde_json::{json, Value};

use crate::cache::Cache;
use crate::http::{jlc_detail_url, jlc_search_url, Client};
use crate::FetchError;

pub const PAGE_SIZE: u32 = 25;

/// One search hit, normalized for tool payloads.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub lcsc: String,
    pub mpn: String,
    pub manufacturer: String,
    pub package: String,
    pub description: String,
    /// `basic` or `extended` — extended parts carry a JLC setup fee.
    pub class: String,
    pub preferred: bool,
    pub assembly: bool,
    pub stock: i64,
    pub min_qty: i64,
    /// Unit price at the lowest break, USD.
    pub unit_price: Option<f64>,
    pub datasheet: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchPage {
    pub total: i64,
    pub hits: Vec<SearchHit>,
    /// Unix seconds when this payload was fetched (or originally cached).
    pub as_of: u64,
    pub cached: bool,
}

/// `componentModelHigh` etc. wrap matches in
/// `<span class='lucene_highlight_class'>…</span>` — strip any tags.
pub fn strip_highlight(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn stored_secs(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn parse_hit(item: &Value) -> Option<SearchHit> {
    let s = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let lcsc = s("componentCode");
    if lcsc.is_empty() {
        return None;
    }
    let class = match item.get("componentLibraryType").and_then(Value::as_str) {
        Some("base") => "basic",
        Some("expand") => "extended",
        _ => "unknown",
    };
    let unit_price = item
        .get("componentPrices")
        .and_then(Value::as_array)
        .and_then(|prices| {
            prices
                .iter()
                .filter_map(|p| p.get("productPrice").and_then(Value::as_f64))
                .fold(None, |min: Option<f64>, p| {
                    Some(min.map_or(p, |m| m.min(p)))
                })
        });
    Some(SearchHit {
        lcsc,
        mpn: strip_highlight(&s("componentModelEn")),
        manufacturer: strip_highlight(&s("componentBrandEn")),
        package: s("componentSpecificationEn"),
        description: {
            let mut d = strip_highlight(&s("describe"));
            d.truncate(160);
            d
        },
        class: class.to_string(),
        preferred: item
            .get("preferredComponentFlag")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        assembly: item
            .get("assemblyComponentFlag")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        stock: item.get("stockCount").and_then(Value::as_i64).unwrap_or(0),
        min_qty: item.get("minPurchaseNum").and_then(Value::as_i64).unwrap_or(1),
        unit_price,
        datasheet: item
            .get("dataManualUrl")
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty())
            .map(String::from),
    })
}

/// Parse the search envelope `{code, data:{componentPageInfo:{total, list}}}`.
pub fn parse_search_page(bytes: &[u8], as_of: u64, cached: bool) -> anyhow::Result<SearchPage> {
    let v: Value = serde_json::from_slice(bytes).context("jlc search: bad JSON")?;
    let info = v
        .pointer("/data/componentPageInfo")
        .ok_or_else(|| anyhow!("jlc search: missing componentPageInfo"))?;
    let hits = info
        .get("list")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(parse_hit).collect())
        .unwrap_or_default();
    Ok(SearchPage {
        total: info.get("total").and_then(Value::as_i64).unwrap_or(0),
        hits,
        as_of,
        cached,
    })
}

pub async fn search(
    client: &Client,
    cache: &Cache,
    keyword: &str,
) -> Result<SearchPage, FetchError> {
    let key = keyword.to_ascii_lowercase();
    if let Some(hit) = cache.get("search", &key) {
        if let Ok(page) = parse_search_page(&hit.bytes, stored_secs(hit.stored_at), true) {
            return Ok(page);
        }
    }
    let body = json!({"keyword": keyword, "currentPage": 1, "pageSize": PAGE_SIZE});
    let bytes = match client
        .request(&jlc_search_url(), Some(("application/json", body.to_string().into_bytes())))
        .await
    {
        Ok(b) => b,
        Err(e) => {
            // Network trouble: serve stale rather than nothing.
            if let Some(hit) = cache.get_stale("search", &key) {
                if let Ok(page) = parse_search_page(&hit.bytes, stored_secs(hit.stored_at), true) {
                    tracing::warn!("jlc search failed ({e}); serving stale cache");
                    return Ok(page);
                }
            }
            return Err(e);
        }
    };
    let page = parse_search_page(&bytes, stored_secs(std::time::SystemTime::now()), false)
        .map_err(FetchError::Other)?;
    let _ = cache.put("search", &key, &bytes);
    Ok(page)
}

/// Part detail (GET — POST returns 405). Adds ref-prefix (`componentDesignator`),
/// assembly process, MSL, and status over the search hit.
pub async fn detail(client: &Client, cache: &Cache, lcsc: &str) -> Result<Value, FetchError> {
    if let Some(hit) = cache.get("jlc", lcsc) {
        if let Ok(v) = serde_json::from_slice::<Value>(&hit.bytes) {
            return Ok(v);
        }
    }
    let url = format!("{}?componentCode={lcsc}", jlc_detail_url());
    let bytes = match client.request(&url, None).await {
        Ok(b) => b,
        Err(e) => {
            if let Some(hit) = cache.get_stale("jlc", lcsc) {
                if let Ok(v) = serde_json::from_slice::<Value>(&hit.bytes) {
                    tracing::warn!("jlc detail failed ({e}); serving stale cache");
                    return Ok(v);
                }
            }
            return Err(e);
        }
    };
    let v: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("jlc detail {lcsc}: bad JSON"))
        .map_err(FetchError::Other)?;
    let data = v
        .get("data")
        .cloned()
        .filter(|d| !d.is_null())
        .ok_or_else(|| FetchError::NotFound(format!("JLC part {lcsc}")))?;
    let _ = cache.put("jlc", lcsc, data.to_string().as_bytes());
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_markup_is_stripped() {
        assert_eq!(
            strip_highlight("<span class='lucene_highlight_class'>RP2040</span> MCU"),
            "RP2040 MCU"
        );
        assert_eq!(strip_highlight("plain"), "plain");
    }

    #[test]
    fn search_page_parses_the_envelope() {
        let body = serde_json::json!({
            "code": 200,
            "data": {"componentPageInfo": {"total": 1, "list": [{
                "componentCode": "C2040",
                "componentModelEn": "RP2040",
                "componentBrandEn": "Raspberry Pi",
                "componentSpecificationEn": "LQFN-56(7x7)",
                "describe": "MCU",
                "componentLibraryType": "expand",
                "preferredComponentFlag": false,
                "assemblyComponentFlag": false,
                "stockCount": 39290,
                "minPurchaseNum": 1,
                "componentPrices": [
                    {"startNumber":1,"endNumber":9,"productPrice":0.9877},
                    {"startNumber":1000,"endNumber":-1,"productPrice":0.721}
                ],
                "dataManualUrl": "https://www.lcsc.com/datasheet/x.pdf"
            }]}},
            "message": null
        });
        let page = parse_search_page(body.to_string().as_bytes(), 42, false).unwrap();
        assert_eq!(page.total, 1);
        let hit = &page.hits[0];
        assert_eq!(hit.lcsc, "C2040");
        assert_eq!(hit.class, "extended");
        assert_eq!(hit.stock, 39290);
        assert_eq!(hit.unit_price, Some(0.721));
        assert_eq!(hit.datasheet.as_deref(), Some("https://www.lcsc.com/datasheet/x.pdf"));
    }
}
