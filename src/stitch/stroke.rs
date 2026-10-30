//! Running stitch along a line (satin columns live in `satin.rs`).

use crate::geom::{P, Polyline, resample_keep_corners};

/// Running stitch along `line`, `passes` times back and forth (odd = ends at the far end).
pub fn running(line: &[P], closed: bool, len: f64, passes: u32) -> Polyline {
    let mut base: Polyline = line.to_vec();
    if closed && base.len() > 2 {
        base.push(base[0]);
    }
    let one = resample_keep_corners(&base, len);
    let mut out = one.clone();
    for pass in 1..passes.max(1) {
        if pass % 2 == 1 {
            out.extend(one.iter().rev().skip(1));
        } else {
            out.extend(one.iter().skip(1));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_respects_length_and_passes() {
        let line = vec![P::new(0.0, 0.0), P::new(100.0, 0.0)];
        let r = running(&line, false, 25.0, 1);
        assert_eq!(r.len(), 5);
        let bean = running(&line, false, 25.0, 3);
        assert_eq!(bean.len(), 13);
        assert_eq!(bean.last().unwrap().x, 100.0);
    }
}
