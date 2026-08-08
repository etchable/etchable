//! EasyEDA units: 1 unit = 10 mil = 0.254 mm. Coordinates are absolute
//! canvas positions (often near 4000,3000 — but NOT always; C381367 sits
//! near 363,310), so every conversion subtracts a document origin. KiCad's
//! Y axis is inverted relative to EasyEDA's.

pub const MM_PER_UNIT: f64 = 0.254;

pub fn to_mm(units: f64) -> f64 {
    units * MM_PER_UNIT
}

/// X relative to origin, in mm.
pub fn x_mm(x: f64, origin_x: f64) -> f64 {
    to_mm(x - origin_x)
}

/// Y relative to origin, in mm, sign flipped for KiCad.
pub fn y_mm(y: f64, origin_y: f64) -> f64 {
    -to_mm(y - origin_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_and_sign() {
        assert!((to_mm(10.0) - 2.54).abs() < 1e-9);
        assert!((x_mm(4010.0, 4000.0) - 2.54).abs() < 1e-9);
        assert!((y_mm(3010.0, 3000.0) + 2.54).abs() < 1e-9);
        // Odd origins must work the same way (C381367).
        assert!((x_mm(373.0, 363.0) - 2.54).abs() < 1e-9);
    }
}
