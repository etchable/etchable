//! EasyEDA `shape` record grammars. Records are `~`-delimited; the first
//! token is the type tag. Symbol pins additionally use `^^` as a segment
//! separator. Field orders were verified against live documents (see
//! tests/fixtures/README.md); unknown tags are skipped with a warning, never
//! errors — graphics fidelity must not gate a part install.

use anyhow::{anyhow, bail, Result};

// ---------------------------------------------------------------------------
// Symbol records
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct SymbolPin {
    /// KiCad pin number — segment 4 field 4, NEVER `spice_pin_number`
    /// (settings field 2), which is frequently wrong.
    pub number: String,
    pub name: String,
    /// Absolute canvas position (EE units).
    pub x: f64,
    pub y: f64,
    /// Degrees; 0 = pin points right.
    pub rotation: f64,
    /// EasyEDA electrical type: 0 unspecified, 1 in, 2 out, 3 bidi, 4 power.
    pub electrical: u8,
}

impl SymbolPin {
    /// KiCad electrical type as a bare symbol. Power pins map to `power_in`
    /// (never `power_out` — a symbol claiming to source power trips ERC).
    pub fn kicad_type(&self) -> &'static str {
        match self.electrical {
            1 => "input",
            2 => "output",
            3 => "bidirectional",
            4 => "power_in",
            _ => "passive",
        }
    }
}

/// Parse a `P~…^^…` symbol pin record.
pub fn parse_pin(record: &str) -> Result<SymbolPin> {
    let segments: Vec<&str> = record.split("^^").collect();
    let settings: Vec<&str> = segments
        .first()
        .ok_or_else(|| anyhow!("empty pin record"))?
        .split('~')
        .collect();
    if settings.first() != Some(&"P") {
        bail!("not a pin record: {record:.40}");
    }
    let f = |i: usize| -> f64 {
        settings
            .get(i)
            .and_then(|s| s.parse().ok())
            .unwrap_or_default()
    };
    // Segment 4 ("num"): show~x~y~rotation~number~…
    let number = segments
        .get(4)
        .and_then(|seg| seg.split('~').nth(4))
        .unwrap_or("")
        .trim()
        .to_string();
    if number.is_empty() {
        // pcb-eda silently DROPS pins with empty numbers — that would
        // corrupt the part, so fail loudly instead.
        bail!("pin with empty number (name segment: {:?})", segments.get(3));
    }
    let name = segments
        .get(3)
        .and_then(|seg| seg.split('~').nth(4))
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(SymbolPin {
        number,
        name,
        x: f(4),
        y: f(5),
        rotation: f(6),
        electrical: settings
            .get(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
    })
}

// ---------------------------------------------------------------------------
// Footprint records
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PadRecord {
    /// ELLIPSE | RECT | OVAL | POLYGON (anything else -> custom).
    pub shape: String,
    pub cx: f64,
    pub cy: f64,
    pub width: f64,
    pub height: f64,
    pub layer_id: u32,
    pub number: String,
    pub hole_radius: f64,
    /// Space-separated absolute vertex coords (POLYGON / RECT outline).
    pub points: Vec<(f64, f64)>,
    pub rotation: f64,
    /// Slot length; >0 with hole_radius>0 means an oval drill.
    pub hole_length: f64,
    /// `"N"` = non-plated.
    pub plated: bool,
}

#[derive(Debug, Clone)]
pub struct TrackRecord {
    pub stroke_width: f64,
    pub layer_id: u32,
    pub points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub struct ArcRecord {
    pub stroke_width: f64,
    pub layer_id: u32,
    /// `M x y A rx ry xrot large_arc sweep ex ey` — whitespace after
    /// command letters is optional in the wild.
    pub start: (f64, f64),
    pub rx: f64,
    pub ry: f64,
    pub large_arc: bool,
    pub sweep: bool,
    pub end: (f64, f64),
}

#[derive(Debug, Clone)]
pub struct CircleRecord {
    pub cx: f64,
    pub cy: f64,
    pub radius: f64,
    pub stroke_width: f64,
    pub layer_id: u32,
}

#[derive(Debug, Clone)]
pub struct RectRecord {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub layer_id: u32,
    pub stroke_width: f64,
}

#[derive(Debug, Clone)]
pub struct HoleRecord {
    pub cx: f64,
    pub cy: f64,
    pub radius: f64,
}

#[derive(Debug, Clone)]
pub struct ViaRecord {
    pub cx: f64,
    pub cy: f64,
    pub diameter: f64,
    pub hole_radius: f64,
}

#[derive(Debug, Clone)]
pub struct TextRecord {
    /// `N` = name, `P` = prefix; anything else is user text.
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub layer_id: u32,
    pub text: String,
    pub visible: bool,
}

#[derive(Debug, Clone)]
pub struct SolidRegionRecord {
    pub layer_id: u32,
    /// `solid` | `cutout` | `npth`.
    pub region_type: String,
    pub points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub struct SvgNodeRecord {
    /// The downloadable 3D uuid (`attrs.uuid` — NOT `head.uuid_3d`).
    pub uuid: Option<String>,
    /// Degrees, `rx,ry,rz`.
    pub rotation: (f64, f64, f64),
    /// Z offset in EE units.
    pub z: f64,
    /// Model origin in EE units.
    pub origin: Option<(f64, f64)>,
    /// Outline vertex soup from childNodes, for the centre correction.
    pub outline_points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub enum FootprintRecord {
    Pad(PadRecord),
    Track(TrackRecord),
    Arc(ArcRecord),
    Circle(CircleRecord),
    Rect(RectRecord),
    Hole(HoleRecord),
    Via(ViaRecord),
    Text(TextRecord),
    SolidRegion(SolidRegionRecord),
    SvgNode(SvgNodeRecord),
    /// Recognized but deliberately not converted.
    Skipped(&'static str),
}

fn parse_points(s: &str) -> Vec<(f64, f64)> {
    let nums: Vec<f64> = s
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    nums.chunks_exact(2).map(|c| (c[0], c[1])).collect()
}

/// Pad numbers sometimes come as `A(1)` — the parenthesized part is the
/// real number.
pub fn normalize_pad_number(raw: &str) -> String {
    if let (Some(open), Some(close)) = (raw.find('('), raw.rfind(')')) {
        if open < close {
            return raw[open + 1..close].trim().to_string();
        }
    }
    raw.trim().to_string()
}

/// Tokenize an SVG arc path `M x y A rx ry xrot laf sf ex ey`, tolerating
/// `M4007.13` / `A0.6` (no space after the command letter), commas, and
/// stray whitespace.
pub fn parse_arc_path(path: &str) -> Result<((f64, f64), f64, f64, bool, bool, (f64, f64))> {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in path.chars() {
        match c {
            'M' | 'm' | 'A' | 'a' => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
                tokens.push(c.to_string());
            }
            ' ' | ',' | '\t' | '~' => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    let m = tokens
        .iter()
        .position(|t| t.eq_ignore_ascii_case("M"))
        .ok_or_else(|| anyhow!("arc path without M: {path}"))?;
    let a = tokens
        .iter()
        .position(|t| t.eq_ignore_ascii_case("A"))
        .ok_or_else(|| anyhow!("arc path without A: {path}"))?;
    let num = |i: usize| -> Result<f64> {
        tokens
            .get(i)
            .ok_or_else(|| anyhow!("arc path too short: {path}"))?
            .parse()
            .map_err(|_| anyhow!("bad number in arc path: {path}"))
    };
    let start = (num(m + 1)?, num(m + 2)?);
    let rx = num(a + 1)?;
    let ry = num(a + 2)?;
    // a+3 = x-axis-rotation (unused for circular arcs)
    let large_arc = num(a + 4)? != 0.0;
    let sweep = num(a + 5)? != 0.0;
    let end = (num(a + 6)?, num(a + 7)?);
    Ok((start, rx, ry, large_arc, sweep, end))
}

/// Parse one footprint shape record. `Ok(None)` = unknown tag (warned by
/// the caller).
pub fn parse_footprint_record(record: &str) -> Result<Option<FootprintRecord>> {
    let fields: Vec<&str> = record.split('~').collect();
    let tag = *fields.first().unwrap_or(&"");
    let f = |i: usize| -> f64 {
        fields
            .get(i)
            .and_then(|s| s.parse().ok())
            .unwrap_or_default()
    };
    let layer = |i: usize| -> u32 {
        fields
            .get(i)
            .and_then(|s| s.parse().ok())
            .unwrap_or_default()
    };
    Ok(Some(match tag {
        "PAD" => FootprintRecord::Pad(PadRecord {
            shape: fields.get(1).unwrap_or(&"").to_string(),
            cx: f(2),
            cy: f(3),
            width: f(4),
            height: f(5),
            layer_id: layer(6),
            number: normalize_pad_number(fields.get(8).unwrap_or(&"")),
            hole_radius: f(9),
            points: parse_points(fields.get(10).unwrap_or(&"")),
            rotation: f(11),
            hole_length: f(13),
            plated: fields.get(15).map(|s| *s != "N").unwrap_or(true),
        }),
        "TRACK" => FootprintRecord::Track(TrackRecord {
            stroke_width: f(1),
            layer_id: layer(2),
            points: parse_points(fields.get(4).unwrap_or(&"")),
        }),
        "ARC" => {
            let path = fields.get(4).unwrap_or(&"");
            let (start, rx, ry, large_arc, sweep, end) = parse_arc_path(path)?;
            FootprintRecord::Arc(ArcRecord {
                stroke_width: f(1),
                layer_id: layer(2),
                start,
                rx,
                ry,
                large_arc,
                sweep,
                end,
            })
        }
        "CIRCLE" => FootprintRecord::Circle(CircleRecord {
            cx: f(1),
            cy: f(2),
            radius: f(3),
            stroke_width: f(4),
            layer_id: layer(5),
        }),
        "RECT" => FootprintRecord::Rect(RectRecord {
            x: f(1),
            y: f(2),
            width: f(3),
            height: f(4),
            layer_id: layer(5),
            stroke_width: f(8),
        }),
        "HOLE" => FootprintRecord::Hole(HoleRecord {
            cx: f(1),
            cy: f(2),
            radius: f(3),
        }),
        "VIA" => FootprintRecord::Via(ViaRecord {
            cx: f(1),
            cy: f(2),
            diameter: f(3),
            hole_radius: f(5),
        }),
        "TEXT" => FootprintRecord::Text(TextRecord {
            kind: fields.get(1).unwrap_or(&"").to_string(),
            x: f(2),
            y: f(3),
            layer_id: layer(7),
            text: fields.get(10).unwrap_or(&"").to_string(),
            visible: fields.get(12).map(|s| *s != "0").unwrap_or(true),
        }),
        "SOLIDREGION" => {
            let path = fields.get(3).unwrap_or(&"");
            FootprintRecord::SolidRegion(SolidRegionRecord {
                layer_id: layer(1),
                region_type: fields.get(4).unwrap_or(&"solid").to_string(),
                points: parse_region_path(path),
            })
        }
        "SVGNODE" => {
            let json_str = record.splitn(2, '~').nth(1).unwrap_or("{}");
            FootprintRecord::SvgNode(parse_svgnode(json_str)?)
        }
        "" => return Ok(None),
        _ => return Ok(None),
    }))
}

/// SOLIDREGION paths are `M x y L x y … Z` polylines; sample only the
/// vertex coordinates (curved region edges degrade to their control points).
fn parse_region_path(path: &str) -> Vec<(f64, f64)> {
    let mut nums: Vec<f64> = Vec::new();
    let mut cur = String::new();
    for c in path.chars() {
        if c.is_ascii_digit() || c == '.' || c == '-' {
            cur.push(c);
        } else if !cur.is_empty() {
            if let Ok(n) = cur.parse() {
                nums.push(n);
            }
            cur.clear();
        }
    }
    if let Ok(n) = cur.parse() {
        nums.push(n);
    }
    nums.chunks_exact(2).map(|c| (c[0], c[1])).collect()
}

fn parse_svgnode(json_str: &str) -> Result<SvgNodeRecord> {
    let v: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| anyhow!("SVGNODE json: {e}"))?;
    let attrs = v.get("attrs").cloned().unwrap_or_default();
    let s = |k: &str| attrs.get(k).and_then(serde_json::Value::as_str);
    let rotation = s("c_rotation")
        .map(|r| {
            let mut it = r.split(',').map(|t| t.trim().parse().unwrap_or(0.0));
            (
                it.next().unwrap_or(0.0),
                it.next().unwrap_or(0.0),
                it.next().unwrap_or(0.0),
            )
        })
        .unwrap_or((0.0, 0.0, 0.0));
    let origin = s("c_origin").and_then(|o| {
        let mut it = o.split(',').map(|t| t.trim().parse::<f64>());
        match (it.next(), it.next()) {
            (Some(Ok(x)), Some(Ok(y))) => Some((x, y)),
            _ => None,
        }
    });
    let mut outline_points = Vec::new();
    if let Some(children) = v.get("childNodes").and_then(serde_json::Value::as_array) {
        for child in children {
            if let Some(pts) = child.pointer("/attrs/points").and_then(serde_json::Value::as_str) {
                outline_points.extend(parse_points(pts));
            }
        }
    }
    Ok(SvgNodeRecord {
        uuid: s("uuid").map(String::from),
        rotation,
        z: s("z").and_then(|z| z.parse().ok()).unwrap_or(0.0),
        origin,
        outline_points,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Live C2040 record (fixtures/README.md).
    const PIN: &str = "P~show~3~9~525~230~0~gge461~0^^525~230^^M 525 230 h -10~#880000^^1~511~233.75~0~GPIO7~end~Tahoma~5.8pt~#0000FF^^1~524~229.9~0~9~end~Tahoma~5.8pt~#0000FF^^0~518~230^^0~M 515 227 L 512 230 L 515 233";

    #[test]
    fn pin_number_comes_from_segment_4_not_spice_number() {
        let pin = parse_pin(PIN).unwrap();
        // settings field 2 (spice_pin_number) is 9 here too, but make the
        // disagreement explicit:
        let disagreeing = PIN.replacen("P~show~3~9", "P~show~3~99", 1);
        let pin2 = parse_pin(&disagreeing).unwrap();
        assert_eq!(pin.number, "9");
        assert_eq!(pin2.number, "9", "must read segment 4 field 4, not spice_pin_number");
        assert_eq!(pin.name, "GPIO7");
        assert_eq!((pin.x, pin.y), (525.0, 230.0));
        assert_eq!(pin.kicad_type(), "bidirectional");
    }

    #[test]
    fn empty_pin_number_is_a_hard_error() {
        let broken = PIN.replace("^^1~524~229.9~0~9~end", "^^1~524~229.9~0~~end");
        assert!(parse_pin(&broken).is_err());
    }

    #[test]
    fn power_pins_map_to_power_in_never_out() {
        let power = PIN.replacen("P~show~3~", "P~show~4~", 1);
        assert_eq!(parse_pin(&power).unwrap().kicad_type(), "power_in");
    }

    #[test]
    fn arc_tokenizer_handles_both_whitespace_forms() {
        // Both live variants from the research notes.
        let spaced = "M 4002.8401 2998.0329 A 0.6 0.6 0 0 1 4003.4401 2998.6329";
        let tight = "M4007.1307 3003.5575 A0.6000 0.6000 0.0000 0 0 4007.7307 3002.9575 ";
        let (s1, rx1, _, la1, sw1, e1) = parse_arc_path(spaced).unwrap();
        assert_eq!(s1, (4002.8401, 2998.0329));
        assert_eq!(rx1, 0.6);
        assert!(!la1 && sw1);
        assert_eq!(e1, (4003.4401, 2998.6329));
        let (s2, rx2, _, _, sw2, e2) = parse_arc_path(tight).unwrap();
        assert_eq!(s2, (4007.1307, 3003.5575));
        assert_eq!(rx2, 0.6);
        assert!(!sw2);
        assert_eq!(e2, (4007.7307, 3002.9575));
    }

    #[test]
    fn pad_records_parse_with_plating_and_parenthesized_numbers() {
        // Live C2040 pad, with the number swapped for the A(1) form.
        let rec = "PAD~RECT~4002.966~3000~3.1751~3.4016~1~~A(1)~0~4001.378 3001.7008 4001.378 2998.2992 4004.5531 2998.2992 4004.5531 3001.7008~0~gge1002~0~~Y~0~-393.7008~0.2000~4002.9655,3000";
        let Some(FootprintRecord::Pad(pad)) = parse_footprint_record(rec).unwrap() else {
            panic!("expected pad");
        };
        assert_eq!(pad.number, "1");
        assert_eq!(pad.shape, "RECT");
        assert!(pad.plated);
        assert_eq!(pad.points.len(), 4);
        let non_plated = rec.replace("~gge1002~0~~Y~", "~gge1002~0~~N~");
        let Some(FootprintRecord::Pad(pad)) = parse_footprint_record(&non_plated).unwrap() else {
            panic!("expected pad");
        };
        assert!(!pad.plated);
    }

    #[test]
    fn solidregion_types_parse() {
        let rec = "SOLIDREGION~100~~M 3973.6219 3001.1815 L 3973.6219 2999.2129 L 3974.4093 2999.2129 L 3974.4093 3001.1815 Z~solid~gge483~~~~0";
        let Some(FootprintRecord::SolidRegion(sr)) = parse_footprint_record(rec).unwrap() else {
            panic!("expected solidregion");
        };
        assert_eq!(sr.region_type, "solid");
        assert_eq!(sr.points.len(), 4);
        let npth = rec.replace("~solid~", "~npth~");
        let Some(FootprintRecord::SolidRegion(sr)) = parse_footprint_record(&npth).unwrap() else {
            panic!("expected solidregion");
        };
        assert_eq!(sr.region_type, "npth");
    }

    #[test]
    fn svgnode_reads_the_downloadable_uuid_and_rotation() {
        let rec = r#"SVGNODE~{"gId":"g1_outline","attrs":{"c_width":"27.6377","c_rotation":"0,0,90","z":"0","c_origin":"3984.252,3012.9925","uuid":"76b360a9d4c54384a4e47d7e5af156df","c_etype":"outline3D"},"childNodes":[{"attrs":{"points":"3980 3010 3988 3010 3988 3016 3980 3016"}}]}"#;
        let Some(FootprintRecord::SvgNode(node)) = parse_footprint_record(rec).unwrap() else {
            panic!("expected svgnode");
        };
        assert_eq!(node.uuid.as_deref(), Some("76b360a9d4c54384a4e47d7e5af156df"));
        assert_eq!(node.rotation, (0.0, 0.0, 90.0));
        assert_eq!(node.origin, Some((3984.252, 3012.9925)));
        assert_eq!(node.outline_points.len(), 4);
    }
}
