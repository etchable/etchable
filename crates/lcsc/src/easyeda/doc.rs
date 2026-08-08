//! Parse an EasyEDA component document (`result` from
//! `/api/components/{uuid}`) into the structured form conversion consumes:
//! symbol head + shapes, footprint head + shapes + layer table, and the
//! part metadata scattered across `c_para` maps.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ParsedPart {
    pub lcsc: Option<String>,
    pub title: String,
    pub meta: PartMeta,
    pub symbol: SymbolDoc,
    pub footprint: FootprintDoc,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PartMeta {
    pub mpn: Option<String>,
    /// De-CJK'd (`Raspberry Pi(树莓派)` -> `Raspberry Pi`).
    pub manufacturer: Option<String>,
    /// Reference prefix with the `?` stripped (`U?` -> `U`).
    pub ref_prefix: String,
    /// Component value for passives (`10kΩ`).
    pub value: Option<String>,
    /// EasyEDA package name (`LQFN-56_L7.0-W7.0-P0.4-EP`).
    pub package: Option<String>,
    /// `basic` / `extended` from `JLCPCB Part Class`.
    pub class: Option<String>,
    /// Datasheet link from the footprint c_para — unreliable, callers
    /// should prefer JLC's `dataManualUrl`.
    pub datasheet: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SymbolDoc {
    /// Document origin: BBox top-left (absolute canvas units).
    pub origin: (f64, f64),
    pub bbox: (f64, f64, f64, f64),
    pub shapes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FootprintDoc {
    pub title: String,
    /// Document origin: `head.x/.y` (canvas fields 16/17 agree).
    pub origin: (f64, f64),
    pub shapes: Vec<String>,
    /// The inline layer table (`id~Name~color~…` strings).
    pub layers: Vec<String>,
}

/// Strip one trailing parenthesized group — ASCII `(...)` or full-width
/// `（…）`. If stripping empties the string, keep the original (warned by
/// the caller via the returned flag).
pub fn strip_cjk_suffix(name: &str) -> (String, bool) {
    let trimmed = name.trim();
    for (open, close) in [('(', ')'), ('（', '）')] {
        if trimmed.ends_with(close) {
            if let Some(start) = trimmed.rfind(open) {
                let stripped = trimmed[..start].trim();
                if stripped.is_empty() {
                    return (trimmed.to_string(), true);
                }
                return (stripped.to_string(), false);
            }
        }
    }
    (trimmed.to_string(), false)
}

fn c_para(head: &Value) -> impl Fn(&str) -> Option<String> + '_ {
    move |key: &str| {
        head.pointer("/c_para")
            .and_then(|p| p.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    }
}

fn shapes_of(data_str: &Value) -> Vec<String> {
    data_str
        .get("shape")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_component(component: &Value) -> Result<ParsedPart> {
    let data_str = component
        .get("dataStr")
        .context("component has no dataStr (not a symbol document?)")?;
    let head = data_str.get("head").context("dataStr has no head")?;
    let para = c_para(head);

    let bbox = data_str.get("BBox").context("dataStr has no BBox")?;
    let bb = |k: &str| bbox.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let symbol = SymbolDoc {
        origin: (bb("x"), bb("y")),
        bbox: (bb("x"), bb("y"), bb("width"), bb("height")),
        shapes: shapes_of(data_str),
    };

    let pkg = component
        .get("packageDetail")
        .filter(|p| !p.is_null())
        .context("component has no packageDetail (no footprint)")?;
    let pkg_data = pkg.get("dataStr").context("packageDetail has no dataStr")?;
    let pkg_head = pkg_data.get("head").context("packageDetail has no head")?;
    let ph = |k: &str| pkg_head.get(k).and_then(Value::as_f64);
    let footprint = FootprintDoc {
        title: pkg
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        origin: (
            ph("x").ok_or_else(|| anyhow!("footprint head has no x"))?,
            ph("y").ok_or_else(|| anyhow!("footprint head has no y"))?,
        ),
        shapes: shapes_of(pkg_data),
        layers: pkg_data
            .get("layers")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
    };

    let manufacturer = para("Manufacturer").map(|m| strip_cjk_suffix(&m).0);
    let class = para("JLCPCB Part Class").map(|c| {
        if c.to_ascii_lowercase().contains("basic") {
            "basic".to_string()
        } else {
            "extended".to_string()
        }
    });
    let pkg_para = c_para(pkg_head);
    let meta = PartMeta {
        mpn: para("Manufacturer Part"),
        manufacturer,
        ref_prefix: para("pre")
            .map(|p| p.trim_end_matches('?').to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "U".to_string()),
        value: para("Value"),
        package: para("package").or_else(|| Some(footprint.title.clone())),
        class,
        datasheet: pkg_para("link"),
    };

    Ok(ParsedPart {
        lcsc: component
            .pointer("/lcsc/number")
            .and_then(Value::as_str)
            .map(String::from),
        title: component
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        meta,
        symbol,
        footprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let text = std::fs::read_to_string(path).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        v.get("result").cloned().unwrap()
    }

    #[test]
    fn cjk_suffix_stripping() {
        assert_eq!(strip_cjk_suffix("Raspberry Pi(树莓派)").0, "Raspberry Pi");
        assert_eq!(strip_cjk_suffix("UNI-ROYAL(厚声)").0, "UNI-ROYAL");
        assert_eq!(strip_cjk_suffix("TI（德州仪器）").0, "TI");
        assert_eq!(strip_cjk_suffix("Plain Co").0, "Plain Co");
        // Stripping to nothing keeps the original and flags it.
        let (kept, warned) = strip_cjk_suffix("(only)");
        assert_eq!(kept, "(only)");
        assert!(warned);
    }

    #[test]
    fn c2040_parses_with_identity_and_odd_free_origin() {
        let part = parse_component(&fixture("component_C2040.json")).unwrap();
        assert_eq!(part.lcsc.as_deref(), Some("C2040"));
        assert_eq!(part.meta.mpn.as_deref(), Some("RP2040"));
        assert_eq!(part.meta.manufacturer.as_deref(), Some("Raspberry Pi"));
        assert_eq!(part.meta.ref_prefix, "U");
        assert_eq!(part.meta.class.as_deref(), Some("extended"));
        assert_eq!(part.footprint.title, "LQFN-56_L7.0-W7.0-P0.4-EP");
        assert!(part.symbol.shapes.len() > 50);
        assert!(part.footprint.shapes.len() > 100);
        assert!(!part.footprint.layers.is_empty());
    }

    #[test]
    fn c381367_has_an_origin_nowhere_near_4000_3000() {
        let part = parse_component(&fixture("component_C381367.json")).unwrap();
        // The counterexample that kills hardcoded origins.
        assert!(
            part.footprint.origin.0 < 1000.0,
            "expected odd origin, got {:?}",
            part.footprint.origin
        );
    }

    #[test]
    fn c25804_is_a_passive_with_a_value() {
        let part = parse_component(&fixture("component_C25804.json")).unwrap();
        assert_eq!(part.meta.ref_prefix, "R");
        assert!(part.meta.value.is_some());
    }
}
