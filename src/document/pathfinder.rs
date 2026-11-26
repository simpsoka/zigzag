//! Illustrator's Pathfinder, after VectorCraft's (`crates/pathops/src/
//! pathfinder.rs` and the engine's `run_pf`), on this document's terms: an
//! object is an element of the SVG, its look is its thread and stitch
//! settings, and what's in front is what's sewn later.
//!
//! - The **Shape Modes** (Unite, Minus Front, Intersect, Exclude) make one
//!   shape of several. Each operand is a list of objects: a group selected
//!   among other objects is one shape, the area of all its members.
//! - The **Pathfinders** (Divide, Trim, Merge, Crop, Outline, Minus Back)
//!   work on the filled shapes themselves, each object one shape. What's
//!   made of an object is sewn where it was, with its thread and settings.
//!
//! The geometry is the Build tools' (`build.rs`): areas flattened,
//! combined with i_overlay, and curves fitted to the result, keeping the
//! corners the objects had.

use super::Document;
use super::build::{
    Made, SAME, SLIVER, area_of, faces, fit, off_segment, op, parts_kept, slight, touches,
};
use super::paths::RESULT_TOL;
use crate::bezier::{self, Anchor, BezPath, SubPath};
use crate::boolean::{self, PathOp};
use crate::geom::{Bounds, FillRule, P, Polyline};
use crate::params::{Kind, Params};
use crate::svg::{Geometry, Part};
use anyhow::{Result, bail};

/// Every operation of Illustrator's Pathfinder panel, in its order: the
/// Shape Modes row, then the Pathfinders row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pathfinder {
    Unite,
    MinusFront,
    Intersect,
    Exclude,
    Divide,
    Trim,
    Merge,
    Crop,
    Outline,
    MinusBack,
}

/// Shapes this close (units) count as touching for Merge, which closes
/// the gap between them (as Unite does).
const TOUCHING: f64 = 1.0;

/// `rings` with gaps narrower than [`TOUCHING`] closed: grown over and
/// shrunk back, the outline elsewhere kept as it was. Shapes drawn side by
/// side, or snapped to edges a rounding apart, are one piece, not two in
/// one object (sewn as two, with a seam).
fn close_gaps(rings: Vec<Polyline>) -> Vec<Polyline> {
    let closed = boolean::offset(
        &boolean::offset(&rings, FillRule::NonZero, TOUCHING / 2.0),
        FillRule::NonZero,
        -TOUCHING / 2.0,
    );
    boolean::union_normalized(&[rings, closed])
}

/// The points of `rings` where the edges of two or more of `areas` meet:
/// corners of what's made there, whatever the fitting would make of them
/// (a curve round one bulges into the part beside it).
fn crossings(rings: &[Polyline], areas: &[Vec<Polyline>]) -> Vec<P> {
    let boxes: Vec<Bounds> = areas.iter().map(|a| bounds_of(a)).collect();
    rings
        .iter()
        .flatten()
        .filter(|&&p| {
            areas
                .iter()
                .zip(&boxes)
                .filter(|(a, b)| on_edge(p, a, b))
                .count()
                >= 2
        })
        .copied()
        .collect()
}

/// The box round `rings`.
fn bounds_of(rings: &[Polyline]) -> Bounds {
    let mut b = Bounds::empty();
    rings.iter().flatten().for_each(|&p| b.add(p));
    b
}

/// Whether boxes `a` and `b` come within `pad` of one another.
fn boxes_meet(a: &Bounds, b: &Bounds, pad: f64) -> bool {
    a.min.x <= b.max.x + pad
        && b.min.x <= a.max.x + pad
        && a.min.y <= b.max.y + pad
        && b.min.y <= a.max.y + pad
}

/// Whether `p` is on the edge of `area` (as `touches` has it), whose box
/// is `bounds`: most points are nowhere near most shapes, and the box says
/// so without going round every edge.
fn on_edge(p: P, area: &[Polyline], bounds: &Bounds) -> bool {
    let pad = 4.0 * SAME;
    p.x >= bounds.min.x - pad
        && p.x <= bounds.max.x + pad
        && p.y >= bounds.min.y - pad
        && p.y <= bounds.max.y + pad
        && touches(&[vec![p]], area)
}

impl Pathfinder {
    /// The Shape Modes row.
    pub const SHAPE_MODES: [Pathfinder; 4] = [
        Pathfinder::Unite,
        Pathfinder::MinusFront,
        Pathfinder::Intersect,
        Pathfinder::Exclude,
    ];
    /// The Pathfinders row.
    pub const PATHFINDERS: [Pathfinder; 6] = [
        Pathfinder::Divide,
        Pathfinder::Trim,
        Pathfinder::Merge,
        Pathfinder::Crop,
        Pathfinder::Outline,
        Pathfinder::MinusBack,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Pathfinder::Unite => "Unite",
            Pathfinder::MinusFront => "Minus Front",
            Pathfinder::Intersect => "Intersect",
            Pathfinder::Exclude => "Exclude",
            Pathfinder::Divide => "Divide",
            Pathfinder::Trim => "Trim",
            Pathfinder::Merge => "Merge",
            Pathfinder::Crop => "Crop",
            Pathfinder::Outline => "Outline",
            Pathfinder::MinusBack => "Minus Back",
        }
    }

    /// The name agents use.
    pub fn id(self) -> &'static str {
        match self {
            Pathfinder::Unite => "unite",
            Pathfinder::MinusFront => "minus_front",
            Pathfinder::Intersect => "intersect",
            Pathfinder::Exclude => "exclude",
            Pathfinder::Divide => "divide",
            Pathfinder::Trim => "trim",
            Pathfinder::Merge => "merge",
            Pathfinder::Crop => "crop",
            Pathfinder::Outline => "outline",
            Pathfinder::MinusBack => "minus_back",
        }
    }

    pub fn by_id(id: &str) -> Option<Pathfinder> {
        Self::SHAPE_MODES
            .into_iter()
            .chain(Self::PATHFINDERS)
            .find(|p| p.id() == id)
    }

    /// What it does, for its button's tooltip.
    pub fn help(self) -> &'static str {
        match self {
            Pathfinder::Unite => "Unite: one shape of them all, nothing sewn twice",
            Pathfinder::MinusFront => "Minus Front: the one sewn first, less what's sewn after it",
            Pathfinder::Intersect => "Intersect: only where they all overlap",
            Pathfinder::Exclude => "Exclude: where an odd number of them overlap",
            Pathfinder::Divide => {
                "Divide: a shape of every part they cut each other into (lines selected cut them too, and stay)"
            }
            Pathfinder::Trim => "Trim: cut away what's hidden under shapes sewn later",
            Pathfinder::Merge => {
                "Merge: trim, then join touching shapes in the same thread and stitch"
            }
            Pathfinder::Crop => "Crop: keep what's inside the shape sewn last, and drop that shape",
            Pathfinder::Outline => "Outline: a running stitch along every edge, sewn once",
            Pathfinder::MinusBack => "Minus Back: the one sewn last, less what's sewn before it",
        }
    }

    /// The shape mode it is, for the operations that make one shape.
    pub fn shape_op(self) -> Option<PathOp> {
        match self {
            Pathfinder::Unite => Some(PathOp::Unite),
            Pathfinder::MinusFront => Some(PathOp::MinusFront),
            Pathfinder::Intersect => Some(PathOp::Intersect),
            Pathfinder::Exclude => Some(PathOp::Exclude),
            Pathfinder::MinusBack => Some(PathOp::MinusBack),
            _ => None,
        }
    }

    /// How many objects it needs: Divide and Outline work on one.
    pub fn min_objects(self) -> usize {
        match self {
            Pathfinder::Divide | Pathfinder::Outline => 1,
            _ => 2,
        }
    }
}

/// The groups (outermost first) that every one of `lists` is in.
fn common_groups<'a>(mut lists: impl Iterator<Item = &'a [String]>) -> usize {
    let Some(first) = lists.next() else { return 0 };
    lists.fold(first.len(), |n, g| {
        first
            .iter()
            .zip(g)
            .take(n)
            .take_while(|(a, b)| a == b)
            .count()
    })
}

impl Document {
    /// Illustrator's Pathfinder Shape Modes, each object one shape:
    /// [`Document::shape_mode`] with an operand per object.
    pub fn pathfinder(&mut self, indices: &[usize], op: PathOp) -> Result<usize> {
        let operands: Vec<Vec<usize>> = indices.iter().map(|&i| vec![i]).collect();
        self.shape_mode(&operands, op)
    }

    /// Run Pathfinder operation `op` on `operands` (lists of objects: a
    /// group selected among others is one operand). Returns the indices of
    /// what it made or left, to select.
    pub fn run_pathfinder(
        &mut self,
        operands: &[Vec<usize>],
        op: Pathfinder,
    ) -> Result<Vec<usize>> {
        if let Some(mode) = op.shape_op() {
            return Ok(vec![self.shape_mode(operands, mode)?]);
        }
        let mut all: Vec<usize> = operands.iter().flatten().copied().collect();
        all.sort_unstable();
        all.dedup();
        // All or nothing: a refusal part-way leaves the document as it was.
        let mut next = self.clone();
        let made = match op {
            Pathfinder::Divide => next.divide(&all)?,
            Pathfinder::Outline => next.outline_edges(&all)?,
            _ => next.trim_shapes(&all, op)?,
        };
        *self = next;
        Ok(made)
    }

    /// Objects among `indices` as uids, with any text made outlines first
    /// (each in the same place in the list).
    fn pathfinder_uids(&mut self, indices: &[usize]) -> Result<Vec<usize>> {
        let n = indices.iter().filter(|&&i| i < self.shapes.len()).count();
        let indices: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|&i| i < self.shapes.len())
            .collect();
        let after = self.outlines_first(&indices)?;
        if after.len() != n {
            bail!("couldn't make the text outlines");
        }
        Ok(after.iter().map(|&i| self.shapes[i].uid).collect())
    }

    /// A Shape Mode on `operands` (back to front by where each is first
    /// sewn): one filled shape that replaces them, sewn where the backmost
    /// was, inside the groups they're all in. It has the look of the
    /// frontmost (for Minus Front, the backmost; for a group, its member
    /// sewn last). Returns its index.
    pub fn shape_mode(&mut self, operands: &[Vec<usize>], op: PathOp) -> Result<usize> {
        let operands: Vec<Vec<usize>> = operands
            .iter()
            .map(|o| {
                o.iter()
                    .copied()
                    .filter(|&i| i < self.shapes.len())
                    .collect::<Vec<_>>()
            })
            .filter(|o: &Vec<usize>| !o.is_empty())
            .collect();
        if operands.len() < 2 {
            bail!("select two or more overlapping objects");
        }
        let flat: Vec<usize> = operands.iter().flatten().copied().collect();
        let uids = self.pathfinder_uids(&flat)?;
        let mut k = 0;
        let groups: Vec<Vec<usize>> = operands
            .iter()
            .map(|o| {
                let g = uids[k..k + o.len()].to_vec();
                k += o.len();
                g
            })
            .collect();
        self.sync()?;
        let mut idx: Vec<Vec<usize>> = groups.iter().map(|g| self.uid_indices(g)).collect();
        idx.retain(|o| !o.is_empty());
        idx.sort_by_key(|o| o[0]);
        let parts: Vec<(Vec<Polyline>, FillRule)> = idx
            .iter()
            .map(|o| match o.as_slice() {
                [i] => self.area(*i),
                many => (
                    boolean::union(&many.iter().map(|&i| self.area(i)).collect::<Vec<_>>()),
                    FillRule::EvenOdd,
                ),
            })
            .collect();
        let mut rings = boolean::pathfinder(&parts, op);
        if op == PathOp::Unite {
            // (Shapes a hair apart are one shape, as for Merge.)
            rings = close_gaps(rings);
        }
        let members: Vec<usize> = idx.iter().flatten().copied().collect();
        let mut corners: Vec<P> = members.iter().flat_map(|&i| self.corners_of(i)).collect();
        let areas: Vec<Vec<Polyline>> = parts.iter().map(|(r, _)| r.clone()).collect();
        corners.extend(crossings(&rings, &areas));
        let path = fit(&rings, &corners);
        if path.is_empty() {
            bail!(match op {
                PathOp::Intersect => "the objects don't all overlap",
                _ => "nothing would be left",
            });
        }
        let look = match op {
            PathOp::MinusFront => *idx[0].last().unwrap(),
            _ => *idx.last().unwrap().last().unwrap(),
        };
        let keep = common_groups(members.iter().map(|&i| self.shapes[i].groups.as_slice()));
        let made = Made {
            path,
            beside: idx[0][0],
            color: self.shapes[look].color,
            params: self.made_params(look),
            name: self.name_of(look),
            keep: Some(keep),
            line: None,
        };
        // (With their outlines: those were the old shapes' edges, not the
        // new one's.)
        let gone = self.with_outlines(&members);
        let at = self.insert_made(vec![made], &gone)?;
        at.first()
            .copied()
            .ok_or_else(|| anyhow::anyhow!("the result has no area"))
    }

    /// The filled shapes among `indices` (back to front) and the lines
    /// (strokes that aren't a shape's outline), as uids, text outlined.
    fn shapes_and_lines(&mut self, indices: &[usize]) -> Result<(Vec<usize>, Vec<usize>)> {
        let uids = self.pathfinder_uids(indices)?;
        self.sync()?;
        let idx = self.uid_indices(&uids);
        let shapes = idx
            .iter()
            .filter(|&&i| self.builds(i))
            .map(|&i| self.shapes[i].uid)
            .collect();
        let lines = idx
            .iter()
            .filter(|&&i| {
                let s = &self.shapes[i];
                s.part == Part::Stroke
                    && !s.outline
                    && matches!(s.geometry, Geometry::Stroke { .. })
            })
            .map(|&i| self.shapes[i].uid)
            .collect();
        Ok((shapes, lines))
    }

    /// Pathfinder › Divide: every part the shapes cut each other into (and
    /// any lines selected cut them into) becomes a shape, with the look of
    /// the shape in front there, sewn where that one was. The shapes go;
    /// the lines stay. Returns the parts' indices.
    fn divide(&mut self, indices: &[usize]) -> Result<Vec<usize>> {
        let (shapes, lines) = self.shapes_and_lines(indices)?;
        if shapes.is_empty() {
            bail!("select filled shapes to divide");
        }
        let inputs = self.uid_indices(&shapes);
        let cutters: Vec<Polyline> = self
            .uid_indices(&lines)
            .into_iter()
            .flat_map(|i| match &self.shapes[i].geometry {
                Geometry::Stroke { lines, .. } => lines
                    .iter()
                    .map(|(l, closed)| {
                        let mut l = l.clone();
                        if *closed && let Some(&first) = l.first() {
                            l.push(first);
                        }
                        l
                    })
                    .collect(),
                Geometry::Fill { .. } => Vec::new(),
            })
            .collect();
        let areas: Vec<Vec<Polyline>> = inputs.iter().map(|&i| self.clean_area(i)).collect();
        let mut corners: Vec<P> = inputs.iter().flat_map(|&i| self.corners_of(i)).collect();
        let mut parts: Vec<(Vec<Polyline>, usize)> = Vec::new();
        let faces = faces(&areas);
        // (Where one shape's edge crosses another's is a corner of every
        // part there, as Trim has it: else a curve fitted round it bulges
        // into the part beside.)
        for face in &faces {
            corners.extend(crossings(&face.rings, &areas));
        }
        for face in faces {
            let top = *face.sources.last().expect("a face has a source");
            if cutters.is_empty() {
                parts.push((face.rings, top));
                continue;
            }
            for piece in boolean::slice(&face.rings, FillRule::EvenOdd, &cutters) {
                if area_of(&piece) > SLIVER {
                    parts.push((piece, top));
                }
            }
        }
        if !cutters.is_empty() {
            // Where a cut meets an edge is a corner.
            let on_cut = |p: P| {
                cutters
                    .iter()
                    .any(|c| c.windows(2).any(|w| off_segment(p, w[0], w[1]) <= SAME))
            };
            for (rings, _) in &parts {
                for r in rings {
                    let n = r.len();
                    corners.extend((0..n).filter_map(|k| {
                        let beside = [r[(k + n - 1) % n], r[(k + 1) % n]];
                        (on_cut(r[k]) && beside.iter().any(|&q| !on_cut(q))).then_some(r[k])
                    }));
                }
            }
        }
        // Each shape's parts, in order, after it: the first takes a trim
        // before it, the last a trim or stop after.
        parts.sort_by_key(|(_, k)| *k);
        let mut made: Vec<Made> = Vec::new();
        let mut from: Vec<usize> = Vec::new();
        for (rings, k) in parts {
            let i = inputs[k];
            let path = fit(&rings, &corners);
            if path.is_empty() {
                continue;
            }
            made.push(Made {
                path,
                beside: i,
                color: self.shapes[i].color,
                params: self.made_params(i),
                name: self.name_of(i),
                keep: None,
                line: None,
            });
            from.push(i);
        }
        if made.is_empty() {
            bail!("nothing would be left");
        }
        self.carry_trims(&mut made, &from);
        // (The lines stay: unlike Illustrator's cutting paths, a line
        // here is sewn, a satin or a running stitch of its own.)
        let gone = self.with_outlines(&inputs);
        self.insert_made(made, &gone)
    }

    /// What's made of each object among `made` (`from`: the object each
    /// is made of, in order) takes the trims and stops said of that
    /// object: a trim before the first of them, a trim or stop after the
    /// last.
    fn carry_trims(&self, made: &mut [Made], from: &[usize]) {
        for (k, &i) in from.iter().enumerate() {
            let own = &self.shapes[i].params;
            if k == 0 || from[k - 1] != i {
                made[k].params.trim_before = own.trim_before;
            }
            if from.get(k + 1) != Some(&i) {
                (made[k].params.trim, made[k].params.stop) = (own.trim, own.stop);
            }
        }
    }

    /// Pathfinder › Trim, Merge and Crop. Each shape loses what's hidden
    /// under the shapes in front of it, keeping its place, thread and
    /// settings (a shape hidden altogether goes), and the outline of a
    /// shape that changes goes (it ran round what's hidden now). Merge
    /// then makes one shape of each set of touching shapes in the same
    /// thread and stitch type: the biggest, in its place. Crop keeps of
    /// each only what's inside the frontmost, which goes. Returns the
    /// indices of the shapes left.
    fn trim_shapes(&mut self, indices: &[usize], which: Pathfinder) -> Result<Vec<usize>> {
        let (shapes, _) = self.shapes_and_lines(indices)?;
        if shapes.len() < 2 {
            bail!("select two or more filled shapes");
        }
        let inputs = self.uid_indices(&shapes);
        let areas: Vec<Vec<Polyline>> = inputs.iter().map(|&i| self.clean_area(i)).collect();
        let n = inputs.len();
        // The shapes worked on, and what's in front of each (for Crop, the
        // cropping shape is neither).
        let (work, crop) = match which {
            Pathfinder::Crop => (n - 1, Some(&areas[n - 1])),
            _ => (n, None),
        };
        let mut corners: Vec<P> = inputs.iter().flat_map(|&i| self.corners_of(i)).collect();
        // Each shape's area left, `None` where it's unchanged.
        let mut left: Vec<Option<Vec<Polyline>>> = vec![None; work];
        let mut above: Vec<Polyline> = Vec::new();
        for k in (0..work).rev() {
            let mut cut = areas[k].clone();
            let mut changed = false;
            if !above.is_empty() && !op(&cut, &above, PathOp::Intersect).is_empty() {
                cut = op(&cut, &above, PathOp::MinusFront);
                changed = true;
            }
            if let Some(c) = crop
                && !op(&cut, c, PathOp::MinusFront).is_empty()
            {
                cut = op(&cut, c, PathOp::Intersect);
                changed = true;
            }
            if changed {
                // (Less the slight bits left against what's in front.)
                let mut against = above.clone();
                if let Some(c) = crop {
                    against.extend(c.iter().cloned());
                }
                let kept: Vec<Polyline> = parts_kept(&cut)
                    .into_iter()
                    .filter(|p| !(slight(p) && touches(p, &against)))
                    .flatten()
                    .collect();
                left[k] = Some(kept);
            }
            above = if above.is_empty() {
                areas[k].clone()
            } else {
                boolean::union_normalized(&[above, areas[k].clone()])
            };
        }
        // (Where one shape's edge crosses another's is a corner of both.)
        let boxes: Vec<Bounds> = areas.iter().map(|a| bounds_of(a)).collect();
        for (k, l) in left.iter().enumerate() {
            if let Some(l) = l {
                corners.extend(
                    l.iter()
                        .flatten()
                        .filter(|p| {
                            areas
                                .iter()
                                .zip(&boxes)
                                .enumerate()
                                .any(|(j, (a, b))| j != k && on_edge(**p, a, b))
                        })
                        .copied(),
                );
            }
        }
        let mut gone: Vec<usize> = Vec::new();
        let mut edits: Vec<(usize, BezPath)> = Vec::new();
        let now = |k: usize, left: &[Option<Vec<Polyline>>]| -> Vec<Polyline> {
            left[k].clone().unwrap_or_else(|| areas[k].clone())
        };
        if which == Pathfinder::Merge {
            // Touching shapes alike: one set each.
            let alike = |a: usize, b: usize| {
                let (x, y) = (&self.shapes[inputs[a]], &self.shapes[inputs[b]]);
                x.color == y.color
                    && x.params.kind.unwrap_or(Kind::Fill) == y.params.kind.unwrap_or(Kind::Fill)
            };
            let areas_now: Vec<Vec<Polyline>> = (0..n).map(|k| now(k, &left)).collect();
            // (Each shape grown once, when first asked about, and only
            // against the shapes whose boxes come that near.)
            let boxes_now: Vec<Bounds> = areas_now.iter().map(|a| bounds_of(a)).collect();
            let mut grown: Vec<Option<Vec<Polyline>>> = vec![None; n];
            let mut near = |a: usize, b: usize| {
                if areas_now[a].is_empty()
                    || areas_now[b].is_empty()
                    || !boxes_meet(&boxes_now[a], &boxes_now[b], TOUCHING)
                {
                    return false;
                }
                let g = grown[a].get_or_insert_with(|| {
                    boolean::offset(&areas_now[a], FillRule::EvenOdd, TOUCHING)
                });
                !op(g, &areas_now[b], PathOp::Intersect).is_empty()
            };
            let mut set: Vec<usize> = (0..n).collect();
            fn root(set: &mut [usize], k: usize) -> usize {
                let mut r = k;
                while set[r] != r {
                    r = set[r];
                }
                set[k] = r;
                r
            }
            for a in 0..n {
                for b in a + 1..n {
                    if alike(a, b) && root(&mut set, a) != root(&mut set, b) && near(a, b) {
                        let (ra, rb) = (root(&mut set, a), root(&mut set, b));
                        set[rb] = ra;
                    }
                }
            }
            for r in 0..n {
                let members: Vec<usize> = (0..n).filter(|&k| root(&mut set, k) == r).collect();
                if members.len() < 2 {
                    continue;
                }
                let keeper = *members
                    .iter()
                    .max_by(|&&a, &&b| area_of(&areas_now[a]).total_cmp(&area_of(&areas_now[b])))
                    .unwrap();
                let joined = boolean::union_normalized(
                    &members
                        .iter()
                        .map(|&k| areas_now[k].clone())
                        .collect::<Vec<_>>(),
                );
                // Shapes counted as touching may be a hair apart (drawn
                // side by side, or snapped to edges a rounding apart): the
                // gap is closed, grown over and shrunk back, so they're one
                // piece, not two in one object. (What's closed is no wider
                // than what counts as touching; the outline elsewhere is
                // kept as it was.)
                let merged = close_gaps(joined);
                for &k in &members {
                    if k == keeper {
                        left[k] = Some(merged.clone());
                    } else {
                        left[k] = Some(Vec::new());
                    }
                }
            }
        }
        for (k, l) in left.into_iter().enumerate() {
            let Some(l) = l else { continue };
            let i = inputs[k];
            let path = fit(&l, &corners);
            if path.is_empty() {
                gone.push(i);
            } else {
                edits.push((i, path));
            }
        }
        if let Some(c) = crop.map(|_| inputs[n - 1]) {
            // (Nothing inside it: Crop would leave nothing at all.)
            if edits.is_empty() && gone.len() == work {
                bail!("nothing is inside the shape sewn last");
            }
            gone.push(c);
        }
        if edits.is_empty() && gone.is_empty() {
            bail!(match which {
                Pathfinder::Crop => "nothing is outside the shape sewn last",
                _ => "nothing is hidden under the shapes sewn later",
            });
        }
        let changed: Vec<usize> = edits.iter().map(|(i, _)| self.shapes[*i].uid).collect();
        let doomed: Vec<usize> = gone.iter().map(|&i| self.shapes[i].uid).collect();
        if !edits.is_empty() {
            self.set_paths(&edits)?;
        }
        // The shapes that went, and the outlines of those that changed.
        let mut remove = self.with_outlines(&self.uid_indices(&doomed));
        for i in self.uid_indices(&changed) {
            let s = &self.shapes[i];
            if let Some(o) = self
                .shapes
                .iter()
                .position(|o| o.id == s.id && o.part == Part::Stroke && !s.id.is_empty())
                && !remove.contains(&o)
            {
                remove.push(o);
            }
        }
        if !remove.is_empty() {
            self.delete_many(&remove)?;
        }
        self.sync()?;
        Ok(self.uid_indices(&shapes))
    }

    /// Pathfinder › Outline: every shape's edge, cut where the others'
    /// edges cross it, as a running stitch in its thread, sewn where the
    /// shape was; an edge two shapes share is sewn once, with the one
    /// sewn first. The shapes go. Returns the outlines' indices.
    fn outline_edges(&mut self, indices: &[usize]) -> Result<Vec<usize>> {
        let (shapes, _) = self.shapes_and_lines(indices)?;
        if shapes.is_empty() {
            bail!("select filled shapes to outline");
        }
        let inputs = self.uid_indices(&shapes);
        let areas: Vec<Vec<Polyline>> = inputs.iter().map(|&i| self.clean_area(i)).collect();
        let corners: Vec<P> = inputs.iter().flat_map(|&i| self.corners_of(i)).collect();
        // The edges kept so far, for the ones shared to be sewn once.
        let mut kept: Vec<Polyline> = Vec::new();
        let mut made: Vec<Made> = Vec::new();
        let mut from: Vec<usize> = Vec::new();
        for (k, rings) in areas.iter().enumerate() {
            let others: Vec<&Polyline> = areas
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != k)
                .flat_map(|(_, a)| a.iter())
                .collect();
            let mut path: BezPath = Vec::new();
            for ring in rings.iter().filter(|r| r.len() >= 3) {
                let pieces = split_ring(ring, &others);
                // The pieces not along an edge kept already, joined again
                // where they follow on from one another.
                let mut runs: Vec<Polyline> = Vec::new();
                let whole = pieces.len() == 1;
                for piece in pieces {
                    if along_any(&piece, &kept) {
                        continue;
                    }
                    match runs.last_mut() {
                        Some(r) if r.last().is_some_and(|q| q.dist(piece[0]) <= SAME) => {
                            r.extend(piece.into_iter().skip(1));
                        }
                        _ => runs.push(piece),
                    }
                }
                // (Round the end to the start of the ring again.)
                if runs.len() >= 2 && runs[runs.len() - 1].last().unwrap().dist(runs[0][0]) <= SAME
                {
                    let last = runs.pop().unwrap();
                    let first = std::mem::take(&mut runs[0]);
                    runs[0] = last.into_iter().chain(first.into_iter().skip(1)).collect();
                }
                for run in runs {
                    // (What's left of an edge sewn already, against its
                    // ends, isn't one to sew.)
                    let long: f64 = run.windows(2).map(|w| w[0].dist(w[1])).sum();
                    if long < MIN_EDGE {
                        continue;
                    }
                    let closed = whole && run.first().unwrap().dist(*run.last().unwrap()) <= SAME;
                    kept.push(run.clone());
                    if let Some(sp) = fit_line(&run, closed, &corners) {
                        path.push(sp);
                    }
                }
            }
            if path.is_empty() {
                continue;
            }
            let i = inputs[k];
            let width = crate::params::defaults::OUTLINE_SATIN_WIDTH;
            made.push(Made {
                path,
                beside: i,
                color: self.shapes[i].color,
                params: Params {
                    kind: Some(Kind::Run),
                    ..Default::default()
                },
                name: format!("{}_outline", self.name_of(i)),
                keep: None,
                line: Some(width),
            });
            from.push(i);
        }
        if made.is_empty() {
            bail!("nothing to outline");
        }
        self.carry_trims(&mut made, &from);
        let gone = self.with_outlines(&inputs);
        self.insert_made(made, &gone)
    }
}

/// The shortest edge Outline sews (units: 1 mm).
const MIN_EDGE: f64 = 10.0;

/// An open run of points (or a closed one) as a path: corners where the
/// shapes had them, and at both ends; smooth curves fitted between.
fn fit_line(run: &[P], closed: bool, corners: &[P]) -> Option<SubPath> {
    if run.len() < 2 {
        return None;
    }
    if closed {
        let ring = &run[..run.len() - 1];
        return fit(&[ring.to_vec()], corners).into_iter().next();
    }
    // Cut at the corners, each stretch fitted by itself.
    let n = run.len();
    let mut at: Vec<usize> = (1..n - 1)
        .filter(|&k| corners.iter().any(|c| c.dist(run[k]) <= SAME))
        .collect();
    at.insert(0, 0);
    at.push(n - 1);
    let mut anchors: Vec<Anchor> = Vec::new();
    for w in at.windows(2) {
        let stretch = &run[w[0]..=w[1]];
        let fitted = match stretch.len() {
            2 => None,
            _ => bezier::fit_outline(stretch, false, RESULT_TOL, 40.0),
        };
        let mut s = match fitted {
            Some(sp) if sp.anchors.len() >= 2 => sp.anchors,
            _ => vec![
                Anchor::corner(stretch[0]),
                Anchor::corner(*stretch.last().unwrap()),
            ],
        };
        let last = s.len() - 1;
        (s[0].p, s[last].p) = (stretch[0], *stretch.last().unwrap());
        (s[0].h_in, s[last].h_out) = (s[0].p, s[last].p);
        match anchors.last_mut() {
            Some(end) => {
                end.h_out = s[0].h_out;
                anchors.extend(s.into_iter().skip(1));
            }
            None => anchors = s,
        }
    }
    (anchors.len() >= 2).then_some(SubPath {
        anchors,
        closed: false,
    })
}

/// Where segment `a`–`b` crosses `c`–`d`, as how far along `a`–`b` (0–1).
fn crossing(a: P, b: P, c: P, d: P) -> Option<f64> {
    let r = b - a;
    let s = d - c;
    let den = r.x * s.y - r.y * s.x;
    if den.abs() < 1e-12 {
        return None;
    }
    let q = c - a;
    let t = (q.x * s.y - q.y * s.x) / den;
    let u = (q.x * r.y - q.y * r.x) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some(t)
}

/// A closed ring cut wherever `others`' edges cross it or one of their
/// points lies on it: its pieces, in order round it (one, closed, if
/// nothing does).
fn split_ring(ring: &[P], others: &[&Polyline]) -> Vec<Polyline> {
    let n = ring.len();
    let bounds = |r: &[P]| {
        let mut b = crate::geom::Bounds::empty();
        r.iter().for_each(|&p| b.add(p));
        b
    };
    let rb = bounds(ring);
    let near: Vec<&Polyline> = others
        .iter()
        .copied()
        .filter(|o| {
            let b = bounds(o);
            !(b.min.x > rb.max.x + SAME
                || rb.min.x > b.max.x + SAME
                || b.min.y > rb.max.y + SAME
                || rb.min.y > b.max.y + SAME)
        })
        .collect();
    // Each cut: (segment, how far along it).
    let mut cuts: Vec<(usize, f64)> = Vec::new();
    for k in 0..n {
        let (a, b) = (ring[k], ring[(k + 1) % n]);
        let len = a.dist(b);
        if len < 1e-9 {
            continue;
        }
        for o in &near {
            let m = o.len();
            for j in 0..m {
                let (c, d) = (o[j], o[(j + 1) % m]);
                if let Some(t) = crossing(a, b, c, d) {
                    cuts.push((k, t));
                }
                if off_segment(c, a, b) <= SAME {
                    let t = (c - a).dot(b - a) / (len * len);
                    cuts.push((k, t.clamp(0.0, 1.0)));
                }
            }
        }
    }
    if cuts.is_empty() {
        let mut whole = ring.to_vec();
        whole.push(ring[0]);
        return vec![whole];
    }
    // Places round the ring: segment and how far along it, as one number.
    let mut at: Vec<f64> = cuts.into_iter().map(|(k, t)| k as f64 + t).collect();
    at.sort_by(f64::total_cmp);
    let point = |s: f64| {
        let k = (s.floor() as usize) % n;
        let (a, b) = (ring[k], ring[(k + 1) % n]);
        a + (b - a) * (s - s.floor())
    };
    // Cuts that are one point are one cut.
    let mut cut: Vec<f64> = Vec::new();
    for s in at {
        if cut.last().is_none_or(|&l| point(l).dist(point(s)) > SAME) {
            cut.push(s);
        }
    }
    if cut.len() > 1 && point(cut[0]).dist(point(*cut.last().unwrap())) <= SAME {
        cut.pop();
    }
    let mut pieces = Vec::with_capacity(cut.len());
    for w in 0..cut.len() {
        let from = cut[w];
        let mut to = cut[(w + 1) % cut.len()];
        if to <= from {
            to += n as f64;
        }
        let mut piece = vec![point(from)];
        let mut v = from.floor() as usize + 1;
        while (v as f64) < to {
            piece.push(ring[v % n]);
            v += 1;
        }
        piece.push(point(to));
        piece.dedup_by(|a, b| a.dist(*b) <= 1e-9);
        if piece.len() >= 2 {
            pieces.push(piece);
        }
    }
    pieces
}

/// Whether the whole of `piece` lies along one of `kept` (within a
/// hair): its points, and the middles of its segments.
fn along_any(piece: &[P], kept: &[Polyline]) -> bool {
    let tol = 4.0 * SAME;
    kept.iter().any(|k| {
        let on = |p: P| k.windows(2).any(|w| off_segment(p, w[0], w[1]) <= tol);
        piece.iter().all(|&p| on(p)) && piece.windows(2).all(|w| on((w[0] + w[1]) * 0.5))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(body: &str) -> Document {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="60mm" viewBox="0 0 60 60">{body}</svg>"#
        );
        Document::from_svg(&svg, "t", None).unwrap()
    }

    fn sq(id: &str, x: f64, y: f64, w: f64, fill: &str) -> String {
        format!(r#"<rect id="{id}" x="{x}" y="{y}" width="{w}" height="{w}" fill="{fill}"/>"#)
    }

    fn at(d: &Document, name: &str) -> usize {
        d.shapes
            .iter()
            .position(|s| s.id.starts_with(name))
            .unwrap_or_else(|| panic!("no {name}"))
    }

    /// The area of object `i`, mm².
    fn mm2(d: &Document, i: usize) -> f64 {
        let (rings, rule) = d.area(i);
        boolean::parts(&rings, rule)
            .iter()
            .map(|p| area_of(p))
            .sum::<f64>()
            / 100.0
    }

    /// How long object `i`'s lines are, mm.
    fn length(d: &Document, i: usize) -> f64 {
        match &d.shapes[i].geometry {
            Geometry::Stroke { lines, .. } => {
                lines
                    .iter()
                    .map(|(l, closed)| {
                        let open: f64 = l.windows(2).map(|w| w[0].dist(w[1])).sum();
                        open + if *closed {
                            l[l.len() - 1].dist(l[0])
                        } else {
                            0.0
                        }
                    })
                    .sum::<f64>()
                    / 10.0
            }
            Geometry::Fill { .. } => 0.0,
        }
    }

    const RED: [u8; 3] = [0xcc, 0, 0];
    const BLUE: [u8; 3] = [0, 0, 0xcc];

    fn two() -> Document {
        doc(&(sq("a", 0.0, 0.0, 20.0, "#cc0000") + &sq("b", 10.0, 10.0, 20.0, "#0000cc")))
    }

    /// It survives a save and reopening.
    fn reopens(d: &Document) {
        let again = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        assert_eq!(again.shapes.len(), d.shapes.len());
    }

    #[test]
    fn divide_makes_a_shape_of_every_part() {
        let mut d = two();
        let made = d
            .run_pathfinder(&[vec![0], vec![1]], Pathfinder::Divide)
            .unwrap();
        assert_eq!(made.len(), 3);
        assert_eq!(d.shapes.len(), 3);
        let mut parts: Vec<(f64, [u8; 3])> = d
            .shapes
            .iter()
            .enumerate()
            .map(|(i, s)| ((mm2(&d, i)).round(), s.color))
            .collect();
        parts.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(parts[0], (100.0, BLUE), "the overlap is the front one's");
        // The back one's part is sewn first, as it was.
        assert_eq!(d.shapes[0].color, RED);
        assert!((mm2(&d, 0) - 300.0).abs() < 1.0);
        reopens(&d);
    }

    #[test]
    fn divide_cuts_along_lines_selected_with_the_shapes() {
        let mut d = doc(&(sq("a", 0.0, 0.0, 20.0, "#cc0000")
            + r##"<path id="cut" d="M -5 10 L 25 10" stroke="#000" fill="none"/>"##));
        let cut = at(&d, "cut");
        let made = d
            .run_pathfinder(&[vec![0], vec![cut]], Pathfinder::Divide)
            .unwrap();
        assert_eq!(made.len(), 2);
        assert_eq!(d.shapes.len(), 3, "the line stays: it's sewn");
        for i in made {
            assert!((mm2(&d, i) - 200.0).abs() < 1.0, "{}", mm2(&d, i));
            assert_eq!(d.shapes[i].color, RED);
        }
    }

    #[test]
    fn trim_cuts_away_what_is_hidden() {
        let mut d = two();
        let a = d.shapes[0].uid;
        d.run_pathfinder(&[vec![0], vec![1]], Pathfinder::Trim)
            .unwrap();
        assert_eq!(d.shapes.len(), 2);
        assert_eq!(d.shapes[0].uid, a, "the back one is the object it was");
        assert!((mm2(&d, 0) - 300.0).abs() < 1.0, "{}", mm2(&d, 0));
        assert!((mm2(&d, 1) - 400.0).abs() < 1.0);
        // One hidden altogether goes.
        let mut d = doc(&(sq("a", 5.0, 5.0, 5.0, "#cc0000") + &sq("b", 0.0, 0.0, 20.0, "#0000cc")));
        d.run_pathfinder(&[vec![0], vec![1]], Pathfinder::Trim)
            .unwrap();
        assert_eq!(d.shapes.len(), 1);
        assert_eq!(d.shapes[0].color, BLUE);
        // Nothing hidden: nothing to do.
        let mut d = doc(&(sq("a", 0.0, 0.0, 5.0, "#cc0000") + &sq("b", 30.0, 0.0, 5.0, "#0000cc")));
        assert!(
            d.run_pathfinder(&[vec![0], vec![1]], Pathfinder::Trim)
                .is_err()
        );
        reopens(&d);
    }

    #[test]
    fn merge_joins_touching_shapes_in_one_thread_and_stitch() {
        let mut d = doc(&(sq("a", 0.0, 0.0, 20.0, "#cc0000")
            + &sq("b", 10.0, 10.0, 20.0, "#cc0000")
            + &sq("c", 15.0, 0.0, 10.0, "#0000cc")));
        d.run_pathfinder(&[vec![0], vec![1], vec![2]], Pathfinder::Merge)
            .unwrap();
        assert_eq!(d.shapes.len(), 2);
        let red = d.shapes.iter().position(|s| s.color == RED).unwrap();
        let blue = d.shapes.iter().position(|s| s.color == BLUE).unwrap();
        // The blue one in front covers 5 × 10 of the red.
        assert!((mm2(&d, red) - 650.0).abs() < 1.0, "{}", mm2(&d, red));
        assert!((mm2(&d, blue) - 100.0).abs() < 1.0);
        // Sewn as different stitches, they stay apart.
        let mut d =
            doc(&(sq("a", 0.0, 0.0, 20.0, "#cc0000") + &sq("b", 10.0, 10.0, 20.0, "#cc0000")));
        d.shapes[1].params.kind = Some(Kind::Satin);
        d.run_pathfinder(&[vec![0], vec![1]], Pathfinder::Merge)
            .unwrap();
        assert_eq!(d.shapes.len(), 2);
        reopens(&d);
    }

    #[test]
    fn merge_closes_a_hair_thin_gap_into_one_piece() {
        // Side by side a rounding apart (0.02 mm): one piece, not two in
        // one object sewn as two.
        for gap in [0.0, 0.001, 0.02, 0.08] {
            let mut d =
                doc(&(sq("a", 0.0, 0.0, 20.0, "#cc0000")
                    + &sq("b", 20.0 + gap, 0.0, 20.0, "#cc0000")));
            d.run_pathfinder(&[vec![0], vec![1]], Pathfinder::Merge)
                .unwrap();
            assert_eq!(d.shapes.len(), 1, "gap {gap}");
            let (rings, rule) = d.area(0);
            assert_eq!(
                boolean::parts(&rings, rule).len(),
                1,
                "gap {gap}: one piece"
            );
            assert!(
                (mm2(&d, 0) - 800.0 - 20.0 * gap).abs() < 1.0,
                "gap {gap}: {}",
                mm2(&d, 0)
            );
            assert_eq!(d.plan().trim_count(), 0, "gap {gap}");
            reopens(&d);
        }
        // Further apart than that, they aren't touching: nothing merged.
        let mut d =
            doc(&(sq("a", 0.0, 0.0, 20.0, "#cc0000") + &sq("b", 20.3, 0.0, 20.0, "#cc0000")));
        assert!(
            d.run_pathfinder(&[vec![0], vec![1]], Pathfinder::Merge)
                .is_err()
        );
    }

    #[test]
    fn crop_keeps_what_is_inside_the_front_shape() {
        let mut d = two();
        d.run_pathfinder(&[vec![0], vec![1]], Pathfinder::Crop)
            .unwrap();
        assert_eq!(d.shapes.len(), 1);
        assert_eq!(d.shapes[0].color, RED);
        assert!((mm2(&d, 0) - 100.0).abs() < 1.0);
        reopens(&d);
    }

    #[test]
    fn minus_back_keeps_the_front_less_the_back() {
        let mut d = two();
        let made = d
            .run_pathfinder(&[vec![0], vec![1]], Pathfinder::MinusBack)
            .unwrap();
        assert_eq!(d.shapes.len(), 1);
        assert_eq!(d.shapes[made[0]].color, BLUE);
        assert!((mm2(&d, made[0]) - 300.0).abs() < 1.0);
    }

    #[test]
    fn outline_runs_along_every_edge_once() {
        let mut d = two();
        let made = d
            .run_pathfinder(&[vec![0], vec![1]], Pathfinder::Outline)
            .unwrap();
        assert_eq!(made.len(), 2);
        assert_eq!(d.shapes.len(), 2);
        for &i in &made {
            assert_eq!(d.shapes[i].params.kind, Some(Kind::Run));
            assert!((length(&d, i) - 80.0).abs() < 0.5, "{}", length(&d, i));
        }
        // Side by side: the edge they share is sewn once.
        let mut d =
            doc(&(sq("a", 0.0, 0.0, 10.0, "#cc0000") + &sq("b", 10.0, 0.0, 10.0, "#0000cc")));
        let made = d
            .run_pathfinder(&[vec![0], vec![1]], Pathfinder::Outline)
            .unwrap();
        let total: f64 = made.iter().map(|&i| length(&d, i)).sum();
        assert!((total - 70.0).abs() < 0.5, "{total}");
        reopens(&d);
        // A shape in a hole just its size: its edge is the hole's, sewn
        // once, and nothing is left of it to sew.
        let mut d = doc(&(r##"<path id="a" d="M0 0H30V30H0Z M10 10V20H20V10Z" fill="#cc0000" fill-rule="evenodd"/>"##.to_string()
            + &sq("b", 10.0, 10.0, 10.0, "#0000cc")));
        let made = d
            .run_pathfinder(&[vec![0], vec![1]], Pathfinder::Outline)
            .unwrap();
        assert_eq!(made.len(), 1, "the hole's edge only");
        assert!(
            (length(&d, made[0]) - 160.0).abs() < 0.5,
            "{}",
            length(&d, made[0])
        );
    }

    #[test]
    fn a_group_among_other_objects_is_one_shape() {
        let mut d = doc(&format!(
            r#"<g id="grp">{}{}</g>{}"#,
            sq("a", 0.0, 0.0, 10.0, "#cc0000"),
            sq("b", 10.0, 0.0, 10.0, "#cc0000"),
            sq("c", 5.0, 0.0, 10.0, "#0000cc"),
        ));
        let (a, b, c) = (at(&d, "a"), at(&d, "b"), at(&d, "c"));
        let i = d
            .run_pathfinder(&[vec![a, b], vec![c]], Pathfinder::MinusFront)
            .unwrap()[0];
        assert_eq!(d.shapes.len(), 1);
        assert!((mm2(&d, i) - 100.0).abs() < 1.0, "{}", mm2(&d, i));
        assert_eq!(d.shapes[i].color, RED);
        assert!(
            d.shapes[i].groups.is_empty(),
            "not inside the group it took"
        );
    }

    #[test]
    fn what_is_made_stays_in_the_group_they_were_all_in() {
        let mut d = doc(&format!(
            r#"{}<g id="grp">{}{}</g>"#,
            sq("z", 40.0, 40.0, 5.0, "#00cc00"),
            sq("a", 0.0, 0.0, 10.0, "#cc0000"),
            sq("b", 5.0, 0.0, 10.0, "#0000cc"),
        ));
        let (a, b) = (at(&d, "a"), at(&d, "b"));
        let i = d
            .run_pathfinder(&[vec![a], vec![b]], Pathfinder::Unite)
            .unwrap()[0];
        assert_eq!(d.shapes[i].groups, vec!["grp".to_string()]);
        assert_eq!(i, 1, "sewn where the back one was");
        let made = {
            let mut d = doc(&format!(
                r#"<g id="grp">{}{}</g>"#,
                sq("a", 0.0, 0.0, 10.0, "#cc0000"),
                sq("b", 5.0, 0.0, 10.0, "#0000cc"),
            ));
            let m = d
                .run_pathfinder(&[vec![0], vec![1]], Pathfinder::Divide)
                .unwrap();
            m.iter()
                .map(|&i| d.shapes[i].groups.clone())
                .collect::<Vec<_>>()
        };
        assert!(made.iter().all(|g| g == &vec!["grp".to_string()]));
    }

    #[test]
    fn corners_stay_corners() {
        let mut d = two();
        let i = d
            .run_pathfinder(&[vec![0], vec![1]], Pathfinder::Unite)
            .unwrap()[0];
        let path = d.edit_path(i).unwrap();
        let anchors: Vec<&Anchor> = path.iter().flat_map(|sp| sp.anchors.iter()).collect();
        assert_eq!(anchors.len(), 8, "an L of two squares has 8 corners");
        assert!(anchors.iter().all(|a| !a.is_smooth()));
    }

    /// Every operation on pairs of overlapping shapes, and on the whole of
    /// each design, comes back the same when saved and reopened.
    fn pathfinder_holds_on(name: &str, svg: &str) {
        let base = Document::from_svg(svg, name, None).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let fills: Vec<usize> = (0..base.shapes.len())
            .filter(|&i| {
                base.builds(i) && !base.shapes[i].params.locked() && !base.shapes[i].params.hidden()
            })
            .collect();
        let comes_back = |d: &Document, what: &str| {
            let saved = d
                .to_svg()
                .unwrap_or_else(|e| panic!("{name}: {what}: {e:#}"));
            let again = Document::from_svg(&saved, name, None)
                .unwrap_or_else(|e| panic!("{name}: {what}: reopening: {e:#}"));
            assert_eq!(
                again.shapes.len(),
                d.shapes.len(),
                "{name}: {what}: objects"
            );
            for (a, b) in again.shapes.iter().zip(&d.shapes) {
                assert_eq!(a.color, b.color, "{name}: {what}: thread of {}", b.id);
                assert_eq!(
                    a.params.kind, b.params.kind,
                    "{name}: {what}: stitch of {}",
                    b.id
                );
            }
        };
        // Pairs that overlap, a few per design.
        let mut pairs = Vec::new();
        'outer: for (k, &a) in fills.iter().enumerate() {
            for &b in &fills[k + 1..] {
                let (x, y) = (base.clean_area(a), base.clean_area(b));
                if !op(&x, &y, PathOp::Intersect).is_empty() {
                    pairs.push(vec![a, b]);
                    if pairs.len() >= 6 {
                        break 'outer;
                    }
                    break;
                }
            }
        }
        let all: Vec<Pathfinder> = Pathfinder::SHAPE_MODES
            .into_iter()
            .chain(Pathfinder::PATHFINDERS)
            .collect();
        let (mut done, mut refused) = (0, 0);
        for pair in &pairs {
            for &p in &all {
                let mut d = base.clone();
                let operands: Vec<Vec<usize>> = pair.iter().map(|&i| vec![i]).collect();
                match d.run_pathfinder(&operands, p) {
                    Ok(made) => {
                        assert!(made.iter().all(|&i| i < d.shapes.len()), "{name}: {p:?}");
                        comes_back(&d, &format!("{p:?} of {pair:?}"));
                        done += 1;
                    }
                    Err(_) => refused += 1,
                }
            }
        }
        // The whole design at once, as a Select All.
        let mut whole = String::new();
        for p in [
            Pathfinder::Divide,
            Pathfinder::Trim,
            Pathfinder::Merge,
            Pathfinder::Outline,
        ] {
            let mut d = base.clone();
            let operands: Vec<Vec<usize>> = (0..d.shapes.len()).map(|i| vec![i]).collect();
            let t = std::time::Instant::now();
            let r = d.run_pathfinder(&operands, p);
            let ms = t.elapsed().as_millis();
            match r {
                Ok(_) => {
                    comes_back(&d, &format!("{p:?} of all"));
                    whole += &format!(
                        " {}={}→{} {ms}ms",
                        p.label(),
                        base.shapes.len(),
                        d.shapes.len()
                    );
                }
                Err(e) => whole += &format!(" {}: {e} {ms}ms", p.label()),
            }
        }
        eprintln!(
            "{name}: {} objects, {} pairs: {done} done, {refused} refused;{whole}",
            base.shapes.len(),
            pairs.len()
        );
    }

    #[test]
    #[ignore]
    fn designs_of_your_own_go_through_the_pathfinder() {
        let dir = std::env::var("ZIGZAG_DESIGNS").expect("ZIGZAG_DESIGNS: a folder of .svg files");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "svg") {
                let name = path.file_stem().unwrap().to_string_lossy().into_owned();
                pathfinder_holds_on(&name, &std::fs::read_to_string(&path).unwrap());
                seen += 1;
            }
        }
        assert!(seen > 0, "no .svg files in {dir}");
    }

    #[test]
    fn the_pathfinder_holds_on_the_example_badge() {
        pathfinder_holds_on(
            "badge",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg")),
        );
    }
}

#[cfg(test)]
mod sweep {
    use super::*;
    use crate::document::Document;

    fn doc(body: &str) -> Document {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="60mm" viewBox="0 0 60 60">{body}</svg>"#
        );
        Document::from_svg(&svg, "t", None).unwrap()
    }
    fn sq(id: &str, x: f64, y: f64, w: f64, fill: &str) -> String {
        format!(r#"<rect id="{id}" x="{x}" y="{y}" width="{w}" height="{w}" fill="{fill}"/>"#)
    }
    fn circ(id: &str, cx: f64, cy: f64, r: f64, fill: &str) -> String {
        format!(r#"<circle id="{id}" cx="{cx}" cy="{cy}" r="{r}" fill="{fill}"/>"#)
    }
    fn area(rings: &[Polyline]) -> f64 {
        boolean::parts(rings, FillRule::NonZero)
            .iter()
            .map(|p| area_of(p))
            .sum::<f64>()
            / 100.0
    }
    fn uni(v: &[Vec<Polyline>]) -> Vec<Polyline> {
        boolean::union_normalized(v)
    }

    /// Every operation over shapes overlapping, abutting, a hair apart,
    /// nested, apart, curved, with a hole, three at once: what's made
    /// covers what it should (to half a square millimetre), what's made
    /// of several pieces doesn't sew any twice, no slivers, no piece in
    /// two where the shapes it was made of touched, an outline sews every
    /// edge once, and it all reopens.
    #[test]
    fn every_op_over_layouts() {
        let layouts: Vec<(&str, String)> = vec![
            ("overlap", sq("a", 0.0, 0.0, 20.0, "#cc0000") + &sq("b", 10.0, 10.0, 20.0, "#0000cc")),
            ("abut", sq("a", 0.0, 0.0, 20.0, "#cc0000") + &sq("b", 20.0, 0.0, 20.0, "#0000cc")),
            ("hairgap", sq("a", 0.0, 0.0, 20.0, "#cc0000") + &sq("b", 20.02, 0.0, 20.0, "#0000cc")),
            ("hairgap-same", sq("a", 0.0, 0.0, 20.0, "#cc0000") + &sq("b", 20.02, 0.0, 20.0, "#cc0000")),
            ("nested", sq("a", 0.0, 0.0, 30.0, "#cc0000") + &sq("b", 10.0, 10.0, 10.0, "#0000cc")),
            ("disjoint", sq("a", 0.0, 0.0, 10.0, "#cc0000") + &sq("b", 30.0, 30.0, 10.0, "#0000cc")),
            ("circles", circ("a", 20.0, 20.0, 12.0, "#cc0000") + &circ("b", 32.0, 24.0, 12.0, "#0000cc")),
            ("same-overlap", sq("a", 0.0, 0.0, 20.0, "#cc0000") + &sq("b", 10.0, 10.0, 20.0, "#cc0000")),
            ("three", sq("a", 0.0, 0.0, 20.0, "#cc0000") + &sq("b", 10.0, 5.0, 20.0, "#00cc00") + &circ("c", 25.0, 15.0, 9.0, "#0000cc")),
            ("donut", r##"<path id="a" fill-rule="evenodd" fill="#cc0000" d="M0 0H30V30H0Z M10 10V20H20V10Z"/>"##.to_string() + &sq("b", 15.0, 15.0, 20.0, "#0000cc")),
            ("edge-share", sq("a", 0.0, 0.0, 20.0, "#cc0000") + &sq("b", 10.0, 0.0, 10.0, "#0000cc")),
        ];
        let ops = Pathfinder::SHAPE_MODES
            .into_iter()
            .chain(Pathfinder::PATHFINDERS);
        let ops: Vec<Pathfinder> = ops.collect();
        let mut bad = Vec::new();
        for (name, body) in &layouts {
            for &op in &ops {
                let mut d = doc(body);
                let n = d.shapes.len();
                let ins: Vec<Vec<Polyline>> = (0..n).map(|i| d.area(i).0).collect();
                let all = uni(&ins);
                let operands: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
                let r = d.run_pathfinder(&operands, op);
                let tag = format!("{name:12} {:11}", op.label());
                let Ok(_) = r else {
                    eprintln!("{tag} refused: {}", r.unwrap_err());
                    continue;
                };
                // Pieces in each result, seams (two pieces of one object
                // within 0.1 mm), slivers.
                let mut notes = Vec::new();
                let res: Vec<Vec<Polyline>> = (0..d.shapes.len())
                    .filter(|&i| !matches!(d.shapes[i].geometry, Geometry::Stroke { .. }))
                    .map(|i| d.area(i).0)
                    .collect();
                for (k, r) in res.iter().enumerate() {
                    let parts = boolean::parts(r, FillRule::NonZero);
                    for a in 0..parts.len() {
                        for b in a + 1..parts.len() {
                            let g = boolean::offset(&parts[a], FillRule::NonZero, 1.0);
                            if !op_(&g, &parts[b]).is_empty() {
                                notes.push(format!("seam in #{k}"));
                            }
                        }
                    }
                    for p in &parts {
                        let a = area_of(p) / 100.0;
                        if a < 0.5 {
                            notes.push(format!("sliver {a:.3}mm² in #{k}"));
                        }
                    }
                }
                let sum: f64 = res.iter().map(|r| area(r)).sum();
                let mut overlap = 0.0;
                for a in 0..res.len() {
                    for b in a + 1..res.len() {
                        overlap += area(&boolean::pathfinder(
                            &[
                                (res[a].clone(), FillRule::NonZero),
                                (res[b].clone(), FillRule::NonZero),
                            ],
                            PathOp::Intersect,
                        ));
                    }
                }
                let rest = |skip: usize| {
                    uni(&ins
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != skip)
                        .map(|(_, r)| r.clone())
                        .collect::<Vec<_>>())
                };
                let pf = |a: &[Polyline], b: &[Polyline], o: PathOp| {
                    boolean::pathfinder(
                        &[
                            (a.to_vec(), FillRule::NonZero),
                            (b.to_vec(), FillRule::NonZero),
                        ],
                        o,
                    )
                };
                let expect: Option<f64> = match op {
                    Pathfinder::Unite => Some(area(&all)),
                    Pathfinder::MinusFront => {
                        Some(area(&pf(&ins[0], &rest(0), PathOp::MinusFront)))
                    }
                    Pathfinder::MinusBack => {
                        Some(area(&pf(&ins[n - 1], &rest(n - 1), PathOp::MinusFront)))
                    }
                    Pathfinder::Intersect => {
                        Some(area(&ins.iter().skip(1).fold(ins[0].clone(), |acc, r| {
                            pf(&acc, r, PathOp::Intersect)
                        })))
                    }
                    Pathfinder::Divide | Pathfinder::Trim | Pathfinder::Merge => Some(area(&all)),
                    Pathfinder::Crop => {
                        Some(area(&pf(&rest(n - 1), &ins[n - 1], PathOp::Intersect)))
                    }
                    _ => None,
                };
                if let Some(e) = expect
                    && (sum - e).abs() > 0.5
                {
                    notes.push(format!("area {sum:.1} want {e:.1}"));
                }
                if op == Pathfinder::Outline {
                    let per = |rings: &[Polyline]| {
                        rings
                            .iter()
                            .map(|r| {
                                (0..r.len())
                                    .map(|k| r[k].dist(r[(k + 1) % r.len()]))
                                    .sum::<f64>()
                            })
                            .sum::<f64>()
                    };
                    let fs = faces(&ins);
                    let want =
                        (fs.iter().map(|f| per(&f.rings)).sum::<f64>() + per(&all)) / 2.0 / 10.0;
                    let got: f64 = d
                        .shapes
                        .iter()
                        .map(|s| match &s.geometry {
                            Geometry::Stroke { lines, .. } => lines
                                .iter()
                                .map(|(l, c)| {
                                    let mut x = crate::geom::polyline_length(l);
                                    if *c && l.len() > 1 {
                                        x += l[0].dist(*l.last().unwrap());
                                    }
                                    x
                                })
                                .sum::<f64>(),
                            _ => 0.0,
                        })
                        .sum::<f64>()
                        / 10.0;
                    if (got - want).abs() > 0.5 {
                        notes.push(format!("outline {got:.1} mm want {want:.1} mm"));
                    }
                }
                if !matches!(op, Pathfinder::Outline) && overlap > 0.5 {
                    notes.push(format!("results overlap {overlap:.1}mm²"));
                }
                let again = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
                if again.shapes.len() != d.shapes.len() {
                    notes.push("reopen differs".into());
                }
                let plan = d.plan();
                // (Exclude's pieces meet corner to corner: two pieces
                // touching at a point, as they should.)
                if op == Pathfinder::Exclude {
                    notes.retain(|n| !n.starts_with("seam"));
                }
                let line = format!(
                    "{tag} -> {} objects, trims {}, {}",
                    d.shapes.len(),
                    plan.trim_count(),
                    if notes.is_empty() {
                        "ok".to_string()
                    } else {
                        notes.join("; ")
                    }
                );
                if !notes.is_empty() {
                    bad.push(line.clone());
                }
                eprintln!("{line}");
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
        // Crop with nothing inside the front shape leaves everything be.
        let mut d =
            doc(&(sq("a", 0.0, 0.0, 10.0, "#cc0000") + &sq("b", 30.0, 30.0, 10.0, "#0000cc")));
        assert!(
            d.run_pathfinder(&[vec![0], vec![1]], Pathfinder::Crop)
                .is_err()
        );
        assert_eq!(d.shapes.len(), 2);
        // Unite of shapes a hair apart: one piece.
        let mut d =
            doc(&(sq("a", 0.0, 0.0, 20.0, "#cc0000") + &sq("b", 20.02, 0.0, 20.0, "#0000cc")));
        d.run_pathfinder(&[vec![0], vec![1]], Pathfinder::Unite)
            .unwrap();
        let (rings, rule) = d.area(0);
        assert_eq!(boolean::parts(&rings, rule).len(), 1);
    }

    fn op_(a: &[Polyline], b: &[Polyline]) -> Vec<Polyline> {
        boolean::pathfinder(
            &[
                (a.to_vec(), FillRule::NonZero),
                (b.to_vec(), FillRule::NonZero),
            ],
            PathOp::Intersect,
        )
    }
}
