//! Align and Distribute (Object › Align): line objects up by an edge or a
//! center, or space them evenly. A group that's wholly selected moves as
//! one object, as do the fill and the outline of one element.

use super::Document;
use crate::geom::{Bounds, P, translation};
use crate::svg::Geometry;
use anyhow::{Result, bail};
use std::collections::HashMap;

/// What to line objects up by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AlignEdge {
    Left,
    HCenter,
    Right,
    Top,
    VCenter,
    Bottom,
}

impl AlignEdge {
    pub const ALL: [AlignEdge; 6] = [
        AlignEdge::Left,
        AlignEdge::HCenter,
        AlignEdge::Right,
        AlignEdge::Top,
        AlignEdge::VCenter,
        AlignEdge::Bottom,
    ];
    pub fn label(self) -> &'static str {
        match self {
            AlignEdge::Left => "Align Left",
            AlignEdge::HCenter => "Align Horizontal Centers",
            AlignEdge::Right => "Align Right",
            AlignEdge::Top => "Align Top",
            AlignEdge::VCenter => "Align Vertical Centers",
            AlignEdge::Bottom => "Align Bottom",
        }
    }
    /// The name agents use.
    pub fn id(self) -> &'static str {
        match self {
            AlignEdge::Left => "left",
            AlignEdge::HCenter => "hcenter",
            AlignEdge::Right => "right",
            AlignEdge::Top => "top",
            AlignEdge::VCenter => "vcenter",
            AlignEdge::Bottom => "bottom",
        }
    }
    pub fn by_id(id: &str) -> Option<AlignEdge> {
        AlignEdge::ALL.into_iter().find(|e| e.id() == id)
    }
    /// How far a box `b` moves to line up with `frame`.
    fn delta(self, b: &Bounds, frame: &Bounds) -> P {
        match self {
            AlignEdge::Left => P::new(frame.min.x - b.min.x, 0.0),
            AlignEdge::HCenter => P::new(frame.center().x - b.center().x, 0.0),
            AlignEdge::Right => P::new(frame.max.x - b.max.x, 0.0),
            AlignEdge::Top => P::new(0.0, frame.min.y - b.min.y),
            AlignEdge::VCenter => P::new(0.0, frame.center().y - b.center().y),
            AlignEdge::Bottom => P::new(0.0, frame.max.y - b.max.y),
        }
    }
}

/// Which way to space objects evenly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DistributeAxis {
    Horizontal,
    Vertical,
}

impl DistributeAxis {
    pub const ALL: [DistributeAxis; 2] = [DistributeAxis::Horizontal, DistributeAxis::Vertical];
    pub fn label(self) -> &'static str {
        match self {
            DistributeAxis::Horizontal => "Distribute Horizontally",
            DistributeAxis::Vertical => "Distribute Vertically",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            DistributeAxis::Horizontal => "horizontal",
            DistributeAxis::Vertical => "vertical",
        }
    }
    pub fn by_id(id: &str) -> Option<DistributeAxis> {
        DistributeAxis::ALL.into_iter().find(|a| a.id() == id)
    }
}

/// Moves smaller than this (design units: 0.001 mm) aren't moves.
const EPS: f64 = 0.01;

impl Document {
    /// The things `indices` move as: each a list of shape uids with their
    /// combined box. A group every member of which is among them is one
    /// (the outermost such group); otherwise an element is (its fill and
    /// its outline together).
    fn arrange_units(&self, indices: &[usize]) -> Vec<(Vec<usize>, Bounds)> {
        let selected: std::collections::HashSet<usize> = indices
            .iter()
            .copied()
            .filter(|&i| i < self.shapes.len())
            .collect();
        // How many shapes each group has, and how many of them are selected.
        let mut size: HashMap<&str, (usize, usize)> = HashMap::new();
        for (i, s) in self.shapes.iter().enumerate() {
            for g in &s.groups {
                let e = size.entry(g.as_str()).or_default();
                e.0 += 1;
                e.1 += selected.contains(&i) as usize;
            }
        }
        let mut order: Vec<String> = Vec::new();
        let mut units: HashMap<String, (Vec<usize>, Bounds)> = HashMap::new();
        let mut sorted: Vec<usize> = selected.iter().copied().collect();
        sorted.sort_unstable();
        for i in sorted {
            let s = &self.shapes[i];
            let key = s
                .groups
                .iter()
                .find(|g| size.get(g.as_str()).is_some_and(|(n, sel)| n == sel))
                .map(|g| format!("g:{g}"))
                .unwrap_or_else(|| format!("e:{}", s.id));
            let u = units.entry(key.clone()).or_insert_with(|| {
                order.push(key);
                (Vec::new(), Bounds::empty())
            });
            u.0.push(s.uid);
            match &s.geometry {
                Geometry::Fill { rings, .. } => rings.iter().flatten().for_each(|&p| u.1.add(p)),
                Geometry::Stroke { lines, .. } => {
                    lines.iter().flat_map(|(l, _)| l).for_each(|&p| u.1.add(p))
                }
            }
        }
        order
            .into_iter()
            .filter_map(|k| units.remove(&k))
            .filter(|(_, b)| !b.is_empty())
            .collect()
    }

    /// How many things `indices` line up as (a wholly selected group, or
    /// an element's fill and outline together, is one).
    pub fn arrange_count(&self, indices: &[usize]) -> usize {
        self.arrange_units(indices).len()
    }

    /// Move the shapes with these uids by `d`.
    fn move_uids(&mut self, uids: &[usize], d: P) -> Result<()> {
        let idx: Vec<usize> = self
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| uids.contains(&s.uid))
            .map(|(i, _)| i)
            .collect();
        self.transform(&idx, translation(d))
    }

    /// Line objects up by `edge`: with each other (two or more), or, given
    /// a `frame` (the hoop), with that. Returns how many moved.
    pub fn align(
        &mut self,
        indices: &[usize],
        edge: AlignEdge,
        frame: Option<Bounds>,
    ) -> Result<usize> {
        let units = self.arrange_units(indices);
        let frame = match frame {
            Some(f) => {
                if units.is_empty() {
                    bail!("select an object to align");
                }
                f
            }
            None => {
                if units.len() < 2 {
                    bail!("select two or more objects to align them with each other");
                }
                let mut all = Bounds::empty();
                for (_, b) in &units {
                    all.add(b.min);
                    all.add(b.max);
                }
                all
            }
        };
        let mut moved = 0;
        for (uids, b) in units {
            let d = edge.delta(&b, &frame);
            if d.x.abs() > EPS || d.y.abs() > EPS {
                self.move_uids(&uids, d)?;
                moved += 1;
            }
        }
        Ok(moved)
    }

    /// Space three or more objects evenly along `axis`: the two at the ends
    /// stay, and the ones between get equal gaps. Returns how many moved.
    pub fn distribute(&mut self, indices: &[usize], axis: DistributeAxis) -> Result<usize> {
        let mut units = self.arrange_units(indices);
        if units.len() < 3 {
            bail!("select three or more objects to space them evenly");
        }
        let lo = |b: &Bounds| match axis {
            DistributeAxis::Horizontal => b.min.x,
            DistributeAxis::Vertical => b.min.y,
        };
        let size = |b: &Bounds| match axis {
            DistributeAxis::Horizontal => b.width(),
            DistributeAxis::Vertical => b.height(),
        };
        units.sort_by(|a, b| {
            (lo(&a.1) + size(&a.1) / 2.0).total_cmp(&(lo(&b.1) + size(&b.1) / 2.0))
        });
        let (first, last) = (&units[0].1, &units[units.len() - 1].1);
        let span = (lo(last) + size(last)) - lo(first);
        let filled: f64 = units.iter().map(|(_, b)| size(b)).sum();
        let gap = (span - filled) / (units.len() - 1) as f64;
        let mut at = lo(first);
        let mut moves = Vec::new();
        for (uids, b) in &units {
            let d = at - lo(b);
            if d.abs() > EPS {
                moves.push((
                    uids.clone(),
                    match axis {
                        DistributeAxis::Horizontal => P::new(d, 0.0),
                        DistributeAxis::Vertical => P::new(0.0, d),
                    },
                ));
            }
            at += size(b) + gap;
        }
        let moved = moves.len();
        for (uids, d) in moves {
            self.move_uids(&uids, d)?;
        }
        Ok(moved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Kind;

    fn rect(d: &mut Document, x: f64, y: f64, w: f64, h: f64) -> usize {
        let i = d
            .add_drawing(
                &[
                    P::new(x, y),
                    P::new(x + w, y),
                    P::new(x + w, y + h),
                    P::new(x, y + h),
                ],
                true,
                [0, 0, 0],
                Kind::Fill,
            )
            .unwrap();
        d.shapes[i].uid
    }

    fn bounds(d: &Document, uid: usize) -> Bounds {
        let s = d.shapes.iter().find(|s| s.uid == uid).unwrap();
        crate::svg::shapes_bounds(std::slice::from_ref(s))
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.05
    }

    #[test]
    fn align_lines_objects_up_with_each_other() {
        let mut d = Document::empty();
        let a = rect(&mut d, 0.0, 0.0, 100.0, 50.0);
        let b = rect(&mut d, 200.0, 100.0, 40.0, 40.0);
        let c = rect(&mut d, -50.0, 300.0, 20.0, 80.0);
        let all = [0, 1, 2];
        // Left: everything to the leftmost edge (c's, at -50). c stays.
        assert_eq!(d.align(&all, AlignEdge::Left, None).unwrap(), 2);
        for u in [a, b, c] {
            assert!(close(bounds(&d, u).min.x, -50.0), "{:?}", bounds(&d, u));
        }
        // Nothing moved up or down, and nothing changed size.
        assert!(close(bounds(&d, b).min.y, 100.0));
        assert!(close(bounds(&d, a).width(), 100.0));
        // Again: already there.
        assert_eq!(d.align(&all, AlignEdge::Left, None).unwrap(), 0);
        // Right edges, then centers across, then the three edges down.
        d.align(&all, AlignEdge::Right, None).unwrap();
        for u in [a, b, c] {
            assert!(close(bounds(&d, u).max.x, 50.0));
        }
        d.align(&all, AlignEdge::HCenter, None).unwrap();
        for u in [a, b, c] {
            assert!(close(bounds(&d, u).center().x, 0.0));
        }
        d.align(&all, AlignEdge::Top, None).unwrap();
        for u in [a, b, c] {
            assert!(close(bounds(&d, u).min.y, 0.0));
        }
        d.align(&all, AlignEdge::Bottom, None).unwrap();
        for u in [a, b, c] {
            assert!(close(bounds(&d, u).max.y, 80.0));
        }
        d.align(&all, AlignEdge::VCenter, None).unwrap();
        for u in [a, b, c] {
            assert!(close(bounds(&d, u).center().y, 40.0));
        }
        // One object has nothing to line up with, unless there's a frame.
        assert!(d.align(&[0], AlignEdge::Left, None).is_err());
        let hoop = Bounds {
            min: P::new(-500.0, -500.0),
            max: P::new(500.0, 500.0),
        };
        assert_eq!(d.align(&[0], AlignEdge::Left, Some(hoop)).unwrap(), 1);
        assert!(close(bounds(&d, a).min.x, -500.0));
        d.align(&[0], AlignEdge::VCenter, Some(hoop)).unwrap();
        assert!(close(bounds(&d, a).center().y, 0.0));
        // It all lands in the SVG.
        let back = Document::from_svg(&d.to_svg().unwrap(), "a", None).unwrap();
        assert_eq!(back.shapes.len(), 3);
    }

    #[test]
    fn distribute_evens_out_the_gaps() {
        let mut d = Document::empty();
        let a = rect(&mut d, 0.0, 0.0, 100.0, 50.0);
        let b = rect(&mut d, 110.0, 0.0, 50.0, 50.0);
        let c = rect(&mut d, 400.0, 0.0, 100.0, 50.0);
        // Span 0..500, 250 of it objects: two gaps of 125. Only b moves.
        assert_eq!(
            d.distribute(&[0, 1, 2], DistributeAxis::Horizontal)
                .unwrap(),
            1
        );
        assert!(close(bounds(&d, a).min.x, 0.0));
        assert!(close(bounds(&d, b).min.x, 225.0), "{:?}", bounds(&d, b));
        assert!(close(bounds(&d, c).min.x, 400.0));
        assert_eq!(
            d.distribute(&[0, 1, 2], DistributeAxis::Horizontal)
                .unwrap(),
            0,
            "already even"
        );
        // Down the page: whatever order they were selected in.
        let e = rect(&mut d, 0.0, 300.0, 50.0, 100.0);
        d.distribute(&[3, 0, 1], DistributeAxis::Vertical).unwrap();
        // a and b are both at y 0..50 and e at 300..400: whichever of the
        // two is in the middle gets a 100 gap on each side (150..200).
        let mid = if bounds(&d, a).min.y > 1.0 { a } else { b };
        assert!(close(bounds(&d, mid).min.y, 150.0), "{:?}", bounds(&d, mid));
        assert!(close(bounds(&d, e).min.y, 300.0));
        assert!(d.distribute(&[0, 1], DistributeAxis::Horizontal).is_err());
    }

    #[test]
    fn a_whole_group_moves_as_one() {
        const GROUPS: &str = include_str!("../../examples/groups.svg");
        let mut d = Document::from_svg(GROUPS, "g", None).unwrap();
        // A group with more than one member, and something outside it.
        let g = d
            .shapes
            .iter()
            .flat_map(|s| s.groups.first())
            .find(|g| d.shapes.iter().filter(|s| s.groups.contains(g)).count() > 1)
            .cloned()
            .expect("the example has a group");
        let members: Vec<usize> = (0..d.shapes.len())
            .filter(|&i| d.shapes[i].groups.contains(&g))
            .collect();
        let outside = (0..d.shapes.len())
            .find(|i| !members.contains(i))
            .expect("and an object outside it");
        let uids: Vec<usize> = members.iter().map(|&i| d.shapes[i].uid).collect();
        let before: Vec<Bounds> = uids.iter().map(|&u| bounds(&d, u)).collect();
        let target = bounds(&d, d.shapes[outside].uid).min.x;
        let mut sel = members.clone();
        sel.push(outside);
        d.align(&sel, AlignEdge::Left, None).unwrap();
        // The group's own left edge is on the line...
        let after: Vec<Bounds> = uids.iter().map(|&u| bounds(&d, u)).collect();
        let left = |v: &[Bounds]| v.iter().map(|b| b.min.x).fold(f64::INFINITY, f64::min);
        let line = left(&before).min(target);
        assert!(close(left(&after), line), "{} vs {line}", left(&after));
        // ...and its members kept their places relative to each other.
        let shift = after[0].min.x - before[0].min.x;
        for (a, b) in after.iter().zip(&before) {
            assert!(close(a.min.x - b.min.x, shift));
            assert!(close(a.min.y, b.min.y));
        }
        // Only part of the group selected: those objects line up singly.
        let two = [members[0], members[1]];
        d.align(&two, AlignEdge::Top, None).unwrap();
        let (a, b) = (
            bounds(&d, d.shapes[members[0]].uid),
            bounds(&d, d.shapes[members[1]].uid),
        );
        assert!(close(a.min.y, b.min.y));
    }
}
