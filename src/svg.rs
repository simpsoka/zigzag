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
    /// The stroke of an element that's also filled: the outline of that
    /// shape. Each of its subpaths is an edge to sew along (two of them
    /// are never the two rails of one satin column, as a stroked path's
    /// alone can be).
    pub outline: bool,
    /// Where the element's own origin is (units, as the geometry): the
    /// artwork's, for an element nothing has moved. What repeats across
    /// the canvas inside a shape, a fill's rows and the needle points
    /// along them, is laid out from here: shapes drawn together line up
    /// with each other, and a shape that's moved or turned takes its
    /// stitches with it, as they were, instead of sliding under a grid
    /// that stays put (which changes how its parts are sewn, and joined,
    /// with every move; as would opening the file again, were the grid
    /// laid out from the middle of the design, wherever that is by then).
    pub origin: P,
    /// Satin columns sewn as one satin (see `design::joined_satin`): how
    /// many of the geometry's subpaths are each column's, one after
    /// another. Only the planner makes these; empty for every element read.
    pub joined: Vec<usize>,
}

impl Shape {
    /// Lettering in a single-line font (see `retraced`): lines that are a
    /// fill's. Each line is a stroke of a letter, sewn along itself; never
    /// two of them the rails of one column.
    pub fn is_single_line(&self) -> bool {
        self.part == Part::Fill && matches!(self.geometry, Geometry::Stroke { .. })
    }

    /// Only keep an explicit off setting when a group would turn it on.
    pub fn trim_before_setting(&self, on: bool) -> Option<bool> {
        if on {
            return Some(true);
        }
        let base = || {
            self.groups
                .iter()
                .rev()
                .find_map(|id| Params::parse(id).trim_before)
        };
        let inherited = if self.part == Part::Stroke {
            self.inherited_stroke_trim_before().or_else(base)
        } else {
            base()
        };
        (inherited == Some(true)).then_some(false)
    }

    /// A stroke-specific group setting takes precedence over plain tokens.
    pub(crate) fn inherited_stroke_trim_before(&self) -> Option<bool> {
        self.groups
            .iter()
            .rev()
            .find_map(|id| stroke_only(id).trim_before)
    }
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

/// The fonts lettering can use: the system's, and any added since
/// ([`add_fonts`]).
pub fn fontdb() -> std::sync::Arc<usvg::fontdb::Database> {
    let db = fonts_cell()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(db) = db {
        return db;
    }
    let mut w = fonts_cell().write().unwrap_or_else(|e| e.into_inner());
    // (Another thread may have loaded them while this one waited.)
    w.get_or_insert_with(|| {
        // Loading system fonts is slow; do it once per process.
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        // (And the ones that come with ZigZag, and the app's own named
        // before there was a database.)
        let bundled = bundled_font_dir().map(|d| font_files(&d)).unwrap_or_default();
        for f in bundled.into_iter().chain(std::mem::take(&mut *waiting_fonts())) {
            let _ = db.load_font_file(f);
        }
        std::sync::Arc::new(db)
    })
    .clone()
}

/// The fonts that come with ZigZag: a `fonts` folder (with its list,
/// `fonts.json`) in the Mac app's Resources, or beside the program
/// elsewhere; in a debug build, the source tree's. `ZIGZAG_FONT_DIR` names
/// another folder of font files instead (the letter census uses it to try
/// fonts that aren't installed).
pub fn bundled_font_dir() -> Option<std::path::PathBuf> {
    if let Some(d) = std::env::var_os("ZIGZAG_FONT_DIR") {
        return Some(d.into());
    }
    let exe = std::env::current_exe().ok()?;
    let by_exe = exe.parent()?;
    #[allow(unused_mut)]
    let mut places = vec![by_exe.join("../Resources/fonts"), by_exe.join("fonts")];
    // (Not in a release build: that would name the folder it was built in.)
    #[cfg(debug_assertions)]
    places.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fonts"));
    places.into_iter().find(|p| p.join("fonts.json").is_file())
}

/// Every font file (TTF, OTF, TTC) in `dir` and the folders in it, sorted.
pub fn font_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut entries: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            out.extend(font_files(&p));
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
        {
            out.push(p);
        }
    }
    out
}

fn waiting_fonts() -> std::sync::MutexGuard<'static, Vec<std::path::PathBuf>> {
    static FILES: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());
    FILES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Font files lettering should always have: the fonts an app ships with,
/// or keeps from earlier downloads. Named at launch, before anything is
/// parsed, and cheap: if the system's fonts haven't been read yet, these
/// are read along with them (so nothing parsed can miss them); if they
/// have, they're added now ([`add_fonts`]).
pub fn use_fonts(files: Vec<std::path::PathBuf>) {
    {
        let cell = fonts_cell().write().unwrap_or_else(|e| e.into_inner());
        if cell.is_none() {
            waiting_fonts().extend(files);
            return;
        }
    }
    add_fonts(&files);
}

/// How many font faces lettering can use: it changes when fonts are added,
/// so what was measured in a font that wasn't there yet can be measured
/// again.
pub fn font_faces() -> usize {
    fontdb().len()
}

/// The characters of `text` that font `family` has no glyph for, each
/// once, in order (none if the family itself isn't there: that's another
/// matter). Such a character is drawn as the font's missing-character box
/// (see `parse_tree`).
pub fn font_lacks(family: &str, text: &str) -> Vec<char> {
    let db = fontdb();
    let query = usvg::fontdb::Query {
        families: &[usvg::fontdb::Family::Name(family)],
        ..Default::default()
    };
    let Some(id) = db.query(&query) else {
        return Vec::new();
    };
    db.with_face_data(id, |data, idx| {
        let Ok(face) = ttf_parser::Face::parse(data, idx) else {
            return Vec::new();
        };
        let mut out: Vec<char> = Vec::new();
        for c in text.chars() {
            // (Not what's never drawn anyway: joiners, soft hyphens,
            // variation selectors.)
            let unseen = matches!(c, '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{fe00}'..='\u{fe0f}');
            if !c.is_whitespace()
                && !c.is_control()
                && !unseen
                && face.glyph_index(c).is_none()
                && !out.contains(&c)
            {
                out.push(c);
            }
        }
        out
    })
    .unwrap_or_default()
}

fn fonts_cell() -> &'static std::sync::RwLock<Option<std::sync::Arc<usvg::fontdb::Database>>> {
    static DB: std::sync::RwLock<Option<std::sync::Arc<usvg::fontdb::Database>>> =
        std::sync::RwLock::new(None);
    &DB
}

/// Make font files usable for lettering from now on: fonts downloaded from
/// ZigZag's catalogue into the app's own folder, say. Designs parsed before
/// keep the fonts they were parsed with; anything parsed after sees these.
/// Returns how many faces were added (a file can hold several).
pub fn add_fonts(files: &[std::path::PathBuf]) -> usize {
    // (The system's, loaded first if they weren't yet.)
    fontdb();
    // Held from reading the fonts there are to putting the longer list
    // back, so two calls at once don't each drop the other's.
    let mut cell = fonts_cell().write().unwrap_or_else(|e| e.into_inner());
    let mut db = cell.as_deref().cloned().unwrap_or_default();
    let before = db.len();
    for f in files {
        // A file that isn't a font adds nothing (fontdb logs it).
        let _ = db.load_font_file(f);
    }
    let added = db.len() - before;
    if added > 0 {
        *cell = Some(std::sync::Arc::new(db));
    }
    added
}

/// The font database for parsing one SVG: the system fonts only when it
/// could hold text. Scanning them takes 100 ms or more (far longer than
/// reading and planning a typical design), and nothing else uses fonts.
fn fontdb_if(may_have_text: bool) -> std::sync::Arc<usvg::fontdb::Database> {
    if may_have_text {
        return fontdb();
    }
    static EMPTY: std::sync::OnceLock<std::sync::Arc<usvg::fontdb::Database>> =
        std::sync::OnceLock::new();
    EMPTY
        .get_or_init(|| std::sync::Arc::new(usvg::fontdb::Database::new()))
        .clone()
}

/// Whether raw SVG bytes could hold text (or an image, which may be an SVG
/// with text of its own). Errs towards yes: compressed data always could.
fn bytes_may_have_text(data: &[u8]) -> bool {
    let has = |word: &[u8]| data.windows(word.len()).any(|w| w == word);
    data.starts_with(&[0x1f, 0x8b]) || has(b"text") || has(b"image")
}

pub fn parse_tree(data: &[u8]) -> Result<usvg::Tree> {
    let may_have_text = bytes_may_have_text(data);
    let doc = may_have_text
        .then(|| std::str::from_utf8(data).ok())
        .flatten()
        .and_then(|t| roxmltree::Document::parse(t).ok());
    let opt = text_options(doc.as_ref(), may_have_text);
    usvg::Tree::from_data(data, &opt).context("parsing SVG")
}

/// How an SVG's text is read: with the fonts there are (if it may have
/// any text), and a character the text's font hasn't got left as the
/// font's own missing-character box, the letters round it staying in the
/// font. (Left to itself, usvg sets the whole text in another font that
/// has the character: one curly apostrophe, and a single-line font's
/// letters come back as some other font's outlines.) Text its font has
/// none of, Japanese in a Latin font say, is still set in one that does.
/// `doc`: the SVG, where it could be read here.
fn text_options(doc: Option<&roxmltree::Document>, may_have_text: bool) -> usvg::Options<'static> {
    let mut opt = usvg::Options {
        fontdb: fontdb_if(may_have_text),
        ..usvg::Options::default()
    };
    if may_have_text && let Some(keep) = doc.map(stand_in_chars) {
        let usual = usvg::FontResolver::default_fallback_selector();
        opt.font_resolver.select_fallback = Box::new(move |c, used, db| {
            if keep.contains(&c) {
                usual(c, used, db)
            } else {
                None
            }
        });
    }
    opt
}

/// The font each text of an SVG names (the first of its list, not `serif`
/// and the like) with the text it's named on: all that's inside the
/// `text`, `tspan` or `textPath` naming it.
pub fn named_fonts(doc: &roxmltree::Document) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for n in doc.descendants().filter(|n| {
        n.is_element() && matches!(n.tag_name().name(), "text" | "tspan" | "textPath")
    }) {
        let from_style = n.attribute("style").and_then(|st| {
            st.split(';').find_map(|d| {
                let (k, v) = d.split_once(':')?;
                (k.trim() == "font-family").then_some(v)
            })
        });
        let Some(list) = from_style.or(n.attribute("font-family")) else {
            continue;
        };
        let first = list.split(',').next().unwrap_or("").trim();
        let name = first.trim_matches(|c| c == '"' || c == '\'').trim();
        let generic = [
            "serif", "sans-serif", "monospace", "cursive", "fantasy", "system-ui",
        ];
        if name.is_empty() || generic.iter().any(|g| g.eq_ignore_ascii_case(name)) {
            continue;
        }
        let text: String = n
            .descendants()
            .filter(|t| t.is_text())
            .filter_map(|t| t.text())
            .collect();
        out.push((name.to_string(), text));
    }
    out
}

/// The characters that may still be drawn in a stand-in font: those of
/// text whose own font has none of its characters (or isn't here at all,
/// and is a stand-in already).
fn stand_in_chars(doc: &roxmltree::Document) -> std::collections::HashSet<char> {
    let db = fontdb();
    let mut keep = std::collections::HashSet::new();
    for (font, text) in named_fonts(doc) {
        let seen = |c: &char| !c.is_whitespace() && !c.is_control();
        let here = db
            .query(&usvg::fontdb::Query {
                families: &[usvg::fontdb::Family::Name(&font)],
                ..Default::default()
            })
            .is_some();
        let lacks = font_lacks(&font, &text);
        let all: std::collections::HashSet<char> = text.chars().filter(seen).collect();
        if !here || lacks.len() == all.len() {
            keep.extend(all);
        }
    }
    keep
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
            let may_have_text = doc
                .descendants()
                .any(|n| matches!(n.tag_name().name(), "text" | "image"));
            let opt = text_options(Some(doc), may_have_text);
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

/// Where a transform takes the origin, in units.
fn origin_of(ts: usvg::Transform) -> P {
    P::new(ts.tx as f64 * PX_TO_UNITS, ts.ty as f64 * PX_TO_UNITS)
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
    let origin = origin_of(ts);
    // Uniform scale of the transform, for stroke widths.
    let scale = ((ts.sx * ts.sy - ts.kx * ts.ky).abs() as f64).sqrt();
    let fill = path.fill().filter(|f| f.opacity().get() > 0.0);
    let stroke = path.stroke().filter(|s| s.opacity().get() > 0.0);
    // Whether the element's fill became an object.
    let mut filled = false;

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
            filled = push(
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
            )
            .map(|made| made.origin = origin)
            .is_some();
        }
    }
    if let Some(s) = stroke {
        // On an element that also has a fill, its unprefixed settings are
        // the fill's (the outline's are the `stroke.` ones, which is how
        // they're saved): an outline keeps its own thread and density
        // whatever its shape is given. What groups give still reaches both,
        // but for the stitch type. (Only where a filled shape was made of
        // the element: a line has a fill by SVG's default and nothing to
        // fill, and its settings are the line's, as they're saved.)
        let base = if filled {
            Params {
                kind: None,
                ..inherited.base.clone()
            }
        } else {
            ctx.base.clone()
        };
        let params = Params {
            // A route belongs to the element's own sewn part, not its group.
            // For a stroke-only element its unprefixed route is the stroke's.
            via: ctx.stroke.via.clone().or_else(|| {
                if filled { None } else { ctx.base.via.clone() }
            }),
            ..base.merged(&ctx.stroke)
        };
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
        // (With a filled shape too, it's that shape's outline.)
        let outline = filled;
        if let Some(made) = push(
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
        ) {
            made.outline = outline;
            made.origin = origin;
        }
    }
}

/// Text becomes one object: all glyph outlines in one fill.
fn import_text(text: &usvg::Text, inherited: &Inherit, out: &mut Vec<Shape>) {
    let ctx = inherited.child(text.id());
    let mut rings = Vec::new();
    let mut color = None;
    // (Flattened glyph paths carry their whole transform, the text's
    // included: usvg 0.50 made it absolute.)
    collect_glyphs(text.flattened(), &mut rings, &mut color);
    if !rings.is_empty() {
        let mut params = Params {
            kind: Some(ctx.base.kind.unwrap_or(Kind::Fill)),
            ..ctx.base.clone()
        };
        // A single-line font (for running stitch: Hershey, the EMS fonts)
        // draws each stroke out and back along itself, as a font's
        // outlines have to be closed: its letters are lines, not areas.
        let lines: Option<Vec<(Polyline, bool)>> = rings
            .iter()
            .map(|r| retraced(r).map(|l| (l, false)))
            .collect();
        let geometry = match lines {
            Some(lines) => {
                // (Sewn along the line: a run, unless a stitch that follows
                // a line was asked for, satin say, a narrow column.)
                if matches!(
                    params.kind,
                    Some(Kind::Fill | Kind::Contour | Kind::Applique | Kind::Meander | Kind::Ripple | Kind::Cross | Kind::Tartan)
                ) {
                    params.kind = Some(Kind::Run);
                }
                Geometry::Stroke {
                    lines,
                    width: params.width.map(|w| w * 10.0).unwrap_or(SINGLE_LINE_SATIN),
                }
            }
            None => Geometry::Fill {
                // (A stray line among outlines has no area to sew.)
                rings: rings.into_iter().filter(|r| r.len() >= 3).collect(),
                rule: FillRule::NonZero,
            },
        };
        if let Some(made) = push(
            out,
            text.id(),
            Part::Fill,
            &ctx,
            color.unwrap_or([0, 0, 0]),
            geometry,
            params,
        ) {
            made.origin = origin_of(text.abs_transform());
        }
    }
}

/// How wide a single-line font's strokes are sewn as satin, units.
const SINGLE_LINE_SATIN: f64 = 8.0;

/// The line a glyph contour traces out and back along (a single-line
/// font's stroke), or None for an outline around an area. A contour whose
/// points come back the way they went: `a b c b` is the line `a b c`.
fn retraced(ring: &[P]) -> Option<Polyline> {
    let n = ring.len();
    if n < 2 {
        return None;
    }
    // (Within 0.05 mm: the way back is flattened from the same curves.)
    let ok = (1..n).all(|i| ring[i].dist(ring[n - i]) < 0.5);
    ok.then(|| ring[..=n / 2].to_vec())
}

fn collect_glyphs(g: &usvg::Group, rings: &mut Vec<Polyline>, color: &mut Option<[u8; 3]>) {
    for node in g.children() {
        match node {
            usvg::Node::Group(g) => collect_glyphs(g, rings, color),
            usvg::Node::Path(p) => {
                if let Some(f) = p.fill() {
                    color.get_or_insert(paint_color(f.paint()));
                    let ts = p.abs_transform();
                    rings.extend(
                        flatten_path(p.data(), ts)
                            .into_iter()
                            .map(|(r, _)| r)
                            .filter(|r| r.len() >= 2),
                    );
                }
            }
            _ => {}
        }
    }
}

fn push<'a>(
    out: &'a mut Vec<Shape>,
    id: &str,
    part: Part,
    ctx: &Inherit,
    paint: [u8; 3],
    geometry: Geometry,
    params: Params,
) -> Option<&'a mut Shape> {
    if params.kind == Some(Kind::Skip) && id.is_empty() {
        return None;
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
        outline: false,
        origin: P::default(),
        joined: Vec::new(),
    });
    out.last_mut()
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

/// An editable path (design units) flattened the way [`flatten_path`]
/// flattens it once it's written into the SVG: what its shapes' geometry
/// will be, without the round trip through the file.
pub fn flatten_bez(path: &crate::bezier::BezPath) -> Vec<(Polyline, bool)> {
    let mut out = Vec::new();
    for sp in path {
        let Some(first) = sp.anchors.first() else {
            continue;
        };
        let mut cur: Polyline = vec![first.p];
        for (a, b) in sp.segments() {
            let c = sp.cubic(a, b);
            if c[1] == c[0] && c[2] == c[3] {
                cur.push(c[3]);
            } else {
                flatten_cubic(c[0], c[1], c[2], c[3], FLATTEN_TOL, &mut cur);
            }
        }
        if cur.len() < 2 {
            continue;
        }
        if sp.closed && cur.last().unwrap().dist(first.p) < 1e-6 {
            cur.pop();
        }
        out.push((cur, sp.closed));
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
        s.origin = s.origin - c;
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

#[cfg(test)]
mod added_fonts_tests {
    use super::*;

    #[test]
    fn fonts_added_after_launch_are_there_for_whatever_is_parsed_next() {
        // Any installed font file, copied where the app would keep downloads.
        let Some(src) = fontdb().faces().find_map(|f| match &f.source {
            usvg::fontdb::Source::File(p) if p.extension().is_some_and(|e| e == "ttf") => {
                Some(p.clone())
            }
            _ => None,
        }) else {
            return; // no font files on this machine
        };
        let dir = std::env::temp_dir().join(format!("zigzag-fonts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let copy = dir.join("downloaded.ttf");
        std::fs::copy(&src, &copy).unwrap();
        let junk = dir.join("not-a-font.ttf");
        std::fs::write(&junk, b"hello").unwrap();

        let before = fontdb().len();
        assert_eq!(add_fonts(std::slice::from_ref(&junk)), 0);
        assert_eq!(fontdb().len(), before);
        let added = add_fonts(&[copy.clone()]);
        assert!(added > 0);
        assert_eq!(fontdb().len(), before + added);
        assert!(
            fontdb()
                .faces()
                .any(|f| matches!(&f.source, usvg::fontdb::Source::File(p) if p == &copy))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The fonts that come with ZigZag are there with nothing installed
    /// (from the source tree's `fonts` in a debug build).
    #[test]
    fn the_bundled_fonts_are_there() {
        if std::env::var_os("ZIGZAG_FONT_DIR").is_some() {
            return;
        }
        let fams = crate::document::font_families();
        for f in ["Bebas Neue", "Sacramento", "Hershey Sans", "Relief SingleLine"] {
            assert!(fams.iter().any(|n| n == f), "{f} isn't there");
        }
    }

    fn text_in(font: &str, id: &str, text: &str) -> Vec<Shape> {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40"><text id="{id}" x="4" y="28" font-family="{font}" font-size="21" fill="#000000">{text}</text></svg>"##
        );
        load(svg.as_bytes()).unwrap()
    }

    /// A single-line font's letters are lines (each stroke drawn out and
    /// back, as a font has to), sewn as a run; an ordinary font's are
    /// areas.
    #[test]
    fn single_line_lettering_is_lines() {
        if std::env::var_os("ZIGZAG_FONT_DIR").is_some() {
            return;
        }
        let s = text_in("Hershey Sans", "fill", "HAx");
        assert_eq!(s.len(), 1);
        let Geometry::Stroke { lines, .. } = &s[0].geometry else {
            panic!("not lines: {:?}", s[0].geometry);
        };
        // H 3 strokes, A 3, x 2.
        assert_eq!(lines.len(), 8);
        assert_eq!(s[0].params.kind, Some(Kind::Run));
        // (Satin asked for: a narrow column along each stroke.)
        assert_eq!(text_in("Hershey Sans", "satin", "H")[0].params.kind, Some(Kind::Satin));
        let b = text_in("Bebas Neue", "fill", "H");
        assert!(matches!(b[0].geometry, Geometry::Fill { .. }));
        // A plain outline is never taken for a line.
        assert!(retraced(&[P::new(0.0, 0.0), P::new(10.0, 0.0), P::new(10.0, 10.0)]).is_none());
        assert_eq!(
            retraced(&[P::new(0.0, 0.0), P::new(10.0, 0.0), P::new(10.0, 10.0), P::new(10.0, 0.0)]),
            Some(vec![P::new(0.0, 0.0), P::new(10.0, 0.0), P::new(10.0, 10.0)])
        );
    }

    /// A character the font hasn't got is found, and is left as the font's
    /// missing-character box with the letters round it still the font's
    /// (not the whole text set in another font that has it); and a full
    /// stop or the dot of an i, short as its stroke is, is sewn.
    #[test]
    fn a_missing_character_is_found_and_dots_are_sewn() {
        if std::env::var_os("ZIGZAG_FONT_DIR").is_some() {
            return;
        }
        assert_eq!(font_lacks("Hershey Sans", "H\u{2019}s H\u{2019}"), ['\u{2019}']);
        assert!(font_lacks("Hershey Sans", "Mom's Caf\u{e9}").is_empty());
        assert!(font_lacks("No Such Font Anywhere", "\u{2605}").is_empty());
        // Still a single-line font's lines, the H and the s as they are
        // without the apostrophe between them.
        let with = text_in("Hershey Sans", "fill", "H\u{2019}s");
        let Geometry::Stroke { lines, .. } = &with[0].geometry else {
            panic!("set in another font: {:?}", with[0].geometry);
        };
        let Geometry::Stroke { lines: plain, .. } = &text_in("Hershey Sans", "fill", "Hs")[0].geometry
        else {
            unreachable!()
        };
        assert!(lines.len() >= plain.len(), "{} vs {}", lines.len(), plain.len());
        assert_eq!(lines[..3], plain[..3], "the H is Hershey's");
        // An ordinary font keeps its own letters too.
        let star = text_in("Bebas Neue", "fill", "H\u{2605}");
        let h = text_in("Bebas Neue", "fill", "H");
        let (Geometry::Fill { rings: a, .. }, Geometry::Fill { rings: b, .. }) =
            (&star[0].geometry, &h[0].geometry)
        else {
            panic!("not outlines");
        };
        assert_eq!(a[0], b[0], "the H is Bebas Neue's");
        for font in ["EMS Tech", "EMS Allure", "Hershey Sans"] {
            for (text, size) in [(".", 21), (":", 11), ("i", 11)] {
                let svg = format!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40"><text id="fill" x="4" y="28" font-family="{font}" font-size="{size}" fill="#000000">{text}</text></svg>"##
                );
                let shapes = load(svg.as_bytes()).unwrap();
                let Geometry::Stroke { lines, .. } = &shapes[0].geometry else {
                    panic!("{font}: not lines");
                };
                let plain: Vec<Polyline> = lines.iter().map(|(l, _)| l.clone()).collect();
                let sewn = crate::stitch::stroke::joined(&plain, 5.0);
                assert_eq!(sewn.len(), lines.len(), "{font} {text:?}: a stroke was dropped");
                let d = crate::design::plan("t", &shapes, &crate::design::PlanOptions::default());
                assert!(d.stitch_count() >= 2 * lines.len(), "{font} {text:?}: {}", d.stitch_count());
            }
        }
    }

    /// Each letter of a single-line font in one piece of thread: its
    /// strokes touch or cross, and it goes back along one where they
    /// branch rather than cutting the thread.
    #[test]
    fn single_line_satin_lettering_is_a_narrow_column_along_each_stroke() {
        if std::env::var_os("ZIGZAG_FONT_DIR").is_some() {
            return;
        }
        for font in ["Hershey Sans", "EMS Readability", "Relief SingleLine"] {
            let shapes = text_in(font, "satin", "Text");
            assert!(shapes[0].is_single_line(), "{font}");
            let Geometry::Stroke { lines, .. } = &shapes[0].geometry else {
                unreachable!()
            };
            let d = crate::design::plan("t", &shapes, &crate::design::PlanOptions::default());
            assert!(d.stitch_count() > 20, "{font}");
            // Every needle point lies by a stroke: within the column's
            // half-width (0.4 mm) and the pull, never across from one
            // stroke to another (two strokes read as one satin's rails).
            let near = |p: P| {
                lines.iter().flat_map(|(l, _)| l.windows(2)).any(|w| {
                    let (a, b) = (w[0], w[1]);
                    let (dx, dy) = (b.x - a.x, b.y - a.y);
                    let len = dx * dx + dy * dy;
                    let t = if len > 0.0 {
                        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    p.dist(P::new(a.x + t * dx, a.y + t * dy)) < 8.0
                })
            };
            for b in &d.blocks {
                for r in &b.runs {
                    // (A jump's ends are on strokes; what's between is travel.)
                    let far = r.pts.iter().filter(|&&p| !near(p)).count();
                    assert!(far == 0, "{font}: {far} needle points off the strokes");
                }
            }
        }
    }

    #[test]
    fn single_line_letters_sew_without_trims() {
        if std::env::var_os("ZIGZAG_FONT_DIR").is_some() {
            return;
        }
        for (font, letters) in [
            ("Hershey Sans", "HAEKkxt4"),
            ("EMS Readability", "HAEkxt4"),
            ("Relief SingleLine", "HAExt4"),
        ] {
            for ch in letters.chars() {
                let shapes = text_in(font, "fill", &ch.to_string());
                let d = crate::design::plan("t", &shapes, &crate::design::PlanOptions::default());
                assert_eq!(d.trim_count(), 0, "{font} {ch}");
                assert!(d.stitch_count() > 5, "{font} {ch}");
            }
        }
    }
}
