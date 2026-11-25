//! Shapes made out of shapes, by hand on the canvas: the Knife (cut one
//! along a line drawn across it), the Blob Brush (paint one, joining the
//! fills in the same thread that the paint touches) and the Shape Builder
//! (make one of the parts overlapping shapes divide each other into).
//! After VectorCraft's `path.knife`, `path.blob` and `shapeBuilder.merge`,
//! on this document's own terms: an object is an element of the SVG, its
//! look is its thread and stitch settings, and what's in front is what's
//! sewn later.
//!
//! All three work on filled shapes (what's sewn as an area), as their
//! areas are: outlines flattened, combined, and curves fitted to the
//! result, like Pathfinder's.

use super::Document;
use super::paths::RESULT_TOL;
use crate::bezier::{self, Anchor, BezPath, SubPath};
use crate::boolean::{self, PathOp};
use crate::geom::{FillRule, IDENTITY, P, Polyline, affine_inv, affine_mul, point_inside};
use crate::params::{Kind, Params};
use crate::svg::{Geometry, Part};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::{HashMap, HashSet};

/// Pieces smaller than this aren't kept (units²: a twentieth of a square
/// millimetre, less than one stitch covers): what two outlines that should
/// have met exactly leave between them.
const SLIVER: f64 = 5.0;

/// How near a point of a result has to be to a corner of what it was made
/// from (or to the Knife's cut) to be that corner (or on it): units, well
/// over what the arithmetic moves a point by, well under a stitch.
const SAME: f64 = 0.05;

/// How far `p` is from the line `a`–`b`.
fn off_segment(p: P, a: P, b: P) -> f64 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 > 0.0 {
        ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.dist(a + ab * t)
}

/// One ring of a result as a path: what `bezier::fit_outline` makes of it,
/// except that every point that was a corner of what the result was made
/// from (`corners`) is a corner still, and each stretch between two of
/// them is fitted by itself (a straight one stays a line). Left to itself
/// the fitting takes a ring of a few points for a curve drawn coarsely,
/// and rounds a hexagon into a circle.
fn fit_ring(ring: &[P], corners: &[P]) -> Option<SubPath> {
    let n = ring.len();
    let kept: Vec<usize> = (0..n)
        .filter(|&k| corners.iter().any(|c| c.dist(ring[k]) <= SAME))
        .collect();
    if kept.is_empty() {
        return bezier::fit_outline(ring, true, RESULT_TOL, 40.0);
    }
    let mut anchors: Vec<Anchor> = Vec::new();
    for (j, &from) in kept.iter().enumerate() {
        let to = kept[(j + 1) % kept.len()];
        // From one corner round to the next (all the way round, for the
        // only one).
        let mut run = vec![ring[from]];
        let mut k = (from + 1) % n;
        while k != to {
            run.push(ring[k]);
            k = (k + 1) % n;
        }
        run.push(ring[to]);
        let fitted = match run.len() {
            2 => None,
            _ => bezier::fit_outline(&run, false, RESULT_TOL, 40.0),
        };
        let mut stretch = match fitted {
            Some(sp) if sp.anchors.len() >= 2 => sp.anchors,
            _ => vec![Anchor::corner(ring[from]), Anchor::corner(ring[to])],
        };
        // (Its ends are the corners themselves, whatever the fitting made
        // of them.)
        let last = stretch.len() - 1;
        (stretch[0].p, stretch[last].p) = (ring[from], ring[to]);
        (stretch[0].h_in, stretch[last].h_out) = (ring[from], ring[to]);
        match anchors.last_mut() {
            Some(end) => {
                end.h_out = stretch[0].h_out;
                anchors.extend(stretch.into_iter().skip(1));
            }
            None => anchors = stretch,
        }
    }
    // Round to the first corner again: one point, not two.
    let end = anchors.pop()?;
    anchors.first_mut()?.h_in = end.h_in;
    (anchors.len() >= 2).then_some(SubPath {
        anchors,
        closed: true,
    })
}

/// The outline of a shape made out of others, as a path (see `fit_ring`).
fn fit(rings: &[Polyline], corners: &[P]) -> BezPath {
    rings
        .iter()
        .filter(|r| r.len() >= 3)
        .filter_map(|r| fit_ring(r, corners))
        .collect()
}

/// The area of a shape given as its outline and then its holes.
fn area_of(rings: &[Polyline]) -> f64 {
    let mut a: Vec<f64> = rings.iter().map(|r| boolean::ring_area(r)).collect();
    a.sort_by(|x, y| y.total_cmp(x));
    match a.split_first() {
        Some((outer, holes)) => (outer - holes.iter().sum::<f64>()).max(0.0),
        None => 0.0,
    }
}

/// Whether a part of an area is too slight to be a shape of its own, or to
/// be picked out as one: under a square millimetre, or on the whole
/// thinner than a fifth of a millimetre (nothing sews in that). Where one
/// shape's edge runs a hair inside another's, or outside it, what's
/// between them is this.
fn slight(part: &[Polyline]) -> bool {
    let around: f64 = part
        .iter()
        .map(|r| {
            (0..r.len())
                .map(|k| r[k].dist(r[(k + 1) % r.len()]))
                .sum::<f64>()
        })
        .sum();
    let a = area_of(part);
    a < 100.0 || 2.0 * a < 2.0 * around
}

/// Whether part `a` comes right up to area `b`: a point of it on `b`'s edge.
fn touches(a: &[Polyline], b: &[Polyline]) -> bool {
    a.iter().flatten().any(|&p| {
        b.iter()
            .any(|r| (0..r.len()).any(|k| off_segment(p, r[k], r[(k + 1) % r.len()]) <= 4.0 * SAME))
    })
}

/// The separate parts of an area that are worth keeping.
fn parts_kept(rings: &[Polyline]) -> Vec<Vec<Polyline>> {
    boolean::parts(rings, FillRule::EvenOdd)
        .into_iter()
        .filter(|p| area_of(p) > SLIVER)
        .collect()
}

fn op(a: &[Polyline], b: &[Polyline], op: PathOp) -> Vec<Polyline> {
    boolean::pathfinder(
        &[
            (a.to_vec(), FillRule::EvenOdd),
            (b.to_vec(), FillRule::EvenOdd),
        ],
        op,
    )
}

/// The box an area is in: its least corner and its greatest.
fn extent(rings: &[Polyline]) -> (P, P) {
    rings.iter().flatten().fold(
        (P::new(f64::MAX, f64::MAX), P::new(f64::MIN, f64::MIN)),
        |(lo, hi), p| {
            (
                P::new(lo.x.min(p.x), lo.y.min(p.y)),
                P::new(hi.x.max(p.x), hi.y.max(p.y)),
            )
        },
    )
}

/// Whether two boxes overlap or touch.
fn meet(a: (P, P), b: (P, P)) -> bool {
    a.0.x <= b.1.x && b.0.x <= a.1.x && a.0.y <= b.1.y && b.0.y <= a.1.y
}

/// One of the parts that overlapping shapes divide each other into.
#[derive(Clone, Debug)]
pub struct Face {
    /// Its outline, and then its holes (design units).
    pub rings: Vec<Polyline>,
    /// The shapes that cover it, as places in the list they were given
    /// in: the last is the one in front.
    pub sources: Vec<usize>,
}

impl Face {
    pub fn contains(&self, p: P) -> bool {
        point_inside(&self.rings, FillRule::EvenOdd, p)
    }
}

/// The faces of `areas` (each a shape's rings, even-odd, back to front):
/// every connected part of the plane covered by the same set of them.
pub fn faces(areas: &[Vec<Polyline>]) -> Vec<Face> {
    // Each face so far, split by the next shape into what's inside it and
    // what isn't; and what of the shape no face had covered.
    let mut all: Vec<(Vec<usize>, Vec<Polyline>)> = Vec::new();
    for (k, shape) in areas.iter().enumerate() {
        let mut next = Vec::with_capacity(all.len() * 2 + 1);
        let mut rest = shape.clone();
        let reach = extent(shape);
        for (sources, face) in all {
            // (Most faces of a big selection are nowhere near the shape.)
            if !meet(extent(&face), reach) {
                next.push((sources, face));
                continue;
            }
            let inside = op(&face, shape, PathOp::Intersect);
            if !inside.is_empty() {
                let outside = op(&face, shape, PathOp::MinusFront);
                rest = op(&rest, &face, PathOp::MinusFront);
                if !outside.is_empty() {
                    next.push((sources.clone(), outside));
                }
                let mut with = sources;
                with.push(k);
                next.push((with, inside));
            } else {
                next.push((sources, face));
            }
        }
        if !rest.is_empty() {
            next.push((vec![k], rest));
        }
        all = next;
    }
    // (What's too slight to be a part is no face: it goes with whatever
    // is made of the faces beside it.)
    all.into_iter()
        .flat_map(|(sources, rings)| {
            boolean::parts(&rings, FillRule::EvenOdd)
                .into_iter()
                .filter(|p| !slight(p))
                .map(move |rings| Face {
                    rings,
                    sources: sources.clone(),
                })
        })
        .collect()
}

impl Document {
    /// Whether object `i` is a filled shape these tools work on: an area
    /// that isn't text (which is its letters, and stays editable as text).
    pub fn builds(&self, i: usize) -> bool {
        self.shapes
            .get(i)
            .is_some_and(|s| s.part == Part::Fill && matches!(s.geometry, Geometry::Fill { .. }))
            && self.lettering(i).is_none()
    }

    /// The corners of object `i`'s outline: its points that aren't smooth.
    fn corners_of(&self, i: usize) -> Vec<P> {
        self.edit_path(i)
            .map(|path| {
                path.iter()
                    .flat_map(|sp| sp.anchors.iter())
                    .filter(|a| !a.is_smooth())
                    .map(|a| a.p)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The area of object `i`, even-odd.
    fn clean_area(&self, i: usize) -> Vec<Polyline> {
        let (rings, rule) = self.area(i);
        boolean::normalize(&rings, rule)
    }

    /// A new filled shape (design units) in the SVG just before object
    /// `beside`'s element, inside whatever groups that's in: it's sewn
    /// where that one is, and belongs where that one does (a layer, a
    /// group that's been entered). Returns its id; `import` comes after.
    fn insert_beside(&mut self, path: &BezPath, beside: usize, name: &str) -> Result<String> {
        let m = self.mm_to_user()?;
        let d = bezier::to_svg_d(&bezier::transform(path, [0.1, 0.0, 0.0, 0.1, 0.0, 0.0]));
        let (id, at, inside) = {
            let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
            let used: HashSet<&str> = doc
                .descendants()
                .filter_map(|n| n.attribute("id"))
                .collect();
            let mut k = 2;
            let mut id = format!("{name}_#{k}");
            while used.contains(id.as_str()) {
                k += 1;
                id = format!("{name}_#{k}");
            }
            let s = self
                .shapes
                .get(beside)
                .ok_or_else(|| anyhow!("no object {beside}"))?;
            let n = doc
                .descendants()
                .find(|n| n.is_element() && n.attribute("id") == Some(s.id.as_str()))
                .ok_or_else(|| anyhow!("element {} not found", s.id))?;
            // The groups it's in, outermost first, and what they do to
            // what's inside them.
            let groups: Vec<_> = n
                .ancestors()
                .skip(1)
                .filter(|a| a.is_element() && *a != doc.root_element())
                .collect();
            if groups.iter().any(|g| g.tag_name().name() != "g") {
                // (Inside something that isn't a plain group: beside the
                // whole of that, as Pathfinder's results go.)
                return self.insert_shape(path, Some(beside), name);
            }
            let inside = groups
                .iter()
                .rev()
                .filter_map(|g| g.attribute("transform"))
                .fold(IDENTITY, |all, t| {
                    affine_mul(all, super::edit::parse_transform(t))
                });
            (id, n.range().start, inside)
        };
        let Some(out) = affine_inv(inside) else {
            return self.insert_shape(path, Some(beside), name);
        };
        self.next_group += 1;
        let el = format!(
            "<g id=\"tl-drawing-{}\" transform=\"{}\"><path id=\"{}\" d=\"{d}\" fill=\"#000\" fill-rule=\"evenodd\"/></g>\n",
            self.next_group,
            super::edit::fmt_matrix(affine_mul(out, m)),
            super::xml_escape(&id)
        );
        self.xml.insert_str(at, &el);
        Ok(id)
    }

    /// New filled shapes, each beside an object and with a look, read back
    /// in together. Returns their indices.
    fn insert_shapes(
        &mut self,
        made: Vec<(BezPath, usize, [u8; 3], Params, String)>,
        gone: &[usize],
    ) -> Result<Vec<usize>> {
        // Where each is to be sewn: just before the object it's beside,
        // or, that one being about to go, before the next that stays.
        let before = |i: usize| -> Option<usize> {
            (i..self.shapes.len())
                .find(|j| !gone.contains(j))
                .map(|j| self.shapes[j].uid)
        };
        let slots: Vec<Option<usize>> = made.iter().map(|m| before(m.1)).collect();
        let mut looks: Vec<(String, [u8; 3], Params)> = Vec::with_capacity(made.len());
        for (path, behind, color, params, name) in made {
            let id = self.insert_beside(&path, behind, &name)?;
            looks.push((id, color, params));
        }
        if !gone.is_empty() {
            self.delete_many(gone)?;
        }
        let map = self.uid_map(&HashMap::new());
        self.import(&map)?;
        let mut uids = Vec::with_capacity(looks.len());
        for (id, color, params) in looks {
            match self.shapes.iter().position(|s| s.id == id) {
                Some(i) => {
                    self.shapes[i].color = color;
                    self.shapes[i].params = params;
                    uids.push(Some(self.shapes[i].uid));
                }
                None => uids.push(None),
            }
        }
        // In a design with a sewing order of its own (an `o=` in every
        // id), what's new has none, and has been read in as sewn last:
        // each goes to its place. The last first, so that those beside
        // one object stay in the order they were made.
        let mut slot_of: HashMap<Option<usize>, usize> = HashMap::new();
        for (uid, slot) in uids.iter().zip(&slots).rev() {
            let Some(uid) = *uid else { continue };
            let Some(at) = self.shapes.iter().position(|s| s.uid == uid) else {
                continue;
            };
            // Before the one made after it for the same place, if any.
            let next = slot_of.get(slot).copied().or(*slot);
            let to = next
                .and_then(|n| self.shapes.iter().position(|s| s.uid == n))
                .unwrap_or(self.shapes.len());
            if at + 1 != to {
                let s = self.shapes.remove(at);
                self.shapes.insert(if at < to { to - 1 } else { to }, s);
            }
            slot_of.insert(*slot, uid);
        }
        self.sync()?;
        let uids: Vec<usize> = uids.into_iter().flatten().collect();
        Ok(uids
            .iter()
            .filter_map(|u| self.shapes.iter().position(|s| s.uid == *u))
            .collect())
    }

    /// The settings of a shape made from object `i`: `fill_params`, less
    /// what was said of that object alone and means nothing on another
    /// outline (where along it sewing starts and ends) or would be said
    /// once too often (a trim or a stop after it).
    fn made_params(&self, i: usize) -> Params {
        Params {
            start: None,
            end: None,
            trim: None,
            trim_before: None,
            stop: None,
            ..self.fill_params(i)
        }
    }

    /// The Knife: cut the filled shapes among `indices` along `cut`, a
    /// line drawn across them (design units). A shape the line parts is
    /// replaced by its pieces, each sewn as it was; one the line only
    /// enters, or misses, is left as it is. (Of a shape in several parts,
    /// those the line doesn't part stay together, as one of the pieces.)
    /// Returns the pieces' indices.
    pub fn knife(&mut self, indices: &[usize], cut: &[P]) -> Result<Vec<usize>> {
        if cut.len() < 2 {
            bail!("draw the cut across a shape");
        }
        let uids: Vec<usize> = indices
            .iter()
            .filter(|&&i| self.builds(i))
            .map(|&i| self.shapes[i].uid)
            .collect();
        self.sync()?;
        let line = [cut.to_vec()];
        let mut made = Vec::new();
        let mut gone = Vec::new();
        let on_cut = |p: P| cut.windows(2).any(|w| off_segment(p, w[0], w[1]) <= SAME);
        for i in self.uid_indices(&uids) {
            let (rings, rule) = self.area(i);
            let whole = boolean::parts(&rings, rule);
            let pieces: Vec<Vec<Polyline>> = boolean::slice(&rings, rule, &line);
            // (No more pieces than it had parts: the line parted none.)
            if pieces.len() <= whole.len() {
                continue;
            }
            // The parts the line left whole (each is there among the
            // pieces, the size it was) stay one piece together.
            let size = |p: &[Polyline]| (area_of(p), p.len());
            let mut uncut: Vec<(f64, usize)> = whole.iter().map(|p| size(p)).collect();
            let (mut together, mut apart): (Vec<Polyline>, Vec<Vec<Polyline>>) =
                (Vec::new(), Vec::new());
            for piece in pieces {
                let (a, n) = size(&piece);
                match uncut
                    .iter()
                    .position(|&(b, m)| m == n && (a - b).abs() <= 1e-6 * b.max(1.0))
                {
                    Some(k) => {
                        uncut.swap_remove(k);
                        together.extend(piece);
                    }
                    None if a > SLIVER => apart.push(piece),
                    None => {}
                }
            }
            if apart.len() < 2 {
                continue;
            }
            if !together.is_empty() {
                apart.push(together);
            }
            let (color, params, name) =
                (self.shapes[i].color, self.made_params(i), self.name_of(i));
            let mut corners = self.corners_of(i);
            let before = made.len();
            for piece in apart {
                // Where the cut meets the outline is a corner: each point
                // on the cut beside one that isn't.
                for r in &piece {
                    let n = r.len();
                    corners.extend((0..n).filter_map(|k| {
                        let beside = [r[(k + n - 1) % n], r[(k + 1) % n]];
                        (on_cut(r[k]) && beside.iter().any(|&q| !on_cut(q))).then_some(r[k])
                    }));
                }
                let path = fit(&piece, &corners);
                if !path.is_empty() {
                    made.push((path, i, color, params.clone(), name.clone()));
                }
            }
            if made.len() - before >= 2 {
                // (A trim or a stop after the shape comes after the last
                // of its pieces.)
                let own = &self.shapes[i].params;
                made[before].3.trim_before = own.trim_before;
                if let Some(last) = made.last_mut() {
                    (last.3.trim, last.3.stop) = (own.trim, own.stop);
                }
                gone.push(i);
            } else {
                made.truncate(before);
            }
        }
        if made.is_empty() {
            bail!("the cut has to go right across a filled shape");
        }
        // (With their outlines: those ran round the shapes as they were.)
        let gone = self.with_outlines(&gone);
        self.insert_shapes(made, &gone)
    }

    /// The fills a Blob Brush stroke of `color` covering `region` joins,
    /// of `among`: those in that thread, with no outline of their own,
    /// that the paint overlaps; of them the one sewn last, and the others
    /// that follow one another back from it in the sewing order with
    /// nothing between them but more fills in that thread (joining them
    /// then changes nothing about what's sewn over what).
    fn blob_joins(&self, region: &[Polyline], color: [u8; 3], among: &[usize]) -> Vec<usize> {
        let alike = |i: usize| {
            self.builds(i)
                && self.shapes[i].color == color
                && matches!(self.shapes[i].params.kind, None | Some(Kind::Fill))
                && self.with_outlines(&[i]).len() == 1
        };
        let touched: Vec<usize> = among
            .iter()
            .copied()
            .filter(|&i| alike(i))
            .filter(|&i| {
                let both = op(&self.clean_area(i), region, PathOp::Intersect);
                !parts_kept(&both).is_empty()
            })
            .collect();
        let Some(&top) = touched.iter().max() else {
            return Vec::new();
        };
        let mut out = vec![top];
        for i in (0..top).rev() {
            if touched.contains(&i) {
                out.push(i);
            } else if !(alike(i) && self.shapes[i].groups == self.shapes[top].groups) {
                break;
            }
        }
        out.retain(|&i| self.shapes[i].groups == self.shapes[top].groups);
        out
    }

    /// The Blob Brush: a filled shape covering `region` (rings, even-odd,
    /// design units), in thread `color`. Where the paint overlaps fills in
    /// that thread among `among` (see `blob_joins`), it joins them: the
    /// one sewn last grows to take in the paint and the others, which go.
    /// Returns the painted shape's index, and whether it joined others.
    pub fn blob(
        &mut self,
        region: &[Polyline],
        color: [u8; 3],
        among: &[usize],
    ) -> Result<(usize, bool)> {
        let region = boolean::normalize(region, FillRule::EvenOdd);
        if parts_kept(&region).is_empty() {
            bail!("nothing painted");
        }
        let among: Vec<usize> = among
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        self.sync()?;
        let among = self.uid_indices(&among);
        let joins = self.blob_joins(&region, color, &among);
        let Some(&keep) = joins.first() else {
            let path = bezier::from_rings(&region, RESULT_TOL);
            return Ok((self.add_drawing_path(&path, color, Kind::Fill)?, false));
        };
        let mut all = vec![(region, FillRule::EvenOdd)];
        all.extend(
            joins
                .iter()
                .map(|&i| (self.clean_area(i), FillRule::EvenOdd)),
        );
        let corners: Vec<P> = joins.iter().flat_map(|&i| self.corners_of(i)).collect();
        let path = fit(&boolean::union(&all), &corners);
        if path.is_empty() {
            bail!("nothing painted");
        }
        let uid = self.shapes[keep].uid;
        let others: Vec<usize> = joins.iter().copied().filter(|&i| i != keep).collect();
        self.set_path(keep, &path)?;
        if !others.is_empty() {
            let others: Vec<usize> = {
                let uids: Vec<usize> = others.iter().map(|&i| self.shapes[i].uid).collect();
                self.uid_indices(&uids)
            };
            self.delete_many(&others)?;
        }
        let at = self
            .shapes
            .iter()
            .position(|s| s.uid == uid)
            .ok_or_else(|| anyhow::anyhow!("the painted shape has no area"))?;
        Ok((at, true))
    }

    /// The filled shapes among `indices` that the Shape Builder works on,
    /// back to front, and the faces they divide each other into (each
    /// face's sources are places in that list).
    pub fn builder_faces(&self, indices: &[usize]) -> (Vec<usize>, Vec<Face>) {
        let mut inputs: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|&i| self.builds(i))
            .collect();
        inputs.sort_unstable();
        inputs.dedup();
        let areas: Vec<Vec<Polyline>> = inputs.iter().map(|&i| self.clean_area(i)).collect();
        (inputs, faces(&areas))
    }

    /// The Shape Builder: the faces of the filled shapes among `indices`
    /// that the points of `along` are in (a click's one point, or a drag's
    /// path) become one shape, with the look of the shape in front where
    /// the drag began; with `erase`, they're taken away instead. Each
    /// shape that covered any of them keeps what's left of itself, as the
    /// object it was (or goes, if nothing is). Returns the indices of
    /// every shape given that's still there, and the new one's.
    pub fn shape_builder(
        &mut self,
        indices: &[usize],
        along: &[P],
        erase: bool,
    ) -> Result<Vec<usize>> {
        let uids: Vec<usize> = indices
            .iter()
            .filter(|&&i| self.builds(i))
            .map(|&i| self.shapes[i].uid)
            .collect();
        if uids.is_empty() {
            bail!("select the overlapping shapes first");
        }
        self.sync()?;
        let (inputs, faces) = self.builder_faces(&self.uid_indices(&uids));
        // The faces touched, in the order the drag reached them.
        let mut touched: Vec<usize> = Vec::new();
        for &p in along {
            if let Some(f) = faces.iter().position(|f| f.contains(p))
                && !touched.contains(&f)
            {
                touched.push(f);
            }
        }
        let Some(&first) = touched.first() else {
            bail!("no part of the selected shapes there");
        };
        let merged = boolean::union(
            &touched
                .iter()
                .map(|&f| (faces[f].rings.clone(), FillRule::EvenOdd))
                .collect::<Vec<_>>(),
        );
        // Each shape that covered a touched face: what's left of it.
        let mut covering: Vec<usize> = touched
            .iter()
            .flat_map(|&f| faces[f].sources.iter().copied())
            .collect();
        covering.sort_unstable();
        covering.dedup();
        // The look where the drag began: the shape in front there.
        let look = inputs[*faces[first].sources.last().expect("a face has a source")];
        // (Where two outlines cross is a sharp turn, and found as one.)
        let corners: Vec<P> = inputs.iter().flat_map(|&i| self.corners_of(i)).collect();
        let new = (!erase).then(|| {
            (
                fit(&merged, &corners),
                self.shapes[look].color,
                self.made_params(look),
                self.name_of(look),
                self.shapes[look].uid,
            )
        });
        if let Some((path, ..)) = &new
            && path.is_empty()
        {
            bail!("nothing would be left");
        }
        let mut edits: Vec<(usize, BezPath)> = Vec::new();
        let mut gone: Vec<usize> = Vec::new();
        for &k in &covering {
            let i = inputs[k];
            // (Less the slight bits left against the edge of what was
            // taken: a shape of specks would be sewn as specks.)
            let left = op(&self.clean_area(i), &merged, PathOp::MinusFront);
            let left: Vec<Polyline> = parts_kept(&left)
                .into_iter()
                .filter(|p| !(slight(p) && touches(p, &merged)))
                .flatten()
                .collect();
            let path = fit(&left, &corners);
            if path.is_empty() {
                gone.push(self.shapes[i].uid);
            } else {
                edits.push((i, path));
            }
        }
        // (A click on the only face of a shape standing clear of the
        // others would put the same shape back: nothing to do.)
        if !erase && touched.len() == 1 && faces[first].sources.len() == 1 && edits.is_empty() {
            bail!("that's the whole of a shape already");
        }
        if !edits.is_empty() {
            self.set_paths(&edits)?;
        }
        let mut made = Vec::new();
        if let Some((path, color, params, name, look)) = new {
            // Beside the shape it takes after (which may be about to go:
            // it's an element until then).
            let behind = self.shapes.iter().position(|s| s.uid == look).unwrap_or(0);
            made.push((path, behind, color, params, name));
        }
        // (With their outlines: those ran round the shapes as they were.)
        let gone = self.with_outlines(&self.uid_indices(&gone));
        let new_at = self.insert_shapes(made, &gone)?;
        let mut out = self.uid_indices(&uids);
        out.extend(new_at);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Squares side by side or overlapping, each `(id, x, y, side mm, fill)`.
    fn doc(squares: &[(&str, f64, f64, f64, &str)]) -> Document {
        let body: String = squares
            .iter()
            .map(|(id, x, y, w, fill)| {
                format!(
                    r#"<rect id="{id}" x="{x}" y="{y}" width="{w}" height="{w}" fill="{fill}"/>"#
                )
            })
            .collect();
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="60mm" viewBox="0 0 60 60">{body}</svg>"#
        );
        Document::from_svg(&svg, "t", None).unwrap()
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

    /// A point of the document (mm from its corner) in design units.
    fn pt(d: &Document, x: f64, y: f64) -> P {
        P::new(x * 10.0, y * 10.0) - d.offset
    }

    #[test]
    fn faces_are_the_parts_shapes_divide_each_other_into() {
        let sq = |x: f64, y: f64, w: f64| -> Vec<Polyline> {
            vec![vec![
                P::new(x, y),
                P::new(x + w, y),
                P::new(x + w, y + w),
                P::new(x, y + w),
            ]]
        };
        // Two overlapping: each one's own part, and the overlap (which
        // both cover, the second in front).
        let f = faces(&[sq(0.0, 0.0, 100.0), sq(50.0, 0.0, 100.0)]);
        assert_eq!(f.len(), 3);
        let of = |s: &[usize]| f.iter().find(|x| x.sources == s).map(|x| area_of(&x.rings));
        assert!((of(&[0]).unwrap() - 5000.0).abs() < 1.0);
        assert!((of(&[1]).unwrap() - 5000.0).abs() < 1.0);
        assert!((of(&[0, 1]).unwrap() - 5000.0).abs() < 1.0);
        // A bar across a square parts what's left of it in two: two faces
        // of the square alone.
        let bar = vec![vec![
            P::new(-10.0, 40.0),
            P::new(110.0, 40.0),
            P::new(110.0, 60.0),
            P::new(-10.0, 60.0),
        ]];
        let f = faces(&[sq(0.0, 0.0, 100.0), bar]);
        assert_eq!(f.iter().filter(|x| x.sources == [0]).count(), 2);
        assert_eq!(f.iter().filter(|x| x.sources == [0, 1]).count(), 1);
        assert_eq!(f.iter().filter(|x| x.sources == [1]).count(), 2);
        // Apart: one face each; one inside another: the ring and the middle.
        assert_eq!(faces(&[sq(0.0, 0.0, 10.0), sq(50.0, 0.0, 10.0)]).len(), 2);
        let f = faces(&[sq(0.0, 0.0, 100.0), sq(40.0, 40.0, 20.0)]);
        assert_eq!(f.len(), 2);
        assert!(f.iter().any(|x| x.sources == [0] && x.rings.len() == 2));
    }

    #[test]
    fn the_knife_parts_a_shape_and_each_piece_sews_as_it_did() {
        let mut d = doc(&[
            ("a_fill_a=30", 10.0, 10.0, 20.0, "#c00"),
            ("b", 40.0, 40.0, 10.0, "#00c"),
        ]);
        let (a, b) = (at(&d, "a"), at(&d, "b"));
        assert_eq!(d.shapes[a].params.angle, Some(30.0));
        let color = d.shapes[a].color;
        // Across `a`, a third of the way down, and well clear of `b`.
        let cut = [pt(&d, 5.0, 16.0), pt(&d, 35.0, 16.0)];
        let pieces = d.knife(&[a, b], &cut).unwrap();
        assert_eq!(pieces.len(), 2);
        assert_eq!(d.shapes.len(), 3, "a's two pieces, and b");
        let mut areas: Vec<f64> = pieces.iter().map(|&i| mm2(&d, i)).collect();
        areas.sort_by(f64::total_cmp);
        assert!(
            (areas[0] - 120.0).abs() < 1.0 && (areas[1] - 280.0).abs() < 1.0,
            "{areas:?}"
        );
        for &i in &pieces {
            assert_eq!(d.shapes[i].color, color);
            assert_eq!(d.shapes[i].params.angle, Some(30.0));
            assert_eq!(d.shapes[i].params.kind, Some(Kind::Fill));
        }
        assert!((mm2(&d, at(&d, "b")) - 100.0).abs() < 0.5, "b as it was");
        // It all comes back from the file.
        let again = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        assert_eq!(again.shapes.len(), 3);
        assert_eq!(
            again
                .shapes
                .iter()
                .filter(|s| s.params.angle == Some(30.0))
                .count(),
            2
        );
        // A cut that stops inside, or misses, or is a point: nothing, and
        // the document as it was.
        let mut d = doc(&[("a", 10.0, 10.0, 20.0, "#c00")]);
        let before = d.to_svg().unwrap();
        assert!(
            d.knife(&[0], &[pt(&d, 5.0, 16.0), pt(&d, 20.0, 16.0)])
                .is_err()
        );
        assert!(
            d.knife(&[0], &[pt(&d, 5.0, 50.0), pt(&d, 35.0, 50.0)])
                .is_err()
        );
        assert!(d.knife(&[0], &[pt(&d, 5.0, 16.0)]).is_err());
        assert_eq!(d.shapes.len(), 1);
        assert_eq!(d.to_svg().unwrap(), before);
        // Two cuts in one line (down, across, up): three pieces.
        let zig = [
            pt(&d, 16.0, 5.0),
            pt(&d, 16.0, 35.0),
            pt(&d, 24.0, 35.0),
            pt(&d, 24.0, 5.0),
        ];
        assert_eq!(d.knife(&[0], &zig).unwrap().len(), 3);
    }

    #[test]
    fn the_blob_brush_paints_a_fill_and_joins_the_fills_in_its_thread() {
        let mut d = doc(&[
            ("red", 10.0, 10.0, 10.0, "#c00"),
            ("blue", 30.0, 10.0, 10.0, "#00c"),
        ]);
        let red = [0xcc, 0, 0];
        let all = |d: &Document| -> Vec<usize> { (0..d.shapes.len()).collect() };
        let stroke = |d: &Document, x0: f64, x1: f64, y: f64| {
            boolean::brush_area(&[pt(d, x0, y), pt(d, x1, y)], 40.0)
        };
        // Clear of everything: a new fill, in the thread asked for.
        let (new, joined) = d
            .blob(&stroke(&d, 10.0, 20.0, 40.0), red, &all(&d))
            .unwrap();
        assert!(!joined);
        assert_eq!(d.shapes.len(), 3);
        assert_eq!(d.shapes[new].color, red);
        assert_eq!(d.shapes[new].params.kind, Some(Kind::Fill));
        // 10 × 4 mm and its round ends.
        let want = 40.0 + std::f64::consts::PI * 4.0;
        assert!((mm2(&d, new) - want).abs() < 2.0, "{}", mm2(&d, new));
        // From the red square across to the blue one, in red: the red
        // square grows (it's the same object), the blue one is untouched.
        let uid = d.shapes[at(&d, "red")].uid;
        let (grown, joined) = d
            .blob(&stroke(&d, 15.0, 35.0, 15.0), red, &all(&d))
            .unwrap();
        assert!(joined);
        assert_eq!(d.shapes.len(), 3, "nothing new");
        assert_eq!(d.shapes[grown].uid, uid);
        assert!(mm2(&d, grown) > 100.0 + 40.0, "{}", mm2(&d, grown));
        assert!((mm2(&d, at(&d, "blue")) - 100.0).abs() < 0.5);
        // Two reds sewn one after the other, and a stroke over both: one
        // shape, the one sewn later, and an object fewer.
        let mut d = doc(&[
            ("red", 10.0, 10.0, 10.0, "#c00"),
            ("red2", 30.0, 10.0, 10.0, "#c00"),
        ]);
        let later = d.shapes[at(&d, "red2")].uid;
        let (one, joined) = d
            .blob(&stroke(&d, 15.0, 35.0, 15.0), red, &all(&d))
            .unwrap();
        assert!(joined);
        assert_eq!(d.shapes.len(), 1);
        assert_eq!(d.shapes[one].uid, later);
        assert!(mm2(&d, one) > 200.0 + 30.0, "{}", mm2(&d, one));
        // A fill in another thread sewn between two reds keeps them apart:
        // joining them would sew one of them on the wrong side of it.
        let mut d = doc(&[
            ("red", 10.0, 10.0, 10.0, "#c00"),
            ("blue", 18.0, 10.0, 10.0, "#00c"),
            ("red2", 26.0, 10.0, 10.0, "#c00"),
        ]);
        let across = boolean::brush_area(&[pt(&d, 12.0, 15.0), pt(&d, 34.0, 15.0)], 40.0);
        let (top, _) = d.blob(&across, red, &all(&d)).unwrap();
        assert_eq!(d.shapes.len(), 3);
        assert!(d.shapes[top].id.starts_with("red2"), "{}", d.shapes[top].id);
        assert!(
            (mm2(&d, at(&d, "red")) - 100.0).abs() < 0.5,
            "the first red as it was"
        );
        // Only among what it's given (the rest is locked, say).
        let mut d = doc(&[("red", 10.0, 10.0, 10.0, "#c00")]);
        let over = boolean::brush_area(&[pt(&d, 12.0, 15.0), pt(&d, 30.0, 15.0)], 40.0);
        let (_, joined) = d.blob(&over, red, &[]).unwrap();
        assert!(!joined);
        assert_eq!(d.shapes.len(), 2);
    }

    #[test]
    fn the_shape_builder_joins_parts_and_leaves_each_shape_the_rest() {
        // Two 20 mm squares, the second 10 mm along and in front.
        let two = || {
            doc(&[
                ("a_fill_a=30", 10.0, 10.0, 20.0, "#c00"),
                ("b", 20.0, 10.0, 20.0, "#00c"),
            ])
        };
        // A drag from a's own part into the overlap: one shape of the two
        // parts, looking like a (where the drag began); b keeps the rest
        // of itself, and is the object it was.
        let mut d = two();
        let (a, b) = (at(&d, "a"), at(&d, "b"));
        let (red, b_uid) = (d.shapes[a].color, d.shapes[b].uid);
        let drag = [pt(&d, 15.0, 20.0), pt(&d, 25.0, 20.0)];
        let out = d.shape_builder(&[a, b], &drag, false).unwrap();
        assert_eq!(d.shapes.len(), 2);
        assert_eq!(out.len(), 2);
        let merged = d.shapes.iter().position(|s| s.uid != b_uid).unwrap();
        assert!((mm2(&d, merged) - 400.0).abs() < 1.0, "{}", mm2(&d, merged));
        assert_eq!(d.shapes[merged].color, red);
        assert_eq!(d.shapes[merged].params.angle, Some(30.0));
        let rest = d.shapes.iter().position(|s| s.uid == b_uid).unwrap();
        assert!((mm2(&d, rest) - 200.0).abs() < 1.0, "{}", mm2(&d, rest));
        assert!(d.shapes[rest].id.starts_with('b'));
        // A click on the overlap: it's a shape of its own, looking like b
        // (in front there), and both squares keep the rest of themselves.
        let mut d = two();
        let out = d
            .shape_builder(&[0, 1], &[pt(&d, 25.0, 20.0)], false)
            .unwrap();
        assert_eq!(d.shapes.len(), 3);
        assert_eq!(out.len(), 3);
        let mut areas: Vec<f64> = (0..3).map(|i| mm2(&d, i)).collect();
        areas.sort_by(f64::total_cmp);
        for a in &areas {
            assert!((a - 200.0).abs() < 1.0, "{areas:?}");
        }
        let blue = [0, 0, 0xcc];
        assert_eq!(d.shapes.iter().filter(|s| s.color == blue).count(), 2);
        // Erasing the overlap: two shapes, each less the overlap.
        let mut d = two();
        d.shape_builder(&[0, 1], &[pt(&d, 25.0, 20.0)], true)
            .unwrap();
        assert_eq!(d.shapes.len(), 2);
        for i in 0..2 {
            assert!((mm2(&d, i) - 200.0).abs() < 1.0);
        }
        // Erasing all of a shape: it goes.
        let mut d = two();
        let all = [pt(&d, 15.0, 20.0), pt(&d, 25.0, 20.0)];
        d.shape_builder(&[0, 1], &all, true).unwrap();
        assert_eq!(d.shapes.len(), 1);
        assert!(d.shapes[0].id.starts_with('b'));
        assert!((mm2(&d, 0) - 200.0).abs() < 1.0);
        // Across all three parts: one shape, one object.
        let mut d = two();
        let all = [pt(&d, 15.0, 20.0), pt(&d, 25.0, 20.0), pt(&d, 35.0, 20.0)];
        let out = d.shape_builder(&[0, 1], &all, false).unwrap();
        assert_eq!(d.shapes.len(), 1);
        assert_eq!(out.len(), 1);
        assert!((mm2(&d, 0) - 600.0).abs() < 1.0);
        assert_eq!(d.shapes[0].color, red);
        // A shape that pokes a hair out of the one it lies on (0.05 mm
        // here): that hair is no part to be picked out, and a drag over
        // the parts there are leaves nothing of it behind.
        let mut d = doc(&[("a", 10.0, 10.0, 20.0, "#c00")]);
        d.add_drawing(
            &[
                pt(&d, 15.0, 15.0),
                pt(&d, 30.05, 15.0),
                pt(&d, 30.05, 25.0),
                pt(&d, 15.0, 25.0),
            ],
            true,
            [0, 0, 0xcc],
            Kind::Fill,
        )
        .unwrap();
        let (_, f) = d.builder_faces(&[0, 1]);
        assert_eq!(f.len(), 2, "a's own part and the overlap: {f:?}");
        let over = [pt(&d, 12.0, 12.0), pt(&d, 20.0, 20.0)];
        let out = d.shape_builder(&[0, 1], &over, false).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(d.shapes.len(), 1);
        // Off the shapes, or with none given: an error, and no change.
        let mut d = two();
        let before = d.to_svg().unwrap();
        assert!(
            d.shape_builder(&[0, 1], &[pt(&d, 50.0, 50.0)], false)
                .is_err()
        );
        assert!(d.shape_builder(&[], &[pt(&d, 25.0, 20.0)], false).is_err());
        assert_eq!(d.to_svg().unwrap(), before);
        // A shape not among those given is no part of it.
        let mut d = two();
        assert!(d.shape_builder(&[0], &[pt(&d, 25.0, 20.0)], false).is_err());
        assert_eq!(d.shapes.len(), 2, "a alone has one part: nothing to do");
        assert!((mm2(&d, 0) - 400.0).abs() < 1.0 && (mm2(&d, 1) - 400.0).abs() < 1.0);
    }

    #[test]
    fn what_is_made_of_a_shape_in_a_group_is_in_that_group_where_it_was() {
        // A group that moves and scales what's in it, between two loose
        // shapes: `a` and `b` (20 mm squares as drawn, 10 mm as placed,
        // overlapping by half) are in it.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="60mm" viewBox="0 0 60 60"><rect id="first" x="2" y="2" width="5" height="5" fill="#0c0"/><g id="layer"><g id="grp" transform="translate(10 10) scale(0.5)"><rect id="a" x="0" y="0" width="20" height="20" fill="#c00"/><rect id="b" x="10" y="0" width="20" height="20" fill="#00c"/></g></g><rect id="last" x="50" y="50" width="5" height="5" fill="#0c0"/></svg>"##;
        let open = || Document::from_svg(svg, "t", None).unwrap();
        let groups = vec!["layer".to_string(), "grp".to_string()];
        let order = |d: &Document| -> Vec<String> {
            d.shapes
                .iter()
                .map(|s| s.id.split('_').next().unwrap_or_default().to_string())
                .collect()
        };
        // The Knife: a's pieces are in the group, where a was, the size
        // it was.
        let mut d = open();
        assert_eq!(order(&d), ["first", "a", "b", "last"]);
        assert_eq!(d.shapes[1].groups, groups);
        let pieces = d
            .knife(&[1], &[pt(&d, 5.0, 13.0), pt(&d, 25.0, 13.0)])
            .unwrap();
        assert_eq!(pieces, [1, 2]);
        assert_eq!(order(&d), ["first", "a", "a", "b", "last"]);
        let mut areas: Vec<f64> = pieces.iter().map(|&i| mm2(&d, i)).collect();
        areas.sort_by(f64::total_cmp);
        assert!(
            (areas[0] - 30.0).abs() < 0.5 && (areas[1] - 70.0).abs() < 0.5,
            "{areas:?}"
        );
        for &i in &pieces {
            assert_eq!(d.shapes[i].groups, groups);
        }
        // And from the file again.
        let again = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        assert_eq!(order(&again), ["first", "a", "a", "b", "last"]);
        assert_eq!(again.shapes[1].groups, groups);
        assert!((mm2(&again, 1) + mm2(&again, 2) - 100.0).abs() < 0.5);
        // The Shape Builder: the overlap, as a shape of its own, is in
        // the group beside b (whose look it has).
        let mut d = open();
        d.shape_builder(&[1, 2], &[pt(&d, 17.5, 15.0)], false)
            .unwrap();
        assert_eq!(order(&d), ["first", "a", "b", "b", "last"]);
        for i in 1..=3 {
            assert_eq!(d.shapes[i].groups, groups);
            assert!((mm2(&d, i) - 50.0).abs() < 0.5, "{}", mm2(&d, i));
        }
    }

    #[test]
    fn corners_stay_corners_and_curves_stay_curves_through_a_cut() {
        // A hexagon (which, as six points with nothing to say they're
        // corners, fits a circle nicely) cut across: its pieces are
        // straight-sided, and add up to it.
        let hex: Vec<P> = (0..6)
            .map(|k| {
                let a = (k as f64) * std::f64::consts::FRAC_PI_3 + 0.2;
                P::new(100.0 * a.cos(), 100.0 * a.sin())
            })
            .collect();
        let mut d = Document::empty();
        let i = d.add_drawing(&hex, true, [200, 0, 0], Kind::Fill).unwrap();
        let whole = mm2(&d, i);
        let pieces = d
            .knife(&[i], &[P::new(-150.0, 13.0), P::new(150.0, 13.0)])
            .unwrap();
        assert_eq!(pieces.len(), 2);
        let sum: f64 = pieces.iter().map(|&k| mm2(&d, k)).sum();
        assert!((sum - whole).abs() < whole * 0.001, "{sum} of {whole}");
        for &k in &pieces {
            let path = d.edit_path(k).unwrap();
            assert!(
                path.iter()
                    .flat_map(|sp| &sp.anchors)
                    .all(|a| !a.has_handles()),
                "straight sides: {path:?}"
            );
        }
        // A circle cut across: each piece is an arc (a few smooth points)
        // closed by one straight side, between two corners on the cut.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="60mm" viewBox="0 0 60 60"><circle id="a" cx="30" cy="30" r="20" fill="#c00"/></svg>"##;
        let mut d = Document::from_svg(svg, "t", None).unwrap();
        let whole = mm2(&d, 0);
        let y = pt(&d, 0.0, 22.0).y;
        let pieces = d
            .knife(&[0], &[P::new(-400.0, y), P::new(400.0, y)])
            .unwrap();
        assert_eq!(pieces.len(), 2);
        let sum: f64 = pieces.iter().map(|&k| mm2(&d, k)).sum();
        assert!((sum - whole).abs() < whole * 0.005, "{sum} of {whole}");
        for &k in &pieces {
            let path = d.edit_path(k).unwrap();
            assert_eq!(path.len(), 1);
            let a = &path[0].anchors;
            let on: Vec<usize> = (0..a.len())
                .filter(|&j| (a[j].p.y - y).abs() < 0.1)
                .collect();
            assert_eq!(on.len(), 2, "two points on the cut: {a:?}");
            assert!(a.len() <= 8, "and few on the arc: {}", a.len());
            // The side between them is straight: no handle along it.
            let (p, q) = (on[0], on[1]);
            let (from, to) = if (p + 1) % a.len() == q {
                (p, q)
            } else {
                (q, p)
            };
            let straight = (a[from].h_out == a[from].p && a[to].h_in == a[to].p)
                || (a[to].h_out == a[to].p && a[from].h_in == a[from].p);
            assert!(straight, "{a:?}");
        }
    }

    #[test]
    fn round_shapes_join_without_slivers() {
        // Two circles of 10 mm radius, their centres 10 mm apart.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="60mm" viewBox="0 0 60 60"><circle id="a" cx="25" cy="30" r="10" fill="#c00"/><circle id="b" cx="35" cy="30" r="10" fill="#00c"/></svg>"##;
        let mut d = Document::from_svg(svg, "t", None).unwrap();
        let (inputs, f) = d.builder_faces(&[0, 1]);
        assert_eq!(inputs, [0, 1]);
        assert_eq!(f.len(), 3, "each one's crescent, and the lens");
        let drag = [pt(&d, 18.0, 30.0), pt(&d, 30.0, 30.0), pt(&d, 42.0, 30.0)];
        d.shape_builder(&[0, 1], &drag, false).unwrap();
        assert_eq!(d.shapes.len(), 1);
        let (rings, rule) = d.area(0);
        let parts = boolean::parts(&rings, rule);
        assert_eq!(parts.len(), 1, "one piece");
        assert_eq!(parts[0].len(), 1, "with no hole or seam in it");
        // Two discs less their lens: 2πr² − (2r² acos(d/2r) − d/2 √(4r² − d²)).
        let lens = 2.0 * 100.0 * (0.5f64).acos() - 5.0 * (300.0f64).sqrt();
        let want = 2.0 * std::f64::consts::PI * 100.0 - lens;
        assert!(
            (mm2(&d, 0) - want).abs() < want * 0.01,
            "{} for {want}",
            mm2(&d, 0)
        );
    }

    /// A point inside a face: the middle of the widest run of it along a
    /// line across its box (its centre needn't be in it).
    fn within(f: &Face) -> Option<P> {
        let (mut lo, mut hi) = (P::new(f64::MAX, f64::MAX), P::new(f64::MIN, f64::MIN));
        for &p in f.rings.iter().flatten() {
            lo = P::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = P::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        (1..32).find_map(|k| {
            // (The middle first, then out from it.)
            let k = if k % 2 == 0 { 16 + k / 2 } else { 16 - k / 2 };
            let y = lo.y + (hi.y - lo.y) * k as f64 / 32.0;
            let mut best: Option<(f64, f64)> = None;
            let mut run: Option<f64> = None;
            for n in 0..=200 {
                let x = lo.x + (hi.x - lo.x) * n as f64 / 200.0;
                match (f.contains(P::new(x, y)), run) {
                    (true, None) => run = Some(x),
                    (false, Some(from)) => {
                        if best.is_none_or(|(a, b)| x - from > b - a) {
                            best = Some((from, x));
                        }
                        run = None;
                    }
                    _ => {}
                }
            }
            best.map(|(a, b)| P::new((a + b) / 2.0, y))
        })
    }

    /// The Knife, the Blob Brush and the Shape Builder at work on one
    /// design, and the design read back from its file after each: the same
    /// objects, in the same threads, sewn the same way.
    fn tools_hold_on(name: &str, svg: &str) {
        let base = Document::from_svg(svg, name, None).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let fills: Vec<usize> = (0..base.shapes.len())
            .filter(|&i| {
                base.builds(i) && !base.shapes[i].params.locked() && !base.shapes[i].params.hidden()
            })
            .collect();
        let area = |d: &Document, i: usize| -> f64 {
            let (rings, rule) = d.area(i);
            boolean::parts(&rings, rule)
                .iter()
                .map(|p| area_of(p))
                .sum()
        };
        let bounds = |d: &Document, i: usize| -> (P, P) {
            let (rings, _) = d.area(i);
            let (mut lo, mut hi) = (P::new(f64::MAX, f64::MAX), P::new(f64::MIN, f64::MIN));
            for &p in rings.iter().flatten() {
                lo = P::new(lo.x.min(p.x), lo.y.min(p.y));
                hi = P::new(hi.x.max(p.x), hi.y.max(p.y));
            }
            (lo, hi)
        };
        let comes_back = |d: &mut Document, what: &str| {
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
                assert_eq!(a.part, b.part, "{name}: {what}: part of {}", b.id);
            }
            assert!(
                again.plan().stitch_count() > 0 || d.shapes.is_empty(),
                "{name}: {what}: sews"
            );
        };
        let (mut cuts, mut paints, mut builds) = (0, 0, 0);
        for &i in fills.iter().take(8) {
            // A cut across its middle: pieces that add up to it, or (a
            // shape the line doesn't part) nothing at all.
            let mut d = base.clone();
            let (lo, hi) = bounds(&d, i);
            let (whole, y) = (area(&d, i), (lo.y + hi.y) / 2.0 + 0.37);
            let cut = [P::new(lo.x - 40.0, y), P::new(hi.x + 40.0, y)];
            let others = d.shapes.len() - d.with_outlines(&[i]).len();
            match d.knife(&[i], &cut) {
                Ok(pieces) => {
                    assert!(pieces.len() >= 2, "{name}: knife on {i}");
                    assert_eq!(
                        d.shapes.len(),
                        others + pieces.len(),
                        "{name}: knife on {i}"
                    );
                    let sum: f64 = pieces.iter().map(|&k| area(&d, k)).sum();
                    assert!(
                        (sum - whole).abs() < whole * 0.03 + 30.0,
                        "{name}: knife on {i}: {sum} of {whole}"
                    );
                    for &k in &pieces {
                        assert_eq!(d.shapes[k].color, base.shapes[i].color);
                    }
                    cuts += 1;
                }
                Err(_) => assert_eq!(d.shapes.len(), base.shapes.len(), "{name}: knife on {i}"),
            }
            comes_back(&mut d, &format!("knife on {i}"));

            // A dab of its own thread in its middle: it grows (or, where
            // the middle is a hole, there's one fill more).
            let mut d = base.clone();
            let c = P::new((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0);
            let region = boolean::brush_area(&[c, P::new(c.x + 25.0, c.y)], 30.0);
            let all: Vec<usize> = (0..d.shapes.len()).collect();
            let n = d.shapes.len();
            let (at, joined) = d
                .blob(&region, base.shapes[i].color, &all)
                .unwrap_or_else(|e| panic!("{name}: blob on {i}: {e:#}"));
            assert!(d.shapes.len() <= n + 1, "{name}: blob on {i}");
            assert_eq!(d.shapes.len() == n + 1, !joined, "{name}: blob on {i}");
            assert_eq!(d.shapes[at].color, base.shapes[i].color);
            paints += 1;
            comes_back(&mut d, &format!("blob on {i}"));
        }
        // Pairs of fills that overlap: the overlap made a shape of its
        // own, then taken away; and the pair made one shape.
        let mut pairs = 0;
        'pairs: for (n, &i) in fills.iter().enumerate() {
            for &j in &fills[n + 1..] {
                let (_, f) = base.builder_faces(&[i, j]);
                let Some(p) = f.iter().find(|x| x.sources.len() == 2).and_then(within) else {
                    continue;
                };
                let each: Vec<P> = f.iter().filter_map(within).collect();
                for (what, along, erase) in [
                    ("click", vec![p], false),
                    ("erase", vec![p], true),
                    ("all", each, false),
                ] {
                    let mut d = base.clone();
                    let total = area(&d, i) + area(&d, j);
                    let out = d
                        .shape_builder(&[i, j], &along, erase)
                        .unwrap_or_else(|e| panic!("{name}: builder {what} on {i},{j}: {e:#}"));
                    let left: f64 = out.iter().map(|&k| area(&d, k)).sum();
                    assert!(
                        left < total + total * 0.03 + 30.0,
                        "{name}: builder {what} on {i},{j}: {left} of {total}"
                    );
                    // (Every part, if a point was found in each.)
                    if what == "all" && along.len() == f.len() {
                        assert_eq!(out.len(), 1, "{name}: builder all on {i},{j}");
                    }
                    comes_back(&mut d, &format!("builder {what} on {i},{j}"));
                    builds += 1;
                }
                pairs += 1;
                if pairs >= 6 {
                    break 'pairs;
                }
            }
        }
        eprintln!(
            "{name}: {} fills; {cuts} cut, {paints} painted, {builds} built",
            fills.len()
        );
    }

    /// The same of your own designs: every `.svg` in the folder
    /// `ZIGZAG_DESIGNS` names (`cargo test -- --ignored designs_of_your_own`).
    #[test]
    #[ignore]
    fn designs_of_your_own_are_cut_painted_and_built_and_come_back_the_same() {
        let dir = std::env::var("ZIGZAG_DESIGNS").expect("ZIGZAG_DESIGNS: a folder of .svg files");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "svg") {
                let name = path.file_stem().unwrap().to_string_lossy().into_owned();
                tools_hold_on(&name, &std::fs::read_to_string(&path).unwrap());
                seen += 1;
            }
        }
        assert!(seen > 0, "no .svg files in {dir}");
    }

    #[test]
    fn the_tools_hold_on_the_example_badge() {
        tools_hold_on("badge", include_str!("../../examples/badge.svg"));
    }

    #[test]
    fn what_is_made_is_sewn_where_its_shape_was_in_an_order_of_your_own() {
        let three = || {
            let mut d = doc(&[
                ("a", 10.0, 10.0, 20.0, "#c00"),
                ("b_trim_trimbefore=1", 20.0, 10.0, 20.0, "#00c"),
                ("c", 45.0, 45.0, 5.0, "#0c0"),
            ]);
            // Sewn c, b, a: an order that isn't the file's.
            d.shapes.reverse();
            d.sync().unwrap();
            d
        };
        let order = |d: &Document| -> Vec<char> {
            d.shapes
                .iter()
                .map(|s| s.id.chars().next().unwrap())
                .collect()
        };
        // The Knife on b: its pieces where b was, not after a.
        let mut d = three();
        assert_eq!(order(&d), ['c', 'b', 'a']);
        assert_eq!(d.shapes[1].params.trim, Some(true));
        let pieces = d
            .knife(&[1], &[pt(&d, 15.0, 15.0), pt(&d, 45.0, 15.0)])
            .unwrap();
        assert_eq!(pieces, [1, 2]);
        assert_eq!(order(&d), ['c', 'b', 'b', 'a']);
        // The trim after b comes after the last of its pieces, once.
        assert_eq!(d.shapes[1].params.trim, None);
        assert_eq!(d.shapes[2].params.trim, Some(true));
        assert_eq!(d.shapes[1].params.trim_before, Some(true));
        assert_eq!(d.shapes[2].params.trim_before, None);
        // And so it comes back from the file.
        let again = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        assert_eq!(order(&again), ['c', 'b', 'b', 'a']);
        assert_eq!(again.shapes[2].params.trim, Some(true));
        // The Shape Builder: the overlap of b and a (a in front there, as
        // it's sewn later) is a shape beside a, not at the end of
        // everything; and a's start point, which was a's, isn't its.
        let mut d = three();
        d.shapes[2].params.start = Some(0.25);
        let were: Vec<usize> = d.shapes.iter().map(|s| s.uid).collect();
        d.shape_builder(&[1, 2], &[pt(&d, 25.0, 20.0)], false)
            .unwrap();
        assert_eq!(order(&d), ['c', 'b', 'a', 'a']);
        let new = d
            .shapes
            .iter()
            .position(|s| !were.contains(&s.uid))
            .unwrap();
        assert_eq!(new, 2, "just before the shape it takes after");
        assert_eq!(d.shapes[new].params.start, None);
        let again = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        assert_eq!(order(&again), ['c', 'b', 'a', 'a']);
        // A click on a shape that stands clear of the others is nothing
        // to do, and says so.
        let mut d = three();
        let before = d.to_svg().unwrap();
        assert!(
            d.shape_builder(&[0, 1, 2], &[pt(&d, 47.0, 47.0)], false)
                .is_err()
        );
        assert_eq!(d.to_svg().unwrap(), before);
    }

    #[test]
    fn trim_before_can_override_a_group_and_roundtrip() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
          <g id="group_trimbefore">
            <path id="first_fill" fill="#123456" d="M10 10 H20 V20 H10 Z"/>
            <path id="second_fill" fill="#123456" d="M30 10 H40 V20 H30 Z"/>
          </g>
        </svg>"##;
        let mut d = Document::from_svg(svg, "t", None).unwrap();
        assert_eq!(d.shapes[0].params.trim_before, Some(true));
        assert_eq!(d.shapes[1].params.trim_before, Some(true));
        d.shapes[0].params.trim_before = d.shapes[0].trim_before_setting(false);
        d.sync().unwrap();
        let again = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        assert_eq!(again.shapes[0].params.trim_before, Some(false));
        assert_eq!(again.shapes[1].params.trim_before, Some(true));
    }

    #[test]
    fn trim_before_off_only_overrides_an_inherited_cut() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
          <rect id="ungrouped_trimbefore" x="0" y="0" width="10" height="10" fill="#123456"/>
          <g id="outer_trimbefore_stroke.trimbefore=0">
            <rect id="both_trimbefore_stroke.trimbefore" x="15" y="0" width="10" height="10" fill="#123456" stroke="#abcdef"/>
            <g id="inner_trimbefore=0_stroke.trimbefore">
              <rect id="nested_trimbefore_stroke.trimbefore" x="30" y="0" width="10" height="10" fill="#123456" stroke="#abcdef"/>
              <path id="line_trimbefore" d="M45 0 H55" fill="none" stroke="#abcdef"/>
            </g>
          </g>
        </svg>"##;
        let mut d = Document::from_svg(svg, "t", None).unwrap();
        assert_eq!(d.shapes.len(), 6);
        let expected = [None, Some(false), None, None, Some(false), Some(false)];
        for (shape, expected) in d.shapes.iter_mut().zip(expected) {
            shape.params.trim_before = shape.trim_before_setting(false);
            assert_eq!(shape.params.trim_before, expected, "{}", shape.id);
        }
        let again = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        let again = Document::from_svg(&again.to_svg().unwrap(), "t", None).unwrap();
        // A group may already supply false without an explicit object token.
        for (shape, expected) in again.shapes.iter().zip(expected) {
            assert_ne!(shape.params.trim_before, Some(true), "{}", shape.id);
            let token = if shape.outline
                || shape.inherited_stroke_trim_before().is_some()
                    && shape.part == crate::svg::Part::Stroke
            {
                "stroke.trimbefore=0"
            } else {
                "trimbefore=0"
            };
            let has_off = shape.id.split('_').any(|t| t == token);
            assert_eq!(has_off, expected == Some(false), "{}", shape.id);
        }
    }

    #[test]
    fn a_trim_or_a_stop_turned_off_stays_off() {
        // (They're words in the id, as a stitch type is: each read as a
        // setting, and so no part of the name.)
        let mut d = doc(&[("box", 10.0, 10.0, 20.0, "#c00")]);
        for word in ["trim", "trimbefore", "stop", "spiral"] {
            let set = |d: &mut Document, on: Option<bool>| {
                let p = &mut d.shapes[0].params;
                match word {
                    "trim" => p.trim = on,
                    "trimbefore" => p.trim_before = on,
                    "stop" => p.stop = on,
                    _ => p.spiral = on,
                }
                d.sync().unwrap();
            };
            let is = |d: &Document| {
                let p = &d.shapes[0].params;
                match word {
                    "trim" => p.trim,
                    "trimbefore" => p.trim_before,
                    "stop" => p.stop,
                    _ => p.spiral,
                }
            };
            set(&mut d, Some(true));
            // Saved twice over: said once in the id.
            d.shapes[0].params.angle = Some(10.0);
            d.sync().unwrap();
            assert_eq!(
                d.shapes[0].id.matches(word).count(),
                1,
                "{}",
                d.shapes[0].id
            );
            assert_eq!(crate::svg::name_words(&d.shapes[0].id), ["box"]);
            set(&mut d, None);
            assert_eq!(is(&d), None, "{word}: {}", d.shapes[0].id);
            let again = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
            assert_eq!(is(&again), None, "{word}: {}", again.shapes[0].id);
            d.shapes[0].params.angle = None;
        }
    }
}
