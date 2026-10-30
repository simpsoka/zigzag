//! SVG import: flattens an SVG (via usvg) into fill and stroke shapes in
//! embroidery units (0.1 mm), carrying colors and stitch parameters.

use crate::geom::{FillRule, P, Polyline, flatten_cubic, flatten_quad};
use crate::params::{Kind, Params};
use anyhow::{Context, Result};
use usvg::tiny_skia_path::PathSegment;

/// usvg works in CSS px (96 per inch); embroidery units are 0.1 mm.
pub const PX_TO_UNITS: f64 = 254.0 / 96.0;

/// Curve flattening tolerance in embroidery units (0.03 mm).
const FLATTEN_TOL: f64 = 0.3;

#[derive(Clone, Debug)]
pub enum Geometry {
    /// Closed rings of a filled area.
    Fill {
        rings: Vec<Polyline>,
        rule: FillRule,
    },
    /// Center lines of a stroke; `width` in units.
    Stroke {
        lines: Vec<(Polyline, bool)>,
        width: f64,
    },
}

/// Which paint of an SVG element a shape came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Fill,
    Stroke,
}

#[derive(Clone, Debug)]
pub struct Shape {
    /// Source element id (the key used to save settings back into the SVG).
    pub id: String,
    pub part: Part,
    /// Nearest named element (the element's own id, else its layer/group id).
    pub label: String,
    /// Thread color (the SVG paint unless overridden with `t=`).
    pub color: [u8; 3],
    /// The paint color in the SVG.
    pub paint: [u8; 3],
    pub geometry: Geometry,
    pub params: Params,
    /// Stable identity within an editing session.
    pub uid: usize,
    /// Ids of the groups this element sits in, outermost first (ZigZag's
    /// own wrapper groups left out).
    pub groups: Vec<String>,
}

/// ZigZag's own wrapper groups (placed artwork, pastes, lettering, pen
/// drawings) aren't groups the user made.
pub fn is_wrapper_group(id: &str) -> bool {
    ["tl-added-", "tl-paste-", "tl-lettering-", "tl-drawing-"]
        .iter()
        .any(|p| id.starts_with(p))
}

/// Ids of groups that are layers rather than groups someone made: Inkscape
/// layers anywhere, and at the top of the document Illustrator's default
/// "Layer 1" layers and clipped frames (Figma). Clicking selects inside a
/// layer, the way Illustrator does; any other group selects as a whole, even
/// one that holds the entire design.
pub fn layer_ids(data: &[u8]) -> std::collections::HashSet<String> {
    let Ok(text) = std::str::from_utf8(data) else {
        return std::collections::HashSet::new();
    };
    let Ok(doc) = roxmltree::Document::parse(text) else {
        return std::collections::HashSet::new();
    };
    layer_ids_in(&doc)
}

/// [`layer_ids`] of an already parsed document.
fn layer_ids_in(doc: &roxmltree::Document) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let layer_name = |s: &str| {
        let l = s.to_ascii_lowercase();
        let Some(rest) = l.strip_prefix("layer") else {
            return false;
        };
        let rest = rest.trim_start_matches(['_', ' ', '-']);
        !rest.is_empty()
            && rest.starts_with(|c: char| c.is_ascii_digit())
            && rest
                .chars()
                .all(|c| c.is_ascii_digit() || c == '-' || c == '_')
    };
    for n in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "g")
    {
        let Some(id) = n.attribute("id") else {
            continue;
        };
        let inkscape = n
            .attributes()
            .any(|a| a.name() == "groupmode" && a.value() == "layer");
        let top = n.ancestors().skip(1).filter(|a| a.is_element()).all(|a| {
            a.tag_name().name() == "svg" || a.attribute("id").is_some_and(is_wrapper_group)
        });
        let named = layer_name(id) || n.attribute("data-name").is_some_and(layer_name);
        let clipped = n.attribute("clip-path").is_some()
            || n.attribute("style")
                .is_some_and(|s| s.contains("clip-path"));
        if inkscape || (top && (named || clipped)) {
            out.insert(id.to_string());
        }
    }
    out
}

pub fn load_file(path: &std::path::Path) -> Result<Vec<Shape>> {
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    load(&data)
}

pub fn fontdb() -> std::sync::Arc<usvg::fontdb::Database> {
    // Loading system fonts is slow; do it once per process.
    static DB: std::sync::OnceLock<std::sync::Arc<usvg::fontdb::Database>> =
        std::sync::OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        std::sync::Arc::new(db)
    })
    .clone()
}

pub fn parse_tree(data: &[u8]) -> Result<usvg::Tree> {
    let opt = usvg::Options {
        fontdb: fontdb(),
        ..usvg::Options::default()
    };
    usvg::Tree::from_data(data, &opt).context("parsing SVG")
}

pub fn load(data: &[u8]) -> Result<Vec<Shape>> {
    Ok(load_with_tree(data)?.0)
}

/// [`load`], also handing back the parsed tree (reading an outline or a
/// transform later needn't parse the SVG again). The XML is parsed once,
/// for both usvg and the layer names.
pub fn load_with_tree(data: &[u8]) -> Result<(Vec<Shape>, usvg::Tree)> {
    let (shapes, tree, _) = load_with_tree_and(data, |_| ())?;
    Ok((shapes, tree))
}

/// [`load_with_tree`], also running `with_doc` over the parsed XML (so a
/// caller needing something else from it needn't parse it again). None
/// when the data wasn't plain XML (compressed, say).
pub fn load_with_tree_and<T>(
    data: &[u8],
    with_doc: impl FnOnce(&roxmltree::Document) -> T,
) -> Result<(Vec<Shape>, usvg::Tree, Option<T>)> {
    let parsed = std::str::from_utf8(data).ok().and_then(|text| {
        let xml_opt = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        roxmltree::Document::parse_with_options(text, xml_opt).ok()
    });
    let (tree, layers, extra) = match &parsed {
        Some(doc) => {
            let opt = usvg::Options {
                fontdb: fontdb(),
                ..usvg::Options::default()
            };
            (
                usvg::Tree::from_xmltree(doc, &opt).context("parsing SVG")?,
                layer_ids_in(doc),
                Some(with_doc(doc)),
            )
        }
        // Not plain UTF-8 XML (compressed, say): let usvg sort it out.
        None => (parse_tree(data)?, layer_ids(data), None),
    };
    let mut shapes = Vec::new();
    let ctx = Inherit::default();
    walk(tree.root(), &ctx, &mut shapes);
    for (i, s) in shapes.iter_mut().enumerate() {
        s.groups.retain(|g| !layers.contains(g));
        s.uid = i;
    }
    // Saved sewing order (`o=N`) wins over document order.
    if shapes.iter().any(|s| s.params.order.is_some()) {
        shapes.sort_by_key(|s| s.params.order.unwrap_or(u32::MAX));
    }
    Ok((shapes, tree, extra))
}

/// Settings inherited from enclosing groups.
#[derive(Clone, Default)]
struct Inherit<'a> {
    /// Unprefixed tokens (apply to fills, and to strokes of stroke-only elements).
    base: Params,
    /// `stroke.` tokens.
    stroke: Params,
    label: &'a str,
    groups: Vec<String>,
}

impl<'a> Inherit<'a> {
    fn child(&self, id: &'a str) -> Inherit<'a> {
        Inherit {
            groups: self.groups.clone(),
            base: self.base.merged(&Params::parse_part(id, false)),
            stroke: self.stroke.merged(&stroke_only(id)),
            // Auto-assigned ids (`tl-N`) and pure settings carry no name; keep the layer's.
            label: if name_words(id).is_empty() {
                self.label
            } else {
                id
            },
        }
    }
}

/// Only the `stroke.`-prefixed tokens of an id.
fn stroke_only(id: &str) -> Params {
    let decoded = crate::params::decode_illustrator_id(id);
    let toks: Vec<&str> = decoded
        .split([' ', '_', ',', ';'])
        .filter(|t| t.to_ascii_lowercase().starts_with("stroke."))
        .collect();
    Params::parse_part(&toks.join(" "), true)
}

fn walk(group: &usvg::Group, inherited: &Inherit, out: &mut Vec<Shape>) {
    let mut ctx = inherited.child(group.id());
    if !group.id().is_empty() && !is_wrapper_group(group.id()) {
        ctx.groups.push(group.id().to_string());
    }
    if ctx.base.kind == Some(Kind::Skip) && ctx.stroke.kind.is_none() {
        return;
    }
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => walk(g, &ctx, out),
            usvg::Node::Path(p) => import_path(p, &ctx, out),
            usvg::Node::Text(t) => import_text(t, &ctx, out),
            usvg::Node::Image(_) => {}
        }
    }
}

fn paint_color(paint: &usvg::Paint) -> [u8; 3] {
    match paint {
        usvg::Paint::Color(c) => [c.red, c.green, c.blue],
        usvg::Paint::LinearGradient(g) => stop_color(g.stops()),
        usvg::Paint::RadialGradient(g) => stop_color(g.stops()),
        usvg::Paint::Pattern(_) => [0, 0, 0],
    }
}

fn stop_color(stops: &[usvg::Stop]) -> [u8; 3] {
    stops
        .first()
        .map(|s| [s.color().red, s.color().green, s.color().blue])
        .unwrap_or([0, 0, 0])
}

fn fill_rule(r: usvg::FillRule) -> FillRule {
    match r {
        usvg::FillRule::NonZero => FillRule::NonZero,
        usvg::FillRule::EvenOdd => FillRule::EvenOdd,
    }
}

fn import_path(path: &usvg::Path, inherited: &Inherit, out: &mut Vec<Shape>) {
    if !path.is_visible() {
        return;
    }
    let ctx = inherited.child(path.id());
    let ts = path.abs_transform();
    let subpaths = flatten_path(path.data(), ts);
    if subpaths.is_empty() {
        return;
    }
    // Uniform scale of the transform, for stroke widths.
    let scale = ((ts.sx * ts.sy - ts.kx * ts.ky).abs() as f64).sqrt();
    let fill = path.fill().filter(|f| f.opacity().get() > 0.0);
    let stroke = path.stroke().filter(|s| s.opacity().get() > 0.0);

    if let Some(f) = fill {
        let rings: Vec<Polyline> = subpaths
            .iter()
            .map(|(pts, _)| pts.clone())
            .filter(|r| r.len() >= 3)
            .collect();
        if !rings.is_empty() {
            let params = Params {
                kind: Some(ctx.base.kind.unwrap_or(Kind::Fill)),
                ..ctx.base.clone()
            };
            push(
                out,
                path.id(),
                Part::Fill,
                &ctx,
                paint_color(f.paint()),
                Geometry::Fill {
                    rings,
                    rule: fill_rule(f.rule()),
                },
                params,
            );
        }
    }
    if let Some(s) = stroke {
        // On an element that also has a fill, unprefixed kinds belong to the fill.
        let base = if fill.is_some() {
            Params {
                kind: None,
                ..ctx.base.clone()
            }
        } else {
            ctx.base.clone()
        };
        let params = base.merged(&ctx.stroke);
        let width = params
            .width
            .map(|w| w * 10.0)
            .unwrap_or(s.width().get() as f64 * scale * PX_TO_UNITS);
        let kind = match params.kind {
            Some(Kind::Fill) | None => {
                if width >= 10.0 {
                    Kind::Satin
                } else {
                    Kind::Run
                }
            }
            Some(Kind::Contour | Kind::Applique) => Kind::Run,
            Some(k) => k,
        };
        let params = Params {
            kind: Some(kind),
            ..params
        };
        push(
            out,
            path.id(),
            Part::Stroke,
            &ctx,
            paint_color(s.paint()),
            Geometry::Stroke {
                lines: subpaths,
                width,
            },
            params,
        );
    }
}

/// Text becomes one object: all glyph outlines in one fill.
fn import_text(text: &usvg::Text, inherited: &Inherit, out: &mut Vec<Shape>) {
    let ctx = inherited.child(text.id());
    let mut rings = Vec::new();
    let mut color = None;
    // Flattened glyph paths are in the text's own space.
    collect_glyphs(
        text.flattened(),
        text.abs_transform(),
        &mut rings,
        &mut color,
    );
    if !rings.is_empty() {
        let params = Params {
            kind: Some(ctx.base.kind.unwrap_or(Kind::Fill)),
            ..ctx.base.clone()
        };
        let geometry = Geometry::Fill {
            rings,
            rule: FillRule::NonZero,
        };
        push(
            out,
            text.id(),
            Part::Fill,
            &ctx,
            color.unwrap_or([0, 0, 0]),
            geometry,
            params,
        );
    }
}

fn collect_glyphs(
    g: &usvg::Group,
    base: usvg::Transform,
    rings: &mut Vec<Polyline>,
    color: &mut Option<[u8; 3]>,
) {
    for node in g.children() {
        match node {
            usvg::Node::Group(g) => collect_glyphs(g, base, rings, color),
            usvg::Node::Path(p) => {
                if let Some(f) = p.fill() {
                    color.get_or_insert(paint_color(f.paint()));
                    let ts = base.pre_concat(p.abs_transform());
                    rings.extend(
                        flatten_path(p.data(), ts)
                            .into_iter()
                            .map(|(r, _)| r)
                            .filter(|r| r.len() >= 3),
                    );
                }
            }
            _ => {}
        }
    }
}

fn push(
    out: &mut Vec<Shape>,
    id: &str,
    part: Part,
    ctx: &Inherit,
    paint: [u8; 3],
    geometry: Geometry,
    params: Params,
) {
    if params.kind == Some(Kind::Skip) && id.is_empty() {
        return;
    }
    out.push(Shape {
        id: id.to_string(),
        part,
        label: ctx.label.to_string(),
        color: params.thread.unwrap_or(paint),
        paint,
        geometry,
        params,
        uid: 0,
        groups: ctx.groups.clone(),
    });
}

/// Flatten a path into polylines in embroidery units. Returns (points, closed).
pub fn flatten_path(
    data: &usvg::tiny_skia_path::Path,
    ts: usvg::Transform,
) -> Vec<(Polyline, bool)> {
    // Apply the affine transform in f64 for accuracy.
    let map = |p: usvg::tiny_skia_path::Point| {
        let (x, y) = (p.x as f64, p.y as f64);
        let nx = ts.sx as f64 * x + ts.kx as f64 * y + ts.tx as f64;
        let ny = ts.ky as f64 * x + ts.sy as f64 * y + ts.ty as f64;
        P::new(nx * PX_TO_UNITS, ny * PX_TO_UNITS)
    };
    let mut out = Vec::new();
    let mut cur: Polyline = Vec::new();
    let mut start = P::default();
    for seg in data.segments() {
        // A segment right after `Close` continues from the subpath start.
        if cur.is_empty() && !matches!(seg, PathSegment::MoveTo(_) | PathSegment::Close) {
            cur.push(start);
        }
        match seg {
            PathSegment::MoveTo(p) => {
                if cur.len() >= 2 {
                    out.push((std::mem::take(&mut cur), false));
                }
                cur.clear();
                start = map(p);
                cur.push(start);
            }
            PathSegment::LineTo(p) => cur.push(map(p)),
            PathSegment::QuadTo(p1, p2) => {
                let p0 = *cur.last().unwrap_or(&start);
                flatten_quad(p0, map(p1), map(p2), FLATTEN_TOL, &mut cur);
            }
            PathSegment::CubicTo(p1, p2, p3) => {
                let p0 = *cur.last().unwrap_or(&start);
                flatten_cubic(p0, map(p1), map(p2), map(p3), FLATTEN_TOL, &mut cur);
            }
            PathSegment::Close => {
                if cur.len() >= 2 {
                    if cur.last().unwrap().dist(start) < 1e-6 {
                        cur.pop();
                    }
                    out.push((std::mem::take(&mut cur), true));
                }
                cur.clear();
            }
        }
    }
    if cur.len() >= 2 {
        out.push((cur, false));
    }
    out
}

/// Bounds of all shape geometry, units.
pub fn shapes_bounds(shapes: &[Shape]) -> crate::geom::Bounds {
    let mut b = crate::geom::Bounds::empty();
    for s in shapes {
        match &s.geometry {
            Geometry::Fill { rings, .. } => rings.iter().flatten().for_each(|&p| b.add(p)),
            Geometry::Stroke { lines, .. } => {
                lines.iter().flat_map(|(l, _)| l).for_each(|&p| b.add(p))
            }
        }
    }
    b
}

/// Translate shapes so their combined bounds are centered on the origin (hoop center).
pub fn center_shapes(shapes: &mut [Shape]) {
    let c = shapes_bounds(shapes).center();
    for s in shapes {
        match &mut s.geometry {
            Geometry::Fill { rings, .. } => rings.iter_mut().flatten().for_each(|p| *p = *p - c),
            Geometry::Stroke { lines, .. } => lines
                .iter_mut()
                .flat_map(|(l, _)| l)
                .for_each(|p| *p = *p - c),
        }
    }
}

/// The name words of an id: setting tokens, auto-assigned `tl…` ids and `#N`
/// uniqueness suffixes removed. "sun_fill_a=-30_#2" → "sun".
pub fn name_words(id: &str) -> Vec<String> {
    let decoded = crate::params::decode_illustrator_id(id);
    if decoded.starts_with("tl-") {
        return Vec::new();
    }
    decoded
        .split([' ', '_', ',', ';'])
        // `#N` tokens are uniqueness suffixes added on save.
        .filter(|t| !t.is_empty() && !crate::params::is_param_token(t) && !t.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Readable labels for all shapes: name words, "Shape N" fallback, and an
/// "(outline)" suffix for the stroke of an element that also has a fill.
pub fn display_names(shapes: &[Shape]) -> Vec<String> {
    shapes
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let words = name_words(&s.label);
            let base = if words.is_empty() {
                format!("Shape {}", s.uid + 1)
            } else {
                words.join(" ")
            };
            let has_fill_sibling = s.part == Part::Stroke
                && !s.id.is_empty()
                && shapes
                    .iter()
                    .enumerate()
                    .any(|(j, o)| j != i && o.id == s.id && o.part == Part::Fill);
            if has_fill_sibling {
                format!("{base} (outline)")
            } else {
                base
            }
        })
        .collect::<Vec<String>>()
        .into_iter()
        .scan(
            std::collections::HashMap::<String, usize>::new(),
            |seen, name| {
                let n = seen.entry(name.clone()).or_insert(0);
                *n += 1;
                Some(if *n == 1 {
                    name
                } else {
                    format!("{name} ({n})")
                })
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keep_numbers_and_drop_settings() {
        assert_eq!(name_words("wave 1 satin w=2.4"), vec!["wave", "1"]);
        assert_eq!(name_words("sun_fill_a_x3D_-30_#2"), vec!["sun"]);
        assert!(name_words("tl-12").is_empty());
    }
}
