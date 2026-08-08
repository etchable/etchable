//! KiCad emitters. Byte-deterministic: fixed float formatting, stable
//! ordering, and no `(uuid …)` fields — golden tests diff the exact bytes.
//! `(embedded_files …)` is never emitted: it is the one construct the eval
//! validator checksums, and a bad checksum is a hard error.

pub mod footprint;
pub mod model3d;
pub mod symbol;

/// Deterministic mm formatting: up to 4 decimals, trailing zeros trimmed,
/// `-0` normalized.
pub fn fmt_mm(v: f64) -> String {
    let rounded = (v * 10_000.0).round() / 10_000.0;
    let rounded = if rounded == 0.0 { 0.0 } else { rounded };
    let mut s = format!("{rounded:.4}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

/// Quote a string for s-expr output.
pub fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_mm_is_deterministic_and_trimmed() {
        assert_eq!(fmt_mm(2.54), "2.54");
        assert_eq!(fmt_mm(1.0), "1");
        assert_eq!(fmt_mm(0.12345), "0.1235");
        assert_eq!(fmt_mm(-0.00001), "0");
        assert_eq!(fmt_mm(3.8000000000000003), "3.8");
    }
}
