//! Point-by-point path editing: read an object's outline as Bézier anchors
//! (design units) and write edited anchors back into its SVG element, keeping
//! its id, paint and transform. Circles, rects and other basic shapes become
//! `<path>` elements the first time they're edited, as in Illustrator.

use super::{Document, xml_escape};
use crate::bezier::{self, Anchor, BezPath, SubPath};
use crate::boolean::{self, PathOp};
use crate::geom::{Affine, FillRule, P, Polyline, affine_inv, affine_mul};
use crate::params::{Kind, Params};
use crate::svg::{self, PX_TO_UNITS};
use crate::svg::{Geometry, Part};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashMap;

/// Geometry attributes of basic shapes, dropped when they become paths.
const SHAPE_GEOMETRY: [&str; 11] = [
    "x", "y", "width", "height", "rx", "ry", "cx", "cy", "r", "points", "d",
];
const LINE_GEOMETRY: [&str; 4] = ["x1", "y1", "x2", "y2"];

impl Document {
    /// usvg's absolute transform (element px → canvas px), then to design units.
    fn to_design(&self, t: usvg::Transform) -> Affine {
        let k = PX_TO_UNITS;
        affine_mul(
            [k, 0.0, 0.0, k, -self.offset.x, -self.offset.y],
            [
                t.sx as f64,
                t.ky as f64,
                t.kx as f64,
                t.sy as f64,
                t.tx as f64,
                t.ty as f64,
            ],
        )
    }

    /// The element behind shape `index`, as usvg sees it.
    fn usvg_path(&self, tree: &usvg::Tree, index: usize) -> Result<(BezPath, Affine)> {
        let s = self
            .shapes
            .get(index)
            .ok_or_else(|| anyhow!("no object {index}"))?;
        let node = tree
            .node_by_id(&s.id)
            .ok_or_else(|| anyhow!("{} isn't drawn", s.id))?;
        let usvg::Node::Path(p) = node else {
            bail!("lettering is edited as text, not point by point");
        };
        let m = self.to_design(p.abs_transform());
        Ok((bezier::from_usvg(p.data(), m), m))
    }

    /// Shape `index`'s outline as editable anchors (design units). Text's
    /// is its glyph outlines; editing those (`set_paths`) turns the text
    /// into plain paths first, so text is a shape like any other.
    pub fn edit_path(&self, index: usize) -> Result<BezPath> {
        if self.lettering(index).is_some() {
            return Ok(self.glyph_path(index));
        }
        let tree = self.parsed()?;
        Ok(self.usvg_path(&tree, index)?.0)
    }

    /// Text's glyph outlines as a path (as drawn, corners kept).
    fn glyph_path(&self, index: usize) -> BezPath {
        match &self.shapes[index].geometry {
            Geometry::Fill { rings, .. } => bezier::from_rings(rings, 0.0),
            Geometry::Stroke { .. } => Vec::new(),
        }
    }

    /// Any text among `indices` becomes plain paths (so a point-level edit
    /// can go ahead). Returns the indices to use afterwards (text's index
    /// may change), in the same order.
    pub fn outlines_first(&mut self, indices: &[usize]) -> Result<Vec<usize>> {
        if !indices.iter().any(|&i| self.lettering(i).is_some()) {
            return Ok(indices.to_vec());
        }
        let uids: Vec<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i).map(|s| s.uid))
            .collect();
        let mut out = Vec::with_capacity(uids.len());
        for uid in uids {
            let Some(i) = self.shapes.iter().position(|s| s.uid == uid) else {
                continue;
            };
            if self.lettering(i).is_some() {
                let j = self.expand_lettering(i)?;
                out.push(self.shapes[j].uid);
            } else {
                out.push(uid);
            }
        }
        Ok(out
            .into_iter()
            .filter_map(|uid| self.shapes.iter().position(|s| s.uid == uid))
            .collect())
    }

    /// The SVG as usvg reads it: kept from the last import when it's current.
    pub(crate) fn parsed(&self) -> Result<std::sync::Arc<usvg::Tree>> {
        match &self.tree {
            Some(t) => Ok(t.clone()),
            None => Ok(std::sync::Arc::new(svg::parse_tree(self.xml.as_bytes())?)),
        }
    }

    /// Every object's outline as editable anchors (design units), reading the
    /// SVG once: `None` for lettering and anything not drawn as a path.
    pub fn all_edit_paths(&self) -> Vec<Option<BezPath>> {
        let Ok(tree) = self.parsed() else {
            return vec![None; self.shapes.len()];
        };
        (0..self.shapes.len())
            .map(|i| {
                if self.lettering(i).is_some() {
                    return Some(self.glyph_path(i));
                }
                self.usvg_path(&tree, i).ok().map(|(p, _)| p)
            })
            .collect()
    }

    /// Replace shape `index`'s outline with `path` (design units). An empty
    /// path is refused; delete the object instead.
    pub fn set_path(&mut self, index: usize, path: &BezPath) -> Result<()> {
        self.set_paths(&[(index, path.clone())])
    }

    /// [`Document::set_path`] for several objects at once: the SVG is read
    /// once, every outline written, and the result read back in once.
    pub fn set_paths(&mut self, edits: &[(usize, BezPath)]) -> Result<()> {
        if edits.is_empty() {
            return Ok(());
        }
        // Text edited point by point becomes plain paths first.
        let idx: Vec<usize> = edits.iter().map(|(i, _)| *i).collect();
        let idx = self.outlines_first(&idx)?;
        let edits: Vec<(usize, BezPath)> = idx
            .into_iter()
            .zip(edits.iter().map(|(_, p)| p.clone()))
            .collect();
        let edits = &edits[..];
        let mut wanted = Vec::new();
        for (index, path) in edits {
            if path.is_empty() {
                bail!("a path needs at least two points");
            }
            let uid = self
                .shapes
                .get(*index)
                .ok_or_else(|| anyhow!("no object {index}"))?
                .uid;
            wanted.push((uid, path));
        }
        self.sync()?;
        let tree = self.parsed()?;
        let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
        let by_id = super::elements_by_id(&doc);
        let mut writes: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        let mut done = std::collections::HashSet::new();
        for (uid, path) in wanted {
            let index = self
                .shapes
                .iter()
                .position(|s| s.uid == uid)
                .ok_or_else(|| anyhow!("the object went away"))?;
            let id = self.shapes[index].id.clone();
            // An element's fill and outline are one path: written once.
            if !done.insert(id.clone()) {
                continue;
            }
            let (_, m) = self.usvg_path(&tree, index)?;
            let inv = affine_inv(m).ok_or_else(|| anyhow!("the object is squashed flat"))?;
            let d = bezier::to_svg_d(&bezier::transform(path, inv));
            let n = *by_id
                .get(id.as_str())
                .ok_or_else(|| anyhow!("element {id} not found"))?;
            writes.push(if n.tag_name().name() == "path" {
                match n.attributes().find(|a| a.name() == "d") {
                    Some(a) => (a.range_value(), xml_escape(&d)),
                    None => bail!("{id} has no path data"),
                }
            } else {
                // A basic shape: the same element as a path.
                let drop: &[&str] = if n.tag_name().name() == "line" {
                    &LINE_GEOMETRY
                } else {
                    &SHAPE_GEOMETRY
                };
                let mut el = String::from("<path");
                for a in n.attributes() {
                    if a.namespace().is_none() && drop.contains(&a.name()) {
                        continue;
                    }
                    el.push(' ');
                    el.push_str(&self.xml[a.range()]);
                }
                el.push_str(&format!(" d=\"{}\"/>", xml_escape(&d)));
                (n.range(), el)
            });
        }
        drop(doc);
        let uids: HashMap<_, _> = self
            .shapes
            .iter()
            .map(|s| ((s.id.clone(), s.part), s.uid))
            .collect();
        // From the end back, so the earlier ranges still point right.
        writes.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
        for (range, text) in writes {
            self.xml.replace_range(range, &text);
        }
        self.import(&uids)
    }

    /// Add a drawn path (design units): closed paths become filled shapes,
    /// open ones strokes. Returns the new shape's index.
    pub fn add_drawing_path(
        &mut self,
        path: &BezPath,
        color: [u8; 3],
        kind: Kind,
    ) -> Result<usize> {
        self.add_drawing_stroke(path, color, kind, 0.5)
    }

    /// As [`Document::add_drawing_path`], an open path stroked `width_mm`
    /// wide (a satin column's width, say).
    pub fn add_drawing_stroke(
        &mut self,
        path: &BezPath,
        color: [u8; 3],
        kind: Kind,
        width_mm: f64,
    ) -> Result<usize> {
        let width_mm = if width_mm.is_finite() {
            width_mm.clamp(0.1, 20.0)
        } else {
            0.5
        };
        let closed = path.first().is_some_and(|sp| sp.closed);
        if path.is_empty() || path.iter().all(|sp| sp.anchors.len() < 2) {
            bail!("draw at least {} points", if closed { 3 } else { 2 });
        }
        self.sync()?;
        let m = self.mm_to_user()?;
        // Design units → mm, inside a group that maps mm to the document.
        let d = bezier::to_svg_d(&bezier::transform(path, [0.1, 0.0, 0.0, 0.1, 0.0, 0.0]));
        let hex = format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2]);
        let paint = if closed {
            format!("fill=\"{hex}\"")
        } else {
            format!(
                "fill=\"none\" stroke=\"{hex}\" stroke-width=\"{width_mm:.2}\" stroke-linejoin=\"round\" stroke-linecap=\"round\""
            )
        };
        self.next_group += 1;
        let n = self.next_group;
        let id = format!("drawing_{n}_{}", kind.name());
        let group = format!(
            "<g id=\"tl-drawing-{n}\" transform=\"{}\"><path id=\"{id}\" d=\"{d}\" {paint}/></g>\n",
            super::edit::fmt_matrix(m),
        );
        let close = self
            .xml
            .rfind("</svg>")
            .ok_or_else(|| anyhow!("document has no closing </svg>"))?;
        self.xml.insert_str(close, &group);
        let map = self.uid_map(&HashMap::new());
        self.import(&map)?;
        self.shapes
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| anyhow!("the drawing has no area or length"))
    }

    /// Add a drawing through points (design units), joined by straight lines.
    pub fn add_drawing(
        &mut self,
        pts: &[P],
        closed: bool,
        color: [u8; 3],
        kind: Kind,
    ) -> Result<usize> {
        if pts.len() < if closed { 3 } else { 2 } {
            bail!("draw at least {} points", if closed { 3 } else { 2 });
        }
        let path = vec![SubPath {
            anchors: pts.iter().map(|&p| Anchor::corner(p)).collect(),
            closed,
        }];
        self.add_drawing_path(&path, color, kind)
    }
}

/// How an outline added to a filled object is sewn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutlineStitch {
    /// A satin column centered on the edge.
    #[default]
    Satin,
    /// One pass of running stitch.
    Run,
    /// Three passes (bean stitch): a bolder line.
    TripleRun,
}

impl OutlineStitch {
    pub const ALL: [OutlineStitch; 3] = [
        OutlineStitch::Satin,
        OutlineStitch::Run,
        OutlineStitch::TripleRun,
    ];
    pub fn label(self) -> &'static str {
        match self {
            OutlineStitch::Satin => "Satin Border",
            OutlineStitch::Run => "Running Stitch",
            OutlineStitch::TripleRun => "Triple Run (Bean)",
        }
    }
    /// The name agents and files use.
    pub fn id(self) -> &'static str {
        match self {
            OutlineStitch::Satin => "satin",
            OutlineStitch::Run => "run",
            OutlineStitch::TripleRun => "triple",
        }
    }
    pub fn by_id(id: &str) -> Option<OutlineStitch> {
        OutlineStitch::ALL.into_iter().find(|s| s.id() == id)
    }
}

/// What [`Document::add_outline`] makes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutlineSpec {
    pub stitch: OutlineStitch,
    /// A satin border's width, mm (running stitches have none).
    pub width_mm: f64,
    /// How far outside the fill's edge the outline runs, mm (negative:
    /// inside it). 0 puts it on the edge, half over the fill.
    pub offset_mm: f64,
    /// The outline's thread; `None` for the fill's own.
    pub color: Option<[u8; 3]>,
}

impl OutlineSpec {
    pub const MIN_WIDTH: f64 = 0.5;
    pub const MAX_WIDTH: f64 = 12.0;
    pub const MAX_OFFSET: f64 = 20.0;
}

impl Default for OutlineSpec {
    fn default() -> Self {
        OutlineSpec {
            stitch: OutlineStitch::Satin,
            width_mm: crate::params::defaults::OUTLINE_SATIN_WIDTH,
            offset_mm: 0.0,
            color: None,
        }
    }
}

/// What [`Document::add_knockdown`] makes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KnockdownSpec {
    /// How far past the objects the knockdown reaches, mm.
    pub margin_mm: f64,
    /// The lattice's row spacing, mm.
    pub spacing_mm: f64,
    /// Its thread: usually the fabric's color, so it disappears.
    pub color: [u8; 3],
}

impl KnockdownSpec {
    pub const MAX_MARGIN: f64 = 20.0;
    pub const MIN_SPACING: f64 = 0.8;
    pub const MAX_SPACING: f64 = 5.0;
}

impl Default for KnockdownSpec {
    fn default() -> Self {
        KnockdownSpec {
            margin_mm: crate::params::defaults::KNOCKDOWN_MARGIN,
            spacing_mm: crate::params::defaults::KNOCKDOWN_SPACING,
            color: super::DEFAULT_CLOTH,
        }
    }
}

/// A path's subpaths as open lines (a closed one ends where it began), so
/// merged lines stay lines rather than becoming a filled shape.
fn open_ended(path: BezPath) -> BezPath {
    path.into_iter()
        .map(|mut sp| {
            if sp.closed
                && let Some(first) = sp.anchors.first().cloned()
            {
                sp.closed = false;
                sp.anchors.push(first);
            }
            sp
        })
        .collect()
}

/// How closely the curves fitted to path operations' results follow them
/// (design units: 0.06 mm, twice the 0.03 mm curves are flattened to).
const RESULT_TOL: f64 = 0.6;

impl Document {
    /// The area an object covers: its fill, or its stroked line.
    fn area(&self, i: usize) -> (Vec<Polyline>, FillRule) {
        match &self.shapes[i].geometry {
            Geometry::Fill { rings, rule } => (rings.clone(), *rule),
            Geometry::Stroke { lines, width } => (
                boolean::stroke_area(lines, (*width).max(1.0)),
                FillRule::NonZero,
            ),
        }
    }

    fn uid_indices(&self, uids: &[usize]) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| uids.contains(&s.uid))
            .map(|(i, _)| i)
            .collect();
        v.sort_unstable();
        v
    }

    /// Add a filled shape (design units) just behind the top-level element
    /// holding object `behind` (or at the end), with the given look. Returns
    /// its id; call `import` (or a caller that does) afterwards.
    pub(super) fn insert_shape(
        &mut self,
        path: &BezPath,
        behind: Option<usize>,
        name: &str,
    ) -> Result<String> {
        let m = self.mm_to_user()?;
        let d = bezier::to_svg_d(&bezier::transform(path, [0.1, 0.0, 0.0, 0.1, 0.0, 0.0]));
        let used: std::collections::HashSet<String> = {
            let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
            doc.descendants()
                .filter_map(|n| n.attribute("id").map(str::to_string))
                .collect()
        };
        let mut k = 2;
        let mut id = format!("{name}_#{k}");
        while used.contains(&id) {
            k += 1;
            id = format!("{name}_#{k}");
        }
        self.next_group += 1;
        let el = format!(
            "<g id=\"tl-drawing-{}\" transform=\"{}\"><path id=\"{}\" d=\"{d}\" fill=\"#000\" fill-rule=\"evenodd\"/></g>\n",
            self.next_group,
            super::edit::fmt_matrix(m),
            xml_escape(&id)
        );
        let at = match behind.and_then(|i| self.shapes.get(i)) {
            Some(s) => {
                let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
                let n = doc
                    .descendants()
                    .find(|n| n.is_element() && n.attribute("id") == Some(s.id.as_str()))
                    .ok_or_else(|| anyhow!("element {} not found", s.id))?;
                let top = n
                    .ancestors()
                    .filter(|a| a.is_element())
                    .find(|a| a.parent_element().is_some_and(|p| p == doc.root_element()))
                    .unwrap_or(n);
                top.range().start
            }
            None => self
                .xml
                .rfind("</svg>")
                .ok_or_else(|| anyhow!("document has no closing </svg>"))?,
        };
        self.xml.insert_str(at, &el);
        Ok(id)
    }

    /// After `insert_shape`: re-read, then give the new shape `from`'s look.
    fn finish_new(
        &mut self,
        id: &str,
        color: [u8; 3],
        params: crate::params::Params,
    ) -> Result<usize> {
        let map = self.uid_map(&HashMap::new());
        self.import(&map)?;
        let i = self
            .shapes
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| anyhow!("the result has no area"))?;
        self.shapes[i].color = color;
        self.shapes[i].params = params;
        Ok(i)
    }

    fn name_of(&self, i: usize) -> String {
        let w = svg::name_words(&self.shapes[i].label);
        if w.is_empty() {
            "shape".into()
        } else {
            w.join("_")
        }
    }

    /// Fill settings for a shape made from object `i`.
    fn fill_params(&self, i: usize) -> crate::params::Params {
        let mut p = self.shapes[i].params.clone();
        if !matches!(
            p.kind,
            Some(Kind::Fill | Kind::Contour | Kind::Applique | Kind::Satin)
        ) {
            p.kind = Some(Kind::Fill);
        }
        if self.shapes[i].part == Part::Stroke && p.kind == Some(Kind::Satin) {
            // A satin line outlined is a narrow shape: satin across it.
            p.width = None;
        }
        p
    }

    /// Illustrator's Pathfinder: combine objects (in sewing order, back to
    /// front) into one filled shape that replaces them. Returns its index.
    pub fn pathfinder(&mut self, indices: &[usize], op: PathOp) -> Result<usize> {
        let uids: Vec<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        if uids.len() < 2 {
            bail!("select two or more overlapping objects");
        }
        let indices = self.outlines_first(indices)?;
        let uids: Vec<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        self.sync()?;
        let idx = self.uid_indices(&uids);
        let parts: Vec<(Vec<Polyline>, FillRule)> = idx.iter().map(|&i| self.area(i)).collect();
        let rings = boolean::pathfinder(&parts, op);
        let path = bezier::from_rings(&rings, RESULT_TOL);
        if path.is_empty() {
            bail!(match op {
                PathOp::Intersect => "the objects don't all overlap",
                _ => "nothing would be left",
            });
        }
        let back = idx[0];
        let look = if op == PathOp::MinusFront {
            back
        } else {
            *idx.last().unwrap()
        };
        let (color, params, name) = (
            self.shapes[look].color,
            self.fill_params(look),
            self.name_of(look),
        );
        let id = self.insert_shape(&path, Some(back), &name)?;
        self.delete_many(&idx)?;
        self.finish_new(&id, color, params)
    }

    /// Illustrator's Offset Path: a new filled shape `d` units outside each
    /// object (inside, if negative), behind it. Returns the new indices.
    pub fn offset_path(&mut self, indices: &[usize], d: f64) -> Result<Vec<usize>> {
        let uids: Vec<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        self.sync()?;
        let mut made = Vec::new();
        for uid in uids {
            let Some(i) = self.shapes.iter().position(|s| s.uid == uid) else {
                continue;
            };
            let (rings, rule) = self.area(i);
            let path = bezier::from_rings(&boolean::offset(&rings, rule, d), RESULT_TOL);
            if path.is_empty() {
                continue;
            }
            let (color, params, name) =
                (self.shapes[i].color, self.fill_params(i), self.name_of(i));
            let id = self.insert_shape(&path, Some(i), &format!("{name}_offset"))?;
            let n = self.finish_new(&id, color, params)?;
            made.push(self.shapes[n].uid);
        }
        if made.is_empty() {
            bail!("nothing left after offsetting");
        }
        Ok(self.uid_indices(&made))
    }

    /// Illustrator's Outline Stroke: each stroked line becomes the filled
    /// shape it covers. Returns the new indices.
    pub fn outline_stroke(&mut self, indices: &[usize]) -> Result<Vec<usize>> {
        let uids: Vec<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .filter(|s| matches!(s.geometry, Geometry::Stroke { .. }))
            .map(|s| s.uid)
            .collect();
        if uids.is_empty() {
            bail!("select a line (a stroke) to outline");
        }
        self.sync()?;
        let mut made = Vec::new();
        for uid in uids {
            let Some(i) = self.shapes.iter().position(|s| s.uid == uid) else {
                continue;
            };
            let (rings, _) = self.area(i);
            let path = bezier::from_rings(
                &boolean::pathfinder(&[(rings, FillRule::NonZero)], PathOp::Unite),
                RESULT_TOL,
            );
            if path.is_empty() {
                continue;
            }
            let (color, params, name) =
                (self.shapes[i].color, self.fill_params(i), self.name_of(i));
            let id = self.insert_shape(&path, Some(i), &name)?;
            let i = self.shapes.iter().position(|s| s.uid == uid).unwrap();
            self.delete(i)?;
            let n = self.finish_new(&id, color, params)?;
            made.push(self.shapes[n].uid);
        }
        Ok(self.uid_indices(&made))
    }

    /// Add Outline: a new object on top of each filled object, following its
    /// edge (or a line `offset_mm` outside or inside it) as a satin border
    /// or a running stitch, sewn straight after the object it outlines.
    /// The fill itself is left as it is. Returns the new indices.
    pub fn add_outline(&mut self, indices: &[usize], spec: &OutlineSpec) -> Result<Vec<usize>> {
        let uids: Vec<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .filter(|s| matches!(s.geometry, Geometry::Fill { .. }))
            .map(|s| s.uid)
            .collect();
        if uids.is_empty() {
            bail!("select a filled object to outline");
        }
        if !spec.width_mm.is_finite() || !spec.offset_mm.is_finite() {
            bail!("the outline's width and offset must be numbers");
        }
        let width = spec
            .width_mm
            .clamp(OutlineSpec::MIN_WIDTH, OutlineSpec::MAX_WIDTH);
        let offset = spec
            .offset_mm
            .clamp(-OutlineSpec::MAX_OFFSET, OutlineSpec::MAX_OFFSET);
        self.sync()?;
        let mut made = Vec::new();
        for uid in uids {
            let Some(i) = self.shapes.iter().position(|s| s.uid == uid) else {
                continue;
            };
            let (rings, rule) = self.area(i);
            let path =
                bezier::from_rings(&boolean::offset(&rings, rule, offset * 10.0), RESULT_TOL);
            if path.is_empty() {
                continue;
            }
            let color = spec.color.unwrap_or(self.shapes[i].color);
            let params = match spec.stitch {
                OutlineStitch::Satin => Params {
                    kind: Some(Kind::Satin),
                    border: Some(true),
                    width: Some(width),
                    ..Params::default()
                },
                OutlineStitch::Run => Params {
                    kind: Some(Kind::Run),
                    ..Params::default()
                },
                OutlineStitch::TripleRun => Params {
                    kind: Some(Kind::Run),
                    repeats: Some(3),
                    ..Params::default()
                },
            };
            let name = format!("{}_outline", self.name_of(i));
            let id = self.insert_shape(&path, None, &name)?;
            let n = self.finish_new(&id, color, params)?;
            // Sewn straight after the object it outlines (and that object's
            // own stroke, when its element has one).
            let source = self.shapes.iter().position(|s| s.uid == uid);
            let new_uid = self.shapes[n].uid;
            if let Some(src) = source {
                let src_id = self.shapes[src].id.clone();
                let last = self
                    .shapes
                    .iter()
                    .rposition(|s| s.id == src_id)
                    .unwrap_or(src);
                self.move_shape(n, if n > last { last + 1 } else { last });
                // (Written into the SVG, so the next outline's re-read keeps it.)
                self.sync()?;
            }
            made.push(new_uid);
        }
        if made.is_empty() {
            bail!("nothing left to outline at that offset");
        }
        Ok(self.uid_indices(&made))
    }

    /// Merge Layers: two or more objects become one, in the thread and with
    /// the stitch settings of the one sewn last (the one on top), in its
    /// place in the sewing order. Filled shapes are united, so where they
    /// overlapped is sewn once; lines stay lines, as one object. Returns
    /// the merged object's index.
    pub fn merge(&mut self, indices: &[usize]) -> Result<usize> {
        let mut uids: Vec<usize> = Vec::new();
        for &i in indices {
            if let Some(s) = self.shapes.get(i)
                && !uids.contains(&s.uid)
            {
                uids.push(s.uid);
            }
        }
        if uids.len() < 2 {
            bail!("select two or more layers to merge");
        }
        self.sync()?;
        let idx = self.uid_indices(&uids);
        let top = *idx.last().unwrap();
        // Where the top object ends up once the others are gone.
        let slot = top + 1 - idx.len();
        let (color, name) = (self.shapes[top].color, self.name_of(top));
        let lines_only = idx
            .iter()
            .all(|&i| matches!(self.shapes[i].geometry, Geometry::Stroke { .. }));
        let n = if lines_only {
            // Lines: one path with every line in it, as wide as the top one.
            let mut path = BezPath::new();
            for &i in &idx {
                path.extend(self.edit_path(i)?);
            }
            if path.is_empty() {
                bail!("nothing would be left");
            }
            let Geometry::Stroke { width, .. } = self.shapes[top].geometry else {
                unreachable!("every object is a line");
            };
            let mut params = self.shapes[top].params.clone();
            params.order = None;
            let id = self.insert_stroke(&open_ended(path), color, width / 10.0, &name)?;
            self.delete_many(&idx)?;
            self.finish_new(&id, color, params)?
        } else {
            let parts: Vec<(Vec<Polyline>, FillRule)> = idx.iter().map(|&i| self.area(i)).collect();
            let path = bezier::from_rings(&boolean::pathfinder(&parts, PathOp::Unite), RESULT_TOL);
            if path.is_empty() {
                bail!("nothing would be left");
            }
            let mut params = self.fill_params(top);
            params.order = None;
            let id = self.insert_shape(&path, None, &name)?;
            self.delete_many(&idx)?;
            self.finish_new(&id, color, params)?
        };
        let uid = self.shapes[n].uid;
        self.move_shape(n, slot.min(self.shapes.len() - 1));
        self.shapes
            .iter()
            .position(|s| s.uid == uid)
            .ok_or_else(|| anyhow!("the merged layer went missing"))
    }

    /// Add a stroked line (design units, `width_mm` wide) at the end of the
    /// document, named `name`. Returns its id; call `finish_new` afterwards.
    fn insert_stroke(
        &mut self,
        path: &BezPath,
        color: [u8; 3],
        width_mm: f64,
        name: &str,
    ) -> Result<String> {
        let m = self.mm_to_user()?;
        let d = bezier::to_svg_d(&bezier::transform(path, [0.1, 0.0, 0.0, 0.1, 0.0, 0.0]));
        let used: std::collections::HashSet<String> = {
            let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
            doc.descendants()
                .filter_map(|n| n.attribute("id").map(str::to_string))
                .collect()
        };
        let mut k = 2;
        let mut id = format!("{name}_#{k}");
        while used.contains(&id) {
            k += 1;
            id = format!("{name}_#{k}");
        }
        self.next_group += 1;
        let el = format!(
            "<g id=\"tl-drawing-{}\" transform=\"{}\"><path id=\"{}\" d=\"{d}\" fill=\"none\" stroke=\"#{:02x}{:02x}{:02x}\" stroke-width=\"{:.2}\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/></g>\n",
            self.next_group,
            super::edit::fmt_matrix(m),
            xml_escape(&id),
            color[0],
            color[1],
            color[2],
            width_mm.clamp(0.1, 20.0),
        );
        let at = self
            .xml
            .rfind("</svg>")
            .ok_or_else(|| anyhow!("document has no closing </svg>"))?;
        self.xml.insert_str(at, &el);
        Ok(id)
    }

    /// Add Knockdown: a light lattice under objects (all of them, if
    /// `indices` is empty), reaching `margin_mm` past them, sewn before the
    /// first of them, to flatten a towel's or fleece's pile so their
    /// stitches don't sink into it. It follows their combined outline with
    /// the holes filled in. Returns the new object's index.
    pub fn add_knockdown(&mut self, indices: &[usize], spec: &KnockdownSpec) -> Result<usize> {
        if !spec.margin_mm.is_finite() || !spec.spacing_mm.is_finite() {
            bail!("the knockdown's margin and spacing must be numbers");
        }
        let margin = spec.margin_mm.clamp(0.0, KnockdownSpec::MAX_MARGIN);
        let spacing = spec
            .spacing_mm
            .clamp(KnockdownSpec::MIN_SPACING, KnockdownSpec::MAX_SPACING);
        let all: Vec<usize> = (0..self.shapes.len()).collect();
        let uids: Vec<usize> = if indices.is_empty() { &all } else { indices }
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            // Not what isn't sewn, and not another knockdown.
            .filter(|s| !matches!(s.params.kind, Some(Kind::Skip | Kind::Knockdown)))
            .map(|s| s.uid)
            .collect();
        if uids.is_empty() {
            bail!("nothing to put a knockdown under");
        }
        self.sync()?;
        let idx = self.uid_indices(&uids);
        let first = idx[0];
        let parts: Vec<(Vec<Polyline>, FillRule)> = idx.iter().map(|&i| self.area(i)).collect();
        // Everything as one shape, then each of its rings on its own as a
        // solid: the holes (a letter's counters, a ring's middle) fill in.
        let united = boolean::pathfinder(&parts, PathOp::Unite);
        let solids: Vec<(Vec<Polyline>, FillRule)> = united
            .into_iter()
            .map(|r| (vec![r], FillRule::NonZero))
            .collect();
        let solid = boolean::pathfinder(&solids, PathOp::Unite);
        let path = bezier::from_rings(
            &boolean::offset(&solid, FillRule::EvenOdd, margin * 10.0),
            RESULT_TOL,
        );
        if path.is_empty() {
            bail!("those objects cover no area");
        }
        let params = Params {
            kind: Some(Kind::Knockdown),
            spacing: Some(spacing),
            ..Params::default()
        };
        // (Not "knockdown": that word in an id is the stitch type.)
        let id = self.insert_shape(&path, None, "base_layer")?;
        let n = self.finish_new(&id, spec.color, params)?;
        let uid = self.shapes[n].uid;
        // Sewn before the first object it lies under.
        self.move_shape(n, first.min(n));
        self.sync()?;
        self.shapes
            .iter()
            .position(|s| s.uid == uid)
            .ok_or_else(|| anyhow!("the knockdown went missing"))
    }

    /// Move shape `from` to position `to` in the sewing order (both indices
    /// into `shapes` as they are now; `to` is where it sits afterwards).
    fn move_shape(&mut self, from: usize, to: usize) {
        if from == to || from >= self.shapes.len() {
            return;
        }
        let s = self.shapes.remove(from);
        let to = to.min(self.shapes.len());
        self.shapes.insert(to, s);
        self.rev = super::next_rev();
    }

    /// Illustrator's Release Compound Path: a `<path>` made of several
    /// separate shapes becomes a group of one `<path>` per shape, each with
    /// the original's paint, transform and stitch settings, so each can be
    /// set up on its own. Holes stay with the shape they cut into (a ring
    /// stays a ring). Returns the new objects' indices and the new groups'
    /// ids.
    pub fn release_compound(&mut self, indices: &[usize]) -> Result<(Vec<usize>, Vec<String>)> {
        self.sync()?;
        let mut ids: Vec<String> = Vec::new();
        for &i in indices {
            let Some(s) = self.shapes.get(i) else {
                continue;
            };
            if self.lettering(i).is_some() || ids.contains(&s.id) {
                continue;
            }
            ids.push(s.id.clone());
        }
        let tree = self.parsed()?;
        let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
        let by_id = super::elements_by_id(&doc);
        let mut used: std::collections::HashSet<String> = doc
            .descendants()
            .filter_map(|n| n.attribute("id").map(str::to_string))
            .collect();
        let mut unique = |base: &str| {
            let mut k = 1;
            let mut id = format!("{base}_#{k}");
            while used.contains(&id) {
                k += 1;
                id = format!("{base}_#{k}");
            }
            used.insert(id.clone());
            id
        };
        let mut writes: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        // New element id -> the look it inherits, per part.
        let mut looks: Vec<(String, Part, [u8; 3], crate::params::Params)> = Vec::new();
        let mut groups = Vec::new();
        for id in &ids {
            let Some(i) = self.shapes.iter().position(|s| &s.id == id) else {
                continue;
            };
            let Ok((path, m)) = self.usvg_path(&tree, i) else {
                continue;
            };
            let pieces = split_pieces(&path);
            if pieces.len() < 2 {
                continue;
            }
            let Some(inv) = affine_inv(m) else { continue };
            let Some(&n) = by_id.get(id.as_str()) else {
                continue;
            };
            if n.tag_name().name() != "path" {
                continue;
            }
            // Everything but the id and the path data goes onto each piece.
            let mut attrs = String::new();
            for a in n.attributes() {
                if a.namespace().is_none() && (a.name() == "id" || a.name() == "d") {
                    continue;
                }
                attrs.push(' ');
                attrs.push_str(&self.xml[a.range()]);
            }
            let name = self.name_of(i);
            let gid = unique(&name);
            let mut el = format!("<g id=\"{}\">", xml_escape(&gid));
            // Numbered, so the pieces read apart in the Objects list.
            for (k, piece) in pieces.iter().enumerate() {
                let pid = unique(&format!("{name}_{}", k + 1));
                let d = bezier::to_svg_d(&bezier::transform(piece, inv));
                el.push_str(&format!(
                    "<path id=\"{}\"{attrs} d=\"{}\"/>",
                    xml_escape(&pid),
                    xml_escape(&d)
                ));
                for s in self.shapes.iter().filter(|s| &s.id == id) {
                    looks.push((pid.clone(), s.part, s.color, s.params.clone()));
                }
            }
            el.push_str("</g>");
            writes.push((n.range(), el));
            groups.push(gid);
        }
        drop(doc);
        if writes.is_empty() {
            bail!("select a path made of separate shapes (a compound path)");
        }
        let map = self.uid_map(&HashMap::new());
        writes.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
        for (range, text) in writes {
            self.xml.replace_range(range, &text);
        }
        self.import(&map)?;
        let mut made = Vec::new();
        for (pid, part, color, params) in looks {
            if let Some(s) = self
                .shapes
                .iter_mut()
                .find(|s| s.id == pid && s.part == part)
            {
                s.color = color;
                s.params = params;
                made.push(s.uid);
            }
        }
        Ok((self.uid_indices(&made), groups))
    }

    /// Illustrator's Simplify: fewer points, smooth curves, within `tol`
    /// units of the original outline.
    pub fn simplify_paths(&mut self, indices: &[usize], tol: f64) -> Result<usize> {
        let uids: Vec<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        let mut removed = 0;
        let mut seen = std::collections::HashSet::new();
        let mut edits = Vec::new();
        for uid in uids {
            let Some(i) = self.shapes.iter().position(|s| s.uid == uid) else {
                continue;
            };
            if !seen.insert(self.shapes[i].id.clone()) {
                continue;
            }
            let path = self.edit_path(i)?;
            let before: usize = path.iter().map(|sp| sp.anchors.len()).sum();
            let simple = bezier::simplify(&path, tol);
            let after: usize = simple.iter().map(|sp| sp.anchors.len()).sum();
            if after < before && !simple.is_empty() {
                edits.push((i, simple));
                removed += before - after;
            }
        }
        self.set_paths(&edits)?;
        Ok(removed)
    }

    /// Cut the corners of the objects at `indices` (Illustrator's Round
    /// Corners, made real): `radius` in design units, each fitted so cuts
    /// never pass each other. `only`, when not empty, limits it to those
    /// anchors (of a single object). Returns how many corners were cut.
    pub fn round_corners(
        &mut self,
        indices: &[usize],
        radius: f64,
        kind: bezier::CornerKind,
        only: &[bezier::AnchorRef],
    ) -> Result<usize> {
        let uids: Vec<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        let mut cut = 0;
        let mut seen = std::collections::HashSet::new();
        let mut edits = Vec::new();
        for uid in uids {
            let Some(i) = self.shapes.iter().position(|s| s.uid == uid) else {
                continue;
            };
            if !seen.insert(self.shapes[i].id.clone()) {
                continue;
            }
            let path = self.edit_path(i)?;
            let wanted = |r: bezier::AnchorRef| only.is_empty() || only.contains(&r);
            let n = bezier::path_corners(&path)
                .iter()
                .filter(|c| wanted(c.at_ref))
                .count();
            if n == 0 || radius <= 0.0 {
                continue;
            }
            let out = bezier::cut_corners(&path, |r| if wanted(r) { radius } else { 0.0 }, kind);
            edits.push((i, out));
            cut += n;
        }
        self.set_paths(&edits)?;
        Ok(cut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svg::Geometry;

    const ART: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="40mm" viewBox="0 0 60 40">
      <g transform="translate(5 5)">
        <circle id="dot" cx="10" cy="10" r="6" fill="#c00" stroke="#000" stroke-width="0.5" transform="rotate(10 10 10)"/>
        <path id="wave" d="M20 20 L40 20" stroke="#00c" fill="none" stroke-width="0.4"/>
      </g>
    </svg>"##;

    fn bounds(d: &Document, i: usize) -> crate::geom::Bounds {
        let mut b = crate::geom::Bounds::empty();
        match &d.shapes[i].geometry {
            Geometry::Fill { rings, .. } => rings.iter().flatten().for_each(|&p| b.add(p)),
            Geometry::Stroke { lines, .. } => {
                lines.iter().flat_map(|(l, _)| l).for_each(|&p| b.add(p))
            }
        }
        b
    }

    #[test]
    fn release_compound_path_splits_shapes_and_keeps_holes() {
        // Three separate squares in one path; the first has a square hole.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="20mm" viewBox="0 0 60 20">
          <path id="pills" fill="#420" fill-rule="evenodd" transform="translate(1 1)"
            d="M0 0H16V16H0Z M4 4H12V12H4Z M20 0H36V16H20Z M40 0H56V16H40Z"/>
        </svg>"##;
        let mut d = Document::from_svg(svg, "t", None).unwrap();
        assert_eq!(d.shapes.len(), 1);
        d.shapes[0].params.angle = Some(45.0);
        let before = bounds(&d, 0);
        let (made, groups) = d.release_compound(&[0]).unwrap();
        assert_eq!(made.len(), 3, "one object per separate shape");
        assert_eq!(groups.len(), 1);
        let mut rings: Vec<usize> = made
            .iter()
            .map(|&i| match &d.shapes[i].geometry {
                Geometry::Fill { rings, .. } => rings.len(),
                _ => 0,
            })
            .collect();
        rings.sort();
        assert_eq!(rings, vec![1, 1, 2], "the hole stays with its square");
        let mut after = crate::geom::Bounds::empty();
        for &i in &made {
            assert_eq!(
                d.shapes[i].params.angle,
                Some(45.0),
                "settings carried over"
            );
            assert_eq!(d.shapes[i].color, [0x44, 0x22, 0x00]);
            assert_eq!(d.shapes[i].groups.last(), Some(&groups[0]));
            let b = bounds(&d, i);
            after.add(b.min);
            after.add(b.max);
        }
        assert!((after.min - before.min).len() < 0.5 && (after.max - before.max).len() < 0.5);
        let out = d.to_svg().unwrap();
        assert!(out.contains(&format!("<g id=\"{}\"", groups[0])), "{out}");
        assert!(
            d.release_compound(&made[..1]).is_err(),
            "a single shape has nothing to release"
        );
    }

    #[test]
    fn editing_points_round_trips_through_the_svg() {
        let mut d = Document::from_svg(ART, "t", None).unwrap();
        let i = d.shapes.iter().position(|s| s.id == "wave").unwrap();
        let uid = d.shapes[i].uid;
        let mut p = d.edit_path(i).unwrap();
        assert_eq!(p[0].anchors.len(), 2);
        let b0 = bounds(&d, i);
        // Bend it: pull the first point's handle down 50 units (5 mm).
        let a = p[0].anchors[0].p;
        p[0].anchors[0].h_out = a + P::new(30.0, 50.0);
        d.set_path(i, &p).unwrap();
        let i = d.shapes.iter().position(|s| s.uid == uid).unwrap();
        let b1 = bounds(&d, i);
        assert!(b1.max.y > b0.max.y + 10.0, "it curves down now");
        let again = d.edit_path(i).unwrap();
        assert!((again[0].anchors[0].h_out - p[0].anchors[0].h_out).len() < 0.05);
        assert!(d.to_svg().unwrap().contains(" C"));
    }

    #[test]
    fn a_circle_becomes_a_path_keeping_paint_and_transform() {
        let mut d = Document::from_svg(ART, "t", None).unwrap();
        let n = d.shapes.len();
        let i = d.shapes.iter().position(|s| s.id == "dot").unwrap();
        let uid = d.shapes[i].uid;
        let b0 = bounds(&d, i);
        let mut p = d.edit_path(i).unwrap();
        assert_eq!(p[0].anchors.len(), 4);
        // Unchanged: the same shape, now as a path.
        d.set_path(i, &p).unwrap();
        let i = d.shapes.iter().position(|s| s.uid == uid).unwrap();
        let b1 = bounds(&d, i);
        assert!((b1.min - b0.min).len() < 0.5 && (b1.max - b0.max).len() < 0.5);
        let xml = d.to_svg().unwrap();
        assert!(!xml.contains("<circle"), "{xml}");
        assert!(xml.contains("rotate(10 10 10)") && xml.contains("fill=\"#c00\""));
        assert_eq!(d.shapes.len(), n, "fill and outline both still there");
        // Drag a point out.
        p[0].anchors[0].p = p[0].anchors[0].p + P::new(40.0, 0.0);
        d.set_path(i, &p).unwrap();
        let i = d.shapes.iter().position(|s| s.uid == uid).unwrap();
        assert!(bounds(&d, i).width() > b0.width() + 20.0);
    }

    #[test]
    fn lettering_reads_as_its_glyph_outlines() {
        let mut d = Document::from_svg(ART, "t", None).unwrap();
        let spec = super::super::TextSpec {
            text: "A".into(),
            ..Default::default()
        };
        let i = d.add_lettering(&spec, (0.0, 0.0)).unwrap();
        let p = d.edit_path(i).unwrap();
        assert_eq!(p.len(), 2, "an A: outer ring and its hole");
        assert!(d.lettering(i).is_some(), "reading doesn't change it");
    }

    const SQUARES: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm" viewBox="0 0 40 40">
      <rect id="back" x="0" y="0" width="20" height="20" fill="#c00"/>
      <rect id="front" x="10" y="10" width="20" height="20" fill="#00c"/>
      <path id="line" d="M0 35 L30 35" stroke="#0a0" stroke-width="3" fill="none"/>
    </svg>"##;

    fn area_mm2(d: &Document, i: usize) -> f64 {
        match &d.shapes[i].geometry {
            Geometry::Fill { rings, .. } => {
                let mut v: Vec<f64> = rings.iter().map(|r| boolean::ring_area(r)).collect();
                v.sort_by(|a, b| b.total_cmp(a));
                let (outer, holes) = v.split_first().unwrap();
                (outer - holes.iter().sum::<f64>()) / 100.0
            }
            _ => 0.0,
        }
    }

    #[test]
    fn pathfinder_replaces_the_objects_with_one_shape() {
        for (op, mm2, color) in [
            (PathOp::Unite, 700.0, [0, 0, 0xcc]),
            (PathOp::MinusFront, 300.0, [0xcc, 0, 0]),
            (PathOp::Intersect, 100.0, [0, 0, 0xcc]),
            (PathOp::Exclude, 600.0, [0, 0, 0xcc]),
        ] {
            let mut d = Document::from_svg(SQUARES, "t", None).unwrap();
            let i = d.pathfinder(&[0, 1], op).unwrap();
            assert_eq!(d.shapes.len(), 2, "{op:?}: two squares became one");
            // Exclude leaves two L shapes (no holes): add them up.
            let a = if op == PathOp::Exclude {
                match &d.shapes[i].geometry {
                    Geometry::Fill { rings, .. } => {
                        rings.iter().map(|r| boolean::ring_area(r)).sum::<f64>() / 100.0
                    }
                    _ => 0.0,
                }
            } else {
                area_mm2(&d, i)
            };
            assert!((a - mm2).abs() < 1.0, "{op:?}: {a}");
            assert_eq!(d.shapes[i].color, color, "{op:?}");
            assert_eq!(i, 0, "{op:?}: sews where the back object did");
            // It survives a save and reload.
            let again = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
            assert_eq!(again.shapes.len(), 2);
        }
    }

    #[test]
    fn offset_outline_and_simplify() {
        let mut d = Document::from_svg(SQUARES, "t", None).unwrap();
        let new = d.offset_path(&[0], 20.0).unwrap();
        assert_eq!(d.shapes.len(), 4);
        let a = area_mm2(&d, new[0]);
        assert!(
            a > 20.0 * 20.0 + 4.0 * 20.0 * 2.0 - 1.0,
            "grew by 2 mm: {a}"
        );
        assert!(
            new[0]
                < d.shapes
                    .iter()
                    .position(|s| s.label.contains("back"))
                    .unwrap()
                    + 1
        );

        let mut d = Document::from_svg(SQUARES, "t", None).unwrap();
        let line = d.shapes.iter().position(|s| s.id == "line").unwrap();
        let new = d.outline_stroke(&[line]).unwrap();
        assert_eq!(d.shapes.len(), 3);
        assert!(matches!(d.shapes[new[0]].geometry, Geometry::Fill { .. }));
        assert!(
            (area_mm2(&d, new[0]) - 30.0 * 3.0).abs() < 2.0,
            "{}",
            area_mm2(&d, new[0])
        );

        let mut d = Document::from_svg(ART, "t", None).unwrap();
        let dot = d.shapes.iter().position(|s| s.id == "dot").unwrap();
        // Make it a 40-point polygon first, then simplify it back down.
        let p = d.edit_path(dot).unwrap();
        let ring: Vec<P> = (0..40)
            .map(|k| {
                crate::bezier::cubic_at(
                    &p[0].cubic(k / 10, (k / 10 + 1) % 4),
                    (k % 10) as f64 / 10.0,
                )
            })
            .collect();
        d.set_path(dot, &crate::bezier::from_rings(&[ring], 0.0))
            .unwrap();
        let dot = d
            .shapes
            .iter()
            .position(|s| s.label.contains("dot"))
            .unwrap();
        let removed = d.simplify_paths(&[dot], 1.5).unwrap();
        assert!(removed > 20, "{removed}");
    }

    const BADGE: &str = include_str!("../../examples/badge.svg");

    #[test]
    fn add_outline_puts_a_satin_border_on_top_of_the_fill() {
        let mut d = Document::from_svg(BADGE, "badge", None).unwrap();
        let n = d.shapes.len();
        let disc = d
            .shapes
            .iter()
            .position(|s| s.label.contains("disc"))
            .unwrap();
        let disc_uid = d.shapes[disc].uid;
        let (disc_color, disc_params) = (d.shapes[disc].color, d.shapes[disc].params.clone());
        let new = d
            .add_outline(
                &[disc],
                &OutlineSpec {
                    width_mm: 2.5,
                    color: Some([20, 30, 40]),
                    ..OutlineSpec::default()
                },
            )
            .unwrap();
        assert_eq!((d.shapes.len(), new.len()), (n + 1, 1));
        let o = &d.shapes[new[0]];
        // Sewn straight after the fill, which is untouched.
        assert_eq!(new[0], disc + 1);
        assert_eq!(d.shapes[disc].uid, disc_uid);
        assert_eq!(d.shapes[disc].color, disc_color);
        // (Its place in the sewing order is written down now; nothing else.)
        assert_eq!(
            Params {
                order: None,
                ..d.shapes[disc].params.clone()
            },
            disc_params
        );
        assert_eq!(o.params.kind, Some(Kind::Satin));
        assert!(o.params.is_border());
        assert_eq!(o.params.width, Some(2.5));
        assert_eq!(o.color, [20, 30, 40]);
        assert!(svg::name_words(&o.label).contains(&"outline".to_string()));
        // On the fill's own edge: the same size.
        let (a, b) = (bounds(&d, disc), bounds(&d, new[0]));
        assert!((a.width() - b.width()).abs() < 1.0, "{a:?} {b:?}");
        // It sews, as a column about as wide as asked, after the fill.
        let plan = d.plan();
        assert!(plan.per_shape[new[0]] > 50, "{:?}", plan.per_shape);
        let at = |i| plan.sew_order.iter().position(|&k| k == i).unwrap();
        assert!(at(new[0]) > at(disc));
        // It all survives a save and reopen, order included.
        let back = Document::from_svg(&d.to_svg().unwrap(), "badge", None).unwrap();
        assert_eq!(back.shapes.len(), n + 1);
        let o = &back.shapes[disc + 1];
        assert_eq!(
            (o.params.kind, o.params.is_border(), o.params.width, o.color),
            (Some(Kind::Satin), true, Some(2.5), [20, 30, 40])
        );
    }

    #[test]
    fn add_outline_offsets_runs_and_takes_several_fills() {
        let mut d = Document::from_svg(BADGE, "badge", None).unwrap();
        let n = d.shapes.len();
        let fills: Vec<usize> = (0..n)
            .filter(|&i| matches!(d.shapes[i].geometry, Geometry::Fill { .. }))
            .collect();
        assert!(fills.len() >= 2);
        let uids: Vec<usize> = fills.iter().map(|&i| d.shapes[i].uid).collect();
        let sizes: Vec<f64> = fills.iter().map(|&i| bounds(&d, i).width()).collect();
        // Every object selected: the fills get an outline each, lines none.
        let all: Vec<usize> = (0..n).collect();
        let new = d
            .add_outline(
                &all,
                &OutlineSpec {
                    stitch: OutlineStitch::TripleRun,
                    offset_mm: 1.0,
                    ..OutlineSpec::default()
                },
            )
            .unwrap();
        assert_eq!(new.len(), fills.len());
        assert_eq!(d.shapes.len(), n + fills.len());
        for (k, uid) in uids.iter().enumerate() {
            let src = d.shapes.iter().position(|s| s.uid == *uid).unwrap();
            let o = &d.shapes[src + 1];
            assert!(new.contains(&(src + 1)), "outline follows its fill");
            assert_eq!(
                (o.params.kind, o.params.repeats),
                (Some(Kind::Run), Some(3))
            );
            // The fill's own thread when none is given.
            assert_eq!(o.color, d.shapes[src].color);
            // 1 mm outside all round: 2 mm (20 units) wider.
            let w = bounds(&d, src + 1).width();
            assert!((w - sizes[k] - 20.0).abs() < 1.5, "{w} vs {}", sizes[k]);
        }
        // A single run, inside the edge.
        let mut d = Document::from_svg(BADGE, "badge", None).unwrap();
        let disc = d
            .shapes
            .iter()
            .position(|s| s.label.contains("disc"))
            .unwrap();
        let w = bounds(&d, disc).width();
        let new = d
            .add_outline(
                &[disc],
                &OutlineSpec {
                    stitch: OutlineStitch::Run,
                    offset_mm: -2.0,
                    ..OutlineSpec::default()
                },
            )
            .unwrap();
        assert_eq!(d.shapes[new[0]].params.repeats, None);
        assert!((bounds(&d, new[0]).width() - (w - 40.0)).abs() < 1.5);
        // Nothing to outline: a line alone, or an offset that eats the shape.
        let line = d
            .add_drawing(
                &[P::new(0.0, 0.0), P::new(100.0, 0.0)],
                false,
                [0, 0, 0],
                Kind::Run,
            )
            .unwrap();
        assert!(d.add_outline(&[line], &OutlineSpec::default()).is_err());
        let disc = d
            .shapes
            .iter()
            .position(|s| s.label.contains("disc"))
            .unwrap();
        let gone = OutlineSpec {
            offset_mm: -20.0,
            ..OutlineSpec::default()
        };
        let before = d.shapes.len();
        assert!(d.add_outline(&[disc], &gone).is_err());
        assert_eq!(d.shapes.len(), before);
    }

    #[test]
    fn add_knockdown_goes_under_the_design_with_a_margin() {
        let mut d = Document::from_svg(BADGE, "badge", None).unwrap();
        let n = d.shapes.len();
        let all = svg::shapes_bounds(&d.shapes);
        let first_uid = d.shapes[0].uid;
        let k = d.add_knockdown(&[], &KnockdownSpec::default()).unwrap();
        assert_eq!(d.shapes.len(), n + 1);
        // First in the sewing order, a Knockdown in the fabric's color.
        assert_eq!(k, 0);
        assert_eq!(d.shapes[1].uid, first_uid);
        let s = &d.shapes[k];
        assert_eq!(s.params.kind, Some(Kind::Knockdown));
        assert_eq!(s.params.spacing, Some(2.0));
        assert_eq!(s.color, crate::document::DEFAULT_CLOTH);
        assert_eq!(svg::display_names(&d.shapes)[k], "base layer");
        // About 2 mm past the artwork all round (the ring is a 1.2 px line,
        // so its own width adds a little), and one solid piece: the
        // crescent cut out of the disc is filled in.
        let b = bounds(&d, k);
        let grow = (b.width() - all.width()) / 2.0;
        assert!((18.0..=26.0).contains(&grow), "{grow}");
        match &s.geometry {
            Geometry::Fill { rings, .. } => assert_eq!(rings.len(), 1, "no holes"),
            g => panic!("{g:?}"),
        }
        // It sews, lightly, before everything else; nothing is lost on top.
        let plan = d.plan();
        assert!(plan.per_shape[k] > 100);
        assert_eq!(plan.sew_order[0], k);
        let without = Document::from_svg(BADGE, "badge", None).unwrap().plan();
        assert_eq!(&plan.per_shape[1..], &without.per_shape[..]);
        // Saved and reopened: still a knockdown, still first.
        let back = Document::from_svg(&d.to_svg().unwrap(), "badge", None).unwrap();
        assert_eq!(back.shapes[0].params.kind, Some(Kind::Knockdown));
        assert_eq!(back.shapes.len(), n + 1);
        // A second one doesn't go under the first; one for a single object
        // sits just before that object.
        let sun = d
            .shapes
            .iter()
            .position(|s| s.label.contains("sun"))
            .unwrap();
        let spec = KnockdownSpec {
            margin_mm: 1.0,
            spacing_mm: 3.0,
            color: [1, 2, 3],
        };
        let k2 = d.add_knockdown(&[sun], &spec).unwrap();
        assert_eq!(k2, sun);
        assert_eq!(d.shapes[k2 + 1].label, d.shapes[sun + 1].label);
        assert_eq!(d.shapes[k2].color, [1, 2, 3]);
        assert!(bounds(&d, k2).width() < b.width() / 2.0);
        // Nothing but knockdowns selected: nothing to go under.
        assert!(d.add_knockdown(&[0], &spec).is_err());
        assert!(Document::empty().add_knockdown(&[], &spec).is_err());
    }

    #[test]
    fn merge_unites_fills_into_the_top_ones_layer() {
        let mut d = Document::empty();
        let sq = |x: f64, y: f64| {
            vec![
                P::new(x, y),
                P::new(x + 100.0, y),
                P::new(x + 100.0, y + 100.0),
                P::new(x, y + 100.0),
            ]
        };
        let a = d
            .add_drawing(&sq(0.0, 0.0), true, [200, 0, 0], Kind::Fill)
            .unwrap();
        let b = d
            .add_drawing(&sq(300.0, 0.0), true, [0, 200, 0], Kind::Fill)
            .unwrap();
        let c = d
            .add_drawing(&sq(50.0, 0.0), true, [0, 0, 200], Kind::Fill)
            .unwrap();
        let e = d
            .add_drawing(&sq(600.0, 0.0), true, [9, 9, 9], Kind::Fill)
            .unwrap();
        d.shapes[c].params.angle = Some(45.0);
        let (b_uid, e_uid) = (d.shapes[b].uid, d.shapes[e].uid);
        // A and C overlap by half: one 150 x 100 shape, in C's thread, with
        // C's settings, where C was in the order (between B and E).
        let m = d.merge(&[c, a]).unwrap();
        assert_eq!(d.shapes.len(), 3);
        assert_eq!(d.shapes[m].color, [0, 0, 200]);
        assert_eq!(d.shapes[m].params.angle, Some(45.0));
        assert_eq!(d.shapes[m].params.kind, Some(Kind::Fill));
        assert!((area_mm2(&d, m) - 150.0).abs() < 1.0, "{}", area_mm2(&d, m));
        let order: Vec<usize> = d.shapes.iter().map(|s| s.uid).collect();
        assert_eq!(order, vec![b_uid, d.shapes[m].uid, e_uid]);
        // Saved and reopened: still three, in that order.
        let back = Document::from_svg(&d.to_svg().unwrap(), "m", None).unwrap();
        assert_eq!(back.shapes.len(), 3);
        assert_eq!(back.shapes[1].color, [0, 0, 200]);
        assert_eq!(back.shapes[1].params.angle, Some(45.0));
        // Shapes that don't touch merge into one object with both in it.
        let m = d.merge(&[0, 2]).unwrap();
        assert_eq!(d.shapes.len(), 2);
        assert_eq!(d.shapes[m].color, [9, 9, 9]);
        match &d.shapes[m].geometry {
            Geometry::Fill { rings, .. } => {
                assert_eq!(rings.len(), 2);
                let total: f64 = rings.iter().map(|r| boolean::ring_area(r).abs()).sum();
                assert!((total / 100.0 - 200.0).abs() < 1.0, "{total}");
            }
            g => panic!("{g:?}"),
        }
        // One object (or the same one twice) is nothing to merge.
        assert!(d.merge(&[0]).is_err());
        assert!(d.merge(&[0, 0]).is_err());
        assert_eq!(d.shapes.len(), 2);
    }

    #[test]
    fn merging_lines_keeps_them_lines() {
        let mut d = Document::empty();
        let a = d
            .add_drawing(
                &[P::new(0.0, 0.0), P::new(100.0, 0.0)],
                false,
                [1, 2, 3],
                Kind::Run,
            )
            .unwrap();
        let b = d
            .add_drawing(
                &[P::new(0.0, 50.0), P::new(100.0, 50.0), P::new(100.0, 90.0)],
                false,
                [200, 0, 0],
                Kind::Run,
            )
            .unwrap();
        d.shapes[b].params.repeats = Some(3);
        let m = d.merge(&[a, b]).unwrap();
        assert_eq!(d.shapes.len(), 1);
        let s = &d.shapes[m];
        assert_eq!(
            svg::display_names(&d.shapes)[m],
            "drawing 2",
            "the top line's name"
        );
        assert_eq!(s.color, [200, 0, 0]);
        assert_eq!(
            (s.params.kind, s.params.repeats),
            (Some(Kind::Run), Some(3))
        );
        match &s.geometry {
            Geometry::Stroke { lines, .. } => {
                assert_eq!(lines.len(), 2);
                assert!(lines.iter().all(|(_, closed)| !closed));
            }
            g => panic!("{g:?}"),
        }
        // A line and a fill: the line's area joins the shape.
        let f = d
            .add_drawing(
                &[
                    P::new(0.0, 200.0),
                    P::new(100.0, 200.0),
                    P::new(100.0, 300.0),
                    P::new(0.0, 300.0),
                ],
                true,
                [5, 5, 5],
                Kind::Fill,
            )
            .unwrap();
        let m = d.merge(&[0, f]).unwrap();
        assert_eq!(d.shapes.len(), 1);
        assert!(matches!(d.shapes[m].geometry, Geometry::Fill { .. }));
        assert_eq!(d.shapes[m].color, [5, 5, 5]);
    }

    #[test]
    fn drawn_curves_are_written_as_curves() {
        let mut d = Document::from_svg(ART, "t", None).unwrap();
        let path = vec![SubPath {
            anchors: vec![
                Anchor {
                    p: P::new(0.0, 0.0),
                    h_in: P::new(0.0, 0.0),
                    h_out: P::new(0.0, 80.0),
                },
                Anchor {
                    p: P::new(100.0, 0.0),
                    h_in: P::new(100.0, 80.0),
                    h_out: P::new(100.0, 0.0),
                },
            ],
            closed: false,
        }];
        let i = d.add_drawing_path(&path, [0, 0, 0], Kind::Run).unwrap();
        let b = bounds(&d, i);
        assert!(b.max.y > 50.0, "bulges down: {:?}", b.max);
        let back = d.edit_path(i).unwrap();
        assert!((back[0].anchors[0].h_out - P::new(0.0, 80.0)).len() < 0.05);
    }
}

/// A compound path's separate shapes: each outer subpath with the holes
/// directly inside it (a subpath inside an odd number of others is a hole).
/// Open subpaths are each their own piece.
fn split_pieces(path: &BezPath) -> Vec<BezPath> {
    let rings: Vec<Polyline> = path.iter().map(flatten_subpath).collect();
    let inside = |j: usize, k: usize| {
        path[k].closed
            && rings[j]
                .first()
                .is_some_and(|&q| crate::geom::point_inside(&rings[k..=k], FillRule::NonZero, q))
    };
    let n = path.len();
    let depth: Vec<usize> = (0..n)
        .map(|j| (0..n).filter(|&k| k != j && inside(j, k)).count())
        .collect();
    let mut pieces: Vec<(usize, BezPath)> = Vec::new();
    for j in 0..n {
        if !path[j].closed || depth[j].is_multiple_of(2) {
            pieces.push((j, vec![path[j].clone()]));
        }
    }
    for j in 0..n {
        if path[j].closed && depth[j] % 2 == 1 {
            // The hole goes with the outer shape one level up that holds it.
            let owner = pieces
                .iter_mut()
                .find(|(o, _)| path[*o].closed && depth[*o] + 1 == depth[j] && inside(j, *o));
            match owner {
                Some((_, piece)) => piece.push(path[j].clone()),
                None => pieces.push((j, vec![path[j].clone()])),
            }
        }
    }
    pieces.into_iter().map(|(_, p)| p).collect()
}

fn flatten_subpath(sp: &SubPath) -> Polyline {
    let mut out: Polyline = Vec::new();
    if let Some(a) = sp.anchors.first() {
        out.push(a.p);
    }
    for (a, b) in sp.segments() {
        let c = sp.cubic(a, b);
        for k in 1..=16 {
            out.push(bezier::cubic_at(&c, k as f64 / 16.0));
        }
    }
    out
}
