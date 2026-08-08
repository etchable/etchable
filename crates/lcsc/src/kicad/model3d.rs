//! 3D model placement — strategy "emit the offset": geometry is never
//! touched (baking offsets into a copied-verbatim STEP is the known
//! misplacement bug in other tools); we compute the `(offset (xyz …))` from
//! the SVGNODE transform instead. STEP files are mm and need no scaling.

use crate::easyeda::records::SvgNodeRecord;
use crate::easyeda::units;

use super::fmt_mm;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ModelPlacement {
    /// mm.
    pub offset: (f64, f64, f64),
    /// Degrees, already negated for KiCad.
    pub rotate: (f64, f64, f64),
}

/// `doc_origin` is the footprint document origin (head.x/y, EE units).
pub fn placement(node: &SvgNodeRecord, doc_origin: (f64, f64)) -> ModelPlacement {
    let (ox, oy) = doc_origin;
    let (mut tx, mut ty) = match node.origin {
        Some((cx, cy)) => (units::to_mm(cx - ox), -units::to_mm(cy - oy)),
        None => (0.0, 0.0),
    };

    // Outline-centre correction: when the model origin disagrees with the
    // outline's bbox centre by more than 0.1 mm, trust the outline — this
    // fixes a real class of mis-centred LCSC models.
    if !node.outline_points.is_empty() {
        let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        for (px, py) in &node.outline_points {
            min_x = min_x.min(*px);
            max_x = max_x.max(*px);
            min_y = min_y.min(*py);
            max_y = max_y.max(*py);
        }
        let cx = units::to_mm((min_x + max_x) / 2.0 - ox);
        let cy = -units::to_mm((min_y + max_y) / 2.0 - oy);
        if ((cx - tx).powi(2) + (cy - ty).powi(2)).sqrt() > 0.1 {
            (tx, ty) = (cx, cy);
        }
    }

    ModelPlacement {
        offset: (tx, ty, units::to_mm(node.z)),
        rotate: (-node.rotation.0, -node.rotation.1, -node.rotation.2),
    }
}

/// The `(model …)` block to splice into a `.kicad_mod`. `path` is the
/// project-relative model path (e.g. `${KIPRJMOD}/components/X.assets/X.step`).
pub fn model_sexpr(path: &str, p: &ModelPlacement) -> String {
    format!(
        "\t(model {}\n\t\t(offset (xyz {} {} {}))\n\t\t(scale (xyz 1 1 1))\n\t\t(rotate (xyz {} {} {}))\n\t)\n",
        super::quote(path),
        fmt_mm(p.offset.0),
        fmt_mm(p.offset.1),
        fmt_mm(p.offset.2),
        fmt_mm(p.rotate.0),
        fmt_mm(p.rotate.1),
        fmt_mm(p.rotate.2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(origin: Option<(f64, f64)>, rotation: (f64, f64, f64), outline: Vec<(f64, f64)>) -> SvgNodeRecord {
        SvgNodeRecord {
            uuid: Some("u".into()),
            rotation,
            z: 0.0,
            origin,
            outline_points: outline,
        }
    }

    #[test]
    fn centred_model_gets_zero_offset_and_negated_rotation() {
        let p = placement(&node(Some((4000.0, 3000.0)), (0.0, 0.0, 90.0), vec![]), (4000.0, 3000.0));
        assert_eq!(p.offset, (0.0, 0.0, 0.0));
        assert_eq!(p.rotate, (0.0, 0.0, -90.0));
    }

    #[test]
    fn outline_centre_wins_when_it_disagrees_by_more_than_point_one_mm() {
        // Origin says 0,0 but the outline is centred 10 EE units (2.54 mm)
        // to the right.
        let outline = vec![(4005.0, 2995.0), (4015.0, 3005.0)];
        let p = placement(&node(Some((4000.0, 3000.0)), (0.0, 0.0, 0.0), outline), (4000.0, 3000.0));
        assert!((p.offset.0 - 2.54).abs() < 1e-9, "{:?}", p.offset);
        // Within threshold: origin wins.
        let close = vec![(3999.9, 2999.9), (4000.1, 3000.1)];
        let p = placement(&node(Some((4000.0, 3000.0)), (0.0, 0.0, 0.0), close), (4000.0, 3000.0));
        assert_eq!(p.offset.0, 0.0);
    }
}
