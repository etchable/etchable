//! LCSC-backed tools (docs/decisions/0004): the live-parts tier of
//! `search_parts`, the lcsc vendor behind the pre-commit `get_part` check,
//! and `add_component`'s LCSC fetch path — THE way to add a real part.
//! Network failures are never opaque errors: blocked/offline map to
//! actionable messages and cached data keeps working.

use std::sync::OnceLock;

use serde_json::{json, Value};

use lcsc::easyeda::records::parse_pin;
use lcsc::{Cache, Client, FetchError};

pub const UNVERIFIED_NOTICE: &str = "Converted from EasyEDA CAD data — UNVERIFIED. \
    Cross-check pin count, pad count, and pin names against the datasheet before \
    trusting this part; relay any conversion warnings to the user.";

fn cache() -> anyhow::Result<&'static Cache> {
    static CACHE: OnceLock<Option<Cache>> = OnceLock::new();
    CACHE
        .get_or_init(|| Cache::open(&store::paths::cache_dir().join("lcsc/v1")).ok())
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no usable cache directory"))
}

fn client() -> anyhow::Result<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            let cache = cache().ok()?;
            Client::new(cache.root()).ok()
        })
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("http client failed to initialize"))
}

fn fetch_error_payload(e: &FetchError) -> Value {
    let status = match e {
        FetchError::Offline => "offline",
        FetchError::Blocked { .. } => "blocked",
        FetchError::NotFound(_) => "not_found",
        FetchError::Other(_) => "error",
    };
    json!({"status": status, "hint": e.to_string()})
}

const MAX_LCSC_RESULTS: usize = 12;

/// Basic-first ranking: in-stock Basic parts, then in-stock Extended, then
/// out-of-stock anything — stable within each group so upstream relevance
/// survives. Applied BEFORE the result cap so Basic options never fall off.
pub fn rank_hits(hits: &mut [lcsc::jlc::SearchHit]) {
    hits.sort_by_key(|h| (h.stock <= 0, h.class != "basic"));
}

/// The live tier of `search_parts`. Never an error payload — local results
/// always accompany whatever this returns.
pub async fn search_tier(query: &str) -> Value {
    let (client, cache) = match (client(), cache()) {
        (Ok(c), Ok(k)) => (c, k),
        _ => return json!({"status": "error", "hint": "LCSC client unavailable"}),
    };
    match lcsc::jlc::search(client, cache, query).await {
        Ok(mut page) => {
            rank_hits(&mut page.hits);
            let results: Vec<Value> = page
                .hits
                .iter()
                .take(MAX_LCSC_RESULTS)
                .map(|h| {
                    let mut v = serde_json::to_value(h).unwrap_or_default();
                    v["add"] = json!(format!(
                        "add_component{{name: \"<YourName>\", lcsc: \"{}\"}}",
                        h.lcsc
                    ));
                    v
                })
                .collect();
            json!({
                "status": "ok",
                "total": page.total,
                "as_of": page.as_of,
                "cached": page.cached,
                "results": results,
                "hint": "Results are ranked Basic-first. If a class=basic part satisfies the requirement, use it — every Extended part adds a JLC setup fee; pick Extended only when no Basic fits, and tell the user why. Stock 0 is unbuildable. Run get_part before committing to one.",
            })
        }
        Err(e) => fetch_error_payload(&e),
    }
}

/// Pre-commit check: identity + orderability + a first look at the EDA data
/// quality (pin/pad counts are the best early warning for a bad EasyEDA
/// part).
pub async fn get_part(lcsc_code: &str) -> Value {
    let (client, cache) = match (client(), cache()) {
        (Ok(c), Ok(k)) => (c, k),
        _ => return json!({"status": "error", "hint": "LCSC client unavailable"}),
    };

    let jlc = match lcsc::jlc::detail(client, cache, lcsc_code).await {
        Ok(v) => Some(v),
        Err(e @ (FetchError::Offline | FetchError::Blocked { .. })) => {
            return fetch_error_payload(&e)
        }
        Err(_) => None,
    };

    let mut out = json!({"status": "ok", "lcsc": lcsc_code});
    if let Some(d) = &jlc {
        let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("");
        out["mpn"] = json!(lcsc::jlc::strip_highlight(s("componentModelEn")));
        out["manufacturer"] = json!(lcsc::jlc::strip_highlight(s("componentBrandEn")));
        out["package"] = json!(s("componentSpecificationEn"));
        out["description"] = json!(s("describe"));
        out["ref_prefix"] = json!(s("componentDesignator"));
        out["class"] = json!(match s("componentLibraryType") {
            "base" => "basic",
            "expand" => "extended",
            _ => "unknown",
        });
        out["stock"] = d.get("stockCount").cloned().unwrap_or(json!(0));
        out["min_qty"] = d.get("minPurchaseNum").cloned().unwrap_or(json!(1));
        out["msl"] = json!(s("moistureSensitivityLevelEn"));
        out["assembly_process"] = json!(s("assemblyProcess"));
        out["status"] = json!("ok");
        out["part_status"] = json!(s("componentStatus"));
        out["datasheet"] = json!(s("dataManualUrl"));
        if let Some(prices) = d.get("componentPrices").and_then(Value::as_array) {
            out["prices"] = json!(prices
                .iter()
                .take(6)
                .map(|p| json!({
                    "from": p.get("startNumber"),
                    "to": p.get("endNumber"),
                    "usd": p.get("productPrice"),
                }))
                .collect::<Vec<_>>());
        }
        if let Some(attrs) = d.get("attributes").and_then(Value::as_array) {
            let cleaned: Vec<Value> = attrs
                .iter()
                .filter_map(|a| {
                    let name = a.get("attribute_name_en").and_then(Value::as_str)?;
                    let val = a.get("attribute_value_name").and_then(Value::as_str)?;
                    // "-" means unspecified upstream.
                    (val != "-").then(|| json!({"name": name, "value": val}))
                })
                .take(30)
                .collect();
            out["attributes"] = json!(cleaned);
        }
    }

    // EDA data quality probe.
    match probe_eda(client, cache, lcsc_code).await {
        Ok(probe) => {
            out["eda"] = probe;
        }
        Err(e) => {
            out["eda"] = json!({"status": "unavailable", "hint": e.to_string()});
        }
    }
    out
}

async fn probe_eda(
    client: &Client,
    cache: &Cache,
    lcsc_code: &str,
) -> Result<Value, FetchError> {
    let entries = lcsc::easyeda::api::search_by_numbers(client, cache, &[lcsc_code]).await?;
    let Some(entry) = entries
        .into_iter()
        .find(|e| e.number.eq_ignore_ascii_case(lcsc_code))
    else {
        return Ok(json!({"has_symbol": false, "has_footprint": false, "has_3d": false,
            "hint": "no EasyEDA CAD data for this part — add_component cannot fetch it from LCSC"}));
    };
    let component = lcsc::easyeda::api::component(client, cache, &entry.uuid).await?;
    let parsed = lcsc::easyeda::doc::parse_component(&component)
        .map_err(FetchError::Other)?;
    let mut pin_names = Vec::new();
    let mut pin_count = 0usize;
    for shape in &parsed.symbol.shapes {
        if shape.starts_with("P~") {
            pin_count += 1;
            if let Ok(pin) = parse_pin(shape) {
                if pin_names.len() < 8 {
                    pin_names.push(pin.name);
                }
            }
        }
    }
    let pad_count = parsed
        .footprint
        .shapes
        .iter()
        .filter(|s| s.starts_with("PAD~"))
        .count();
    Ok(json!({
        "has_symbol": pin_count > 0,
        "has_footprint": pad_count > 0,
        "has_3d": entry.step.is_some(),
        "pin_count": pin_count,
        "pad_count": pad_count,
        "first_pins": pin_names,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(lcsc: &str, class: &str, stock: i64) -> lcsc::jlc::SearchHit {
        lcsc::jlc::SearchHit {
            lcsc: lcsc.into(),
            mpn: String::new(),
            manufacturer: String::new(),
            package: String::new(),
            description: String::new(),
            class: class.into(),
            preferred: false,
            assembly: false,
            stock,
            min_qty: 1,
            unit_price: None,
            datasheet: None,
        }
    }

    #[test]
    fn ranking_puts_stocked_basic_first_and_stays_stable() {
        let mut hits = vec![
            hit("C1", "extended", 100),
            hit("C2", "basic", 0),
            hit("C3", "basic", 500),
            hit("C4", "extended", 50),
            hit("C5", "basic", 200),
        ];
        rank_hits(&mut hits);
        let order: Vec<&str> = hits.iter().map(|h| h.lcsc.as_str()).collect();
        // Stocked basics (original relative order), stocked extendeds
        // (ditto), then out-of-stock.
        assert_eq!(order, ["C3", "C5", "C1", "C4", "C2"]);
    }
}

pub struct AddLcscArgs {
    pub name: String,
    pub lcsc: String,
    pub include_3d: bool,
    pub fetch_datasheet: bool,
    pub overwrite: bool,
}

/// Fetch -> convert -> install -> (optionally) pull the datasheet. Returns
/// the reviewable diff plus provenance and the unverified disclaimer.
pub async fn add_component(
    project_root: &std::path::Path,
    args: &AddLcscArgs,
) -> Result<Value, String> {
    let (client, cache) = match (client(), cache()) {
        (Ok(c), Ok(k)) => (c, k),
        _ => return Err("LCSC client unavailable".into()),
    };

    let raw = lcsc::fetch_part(
        client,
        cache,
        &args.lcsc,
        &lcsc::FetchOptions {
            include_3d: args.include_3d,
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    // Conversion is pure but not free on huge parts — off the async thread.
    let name = args.name.clone();
    let converted = {
        let raw = raw.clone();
        tokio::task::spawn_blocking(move || {
            lcsc::convert(&raw, &lcsc::ConvertOptions { name })
        })
        .await
        .map_err(|e| format!("convert panicked: {e}"))?
        .map_err(|e| format!("{e:#}"))?
    };

    // JLC library class — the detail endpoint is authoritative, the
    // EasyEDA `JLCPCB Part Class` c_para is the fallback. Recorded in the
    // card so the BOM shows Basic vs Extended (setup-fee) composition.
    let class = raw
        .jlc_detail
        .as_ref()
        .and_then(|d| d.get("componentLibraryType").and_then(Value::as_str))
        .and_then(|t| match t {
            "base" => Some("basic".to_string()),
            "expand" => Some("extended".to_string()),
            _ => None,
        })
        .or_else(|| converted.meta.class.clone());
    let lcsc_basic = match class.as_deref() {
        Some("basic") => Some(true),
        Some("extended") => Some(false),
        _ => None,
    };

    let easyeda_uuid = raw
        .component
        .get("uuid")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut provenance = vec![
        ("source".to_string(), "lcsc/easyeda".to_string()),
        ("lcsc".to_string(), args.lcsc.clone()),
        ("easyeda_uuid".to_string(), easyeda_uuid),
        ("fetched_at_unix".to_string(), raw.fetched_at.to_string()),
        ("verified".to_string(), "false".to_string()),
    ];
    if let Some(uuid) = &raw.step_uuid {
        provenance.push(("model_3d_uuid".to_string(), uuid.clone()));
    }

    let mut assets = Vec::new();
    let mut extra_assets = Vec::new();
    if let Some(step) = &converted.step {
        let file_name = format!("{}.step", args.name);
        assets.push((
            "model_3d".to_string(),
            format!("components/{}.assets/{file_name}", args.name),
        ));
        extra_assets.push(zen_build::ExtraAsset {
            file_name,
            bytes: step.clone(),
        });
    }

    let req = zen_build::InstallComponentRequest {
        name: args.name.clone(),
        symbol_kicad_sym: converted.symbol_kicad_sym.clone(),
        footprint_kicad_mod: Some(converted.footprint_kicad_mod.clone()),
        extra_assets,
        mpn: converted.meta.mpn.clone(),
        manufacturer: converted.meta.manufacturer.clone(),
        lcsc: Some(args.lcsc.clone()),
        lcsc_basic,
        description: None,
        datasheet_url: converted.datasheet.clone(),
        provenance,
        assets,
        overwrite: args.overwrite,
    };
    let root = project_root.to_path_buf();
    let installed = tokio::task::spawn_blocking(move || zen_build::install_component(&root, &req))
        .await
        .map_err(|e| format!("install panicked: {e}"))?
        .map_err(|e| format!("{e:#}"))?;

    let mut warnings = converted.warnings.clone();
    let mut datasheet_path = None;
    if args.fetch_datasheet {
        if let Some(url) = &converted.datasheet {
            match crate::datasheet::fetch_datasheet(project_root, url, &args.name).await {
                Ok(f) => datasheet_path = Some(f.path),
                Err(e) => warnings.push(format!("datasheet download failed: {e:#}")),
            }
        } else {
            warnings.push("no datasheet URL available for this part".into());
        }
    }

    Ok(json!({
        "files_written": installed.files_written,
        "class": class,
        "zen_text": installed.zen_text,
        "card_text": installed.card_text,
        "pin_count": converted.pin_count,
        "pad_count": converted.pad_count,
        "io_names": installed.io_names,
        "datasheet": datasheet_path,
        "meta": converted.meta,
        "warnings": warnings,
        "notice": UNVERIFIED_NOTICE,
    }))
}
