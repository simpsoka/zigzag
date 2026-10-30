//! Lettering objects: text typed in the studio, set in a system font, straight
//! or on an arc, and editable later. Stored as an ordinary SVG `<text>` (so
//! Illustrator shows real, editable text) with its settings in a
//! `data-tl-lettering` attribute.

use super::{Document, xml_escape};
use crate::geom::{Affine, P, affine_inv, affine_mul};
use crate::params::Kind;
use crate::svg::{self, Geometry, PX_TO_UNITS};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::{HashMap, HashSet};

/// Lettering settings by element id, each with the document revision and
/// the shape's color and stitch type they were read for. Shared by a
/// document's copies (undo steps), like its parsed tree.
pub(crate) type SpecMemo =
    std::sync::Arc<std::sync::Mutex<HashMap<String, (u64, [u8; 3], Kind, TextSpec)>>>;

/// Where the lines of a text object line up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

impl Align {
    pub fn name(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
        }
    }
    pub fn parse(s: &str) -> Option<Align> {
        match s {
            "left" | "start" => Some(Align::Left),
            "center" | "middle" => Some(Align::Center),
            "right" | "end" => Some(Align::Right),
            _ => None,
        }
    }
    fn anchor(self) -> &'static str {
        match self {
            Align::Left => "start",
            Align::Center => "middle",
            Align::Right => "end",
        }
    }
}

/// Letter case applied when the text is set (the typed text is kept).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Case {
    #[default]
    AsTyped,
    Upper,
    Lower,
    Title,
}

impl Case {
    pub fn name(self) -> &'static str {
        match self {
            Case::AsTyped => "as",
            Case::Upper => "upper",
            Case::Lower => "lower",
            Case::Title => "title",
        }
    }
    pub fn parse(s: &str) -> Option<Case> {
        match s {
            "as" | "typed" => Some(Case::AsTyped),
            "upper" => Some(Case::Upper),
            "lower" => Some(Case::Lower),
            "title" => Some(Case::Title),
            _ => None,
        }
    }
    pub fn apply(self, s: &str) -> String {
        match self {
            Case::AsTyped => s.to_string(),
            Case::Upper => s.to_uppercase(),
            Case::Lower => s.to_lowercase(),
            Case::Title => s
                .split(' ')
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        Some(f) => {
                            f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()
                        }
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// A text object's settings: Illustrator's Character and Paragraph panels,
/// the parts that matter for stitching, plus the baseline (straight, an
/// arc, or along a path you drew) and the stitch type.
#[derive(Clone, Debug, PartialEq)]
pub struct TextSpec {
    /// The text; newlines make lines.
    pub text: String,
    pub font: String,
    /// Height of capital letters, mm.
    pub size_mm: f64,
    /// Extra space between letters, mm (tracking).
    pub spacing_mm: f64,
    /// Baseline arc radius in mm: 0 = straight, > 0 arcs over the top (badge
    /// top), < 0 curves along the bottom.
    pub arc_mm: f64,
    pub bold: bool,
    pub italic: bool,
    pub color: [u8; 3],
    /// Stitch type for the letters.
    pub kind: Kind,
    /// Line spacing as a multiple of the cap height (1.5 ≈ normal).
    pub leading: f64,
    pub align: Align,
    /// Horizontal and vertical scale, percent.
    pub h_scale: f64,
    pub v_scale: f64,
    pub case: Case,
    /// Type on a path: the path (design units, relative to the text's
    /// origin) the baseline follows, instead of a line or arc.
    pub path: Option<crate::bezier::BezPath>,
    /// Where along the path the text starts, 0–1.
    pub path_offset: f64,
    /// Letters on the other side of the path (upside down, for the bottom
    /// of a circle).
    pub path_flip: bool,
    /// Pair kerning: (character index, mm) moves that character and the
    /// rest of its line by that much, so the gap before it changes.
    pub kern: Vec<(usize, f64)>,
}

impl Default for TextSpec {
    fn default() -> Self {
        TextSpec {
            text: "Text".into(),
            font: default_font(),
            size_mm: 8.0,
            spacing_mm: 0.4,
            arc_mm: 0.0,
            bold: true,
            italic: false,
            color: [30, 30, 30],
            kind: Kind::Satin,
            leading: 1.5,
            align: Align::Center,
            h_scale: 100.0,
            v_scale: 100.0,
            case: Case::AsTyped,
            path: None,
            path_offset: 0.0,
            path_flip: false,
            kern: Vec::new(),
        }
    }
}

impl TextSpec {
    /// The kerning before character `at`, mm.
    pub fn kern_at(&self, at: usize) -> f64 {
        self.kern
            .iter()
            .find(|(k, _)| *k == at)
            .map(|(_, v)| *v)
            .unwrap_or(0.0)
    }

    /// Set the kerning before character `at` (0 removes it).
    pub fn set_kern(&mut self, at: usize, mm: f64) {
        self.kern.retain(|(k, _)| *k != at);
        if mm.abs() >= 0.005 && at > 0 {
            self.kern.push((at, (mm * 100.0).round() / 100.0));
            self.kern.sort_by_key(|(k, _)| *k);
        }
    }

    /// SVG `dx`: one shift per character (the kerns), or nothing.
    fn dx_attr(&self) -> String {
        if self.kern.is_empty() {
            return String::new();
        }
        let n = self.case.apply(&self.text).chars().count();
        let last = self.kern.iter().map(|(k, _)| *k).max().unwrap_or(0).min(n);
        let list: Vec<String> = (0..=last)
            .map(|k| crate::params::fmt_num(self.kern_at(k)))
            .collect();
        format!(" dx=\"{}\"", list.join(" "))
    }

    /// The text as set: case applied, lines split.
    pub fn lines(&self) -> Vec<String> {
        self.case
            .apply(&self.text)
            .split('\n')
            .map(str::to_string)
            .collect()
    }
}

/// Font families available to lettering, sorted.
pub fn font_families() -> Vec<String> {
    let db = svg::fontdb();
    let mut v: Vec<String> = db
        .faces()
        .flat_map(|f| f.families.first().map(|(n, _)| n.clone()))
        .collect();
    v.sort_by_key(|s| s.to_lowercase());
    v.dedup();
    v
}

/// A clean sans font for embroidery lettering, whatever's installed.
pub fn default_font() -> String {
    let fams = font_families();
    for want in [
        "Poppins",
        "Inter",
        "Arial",
        "Helvetica",
        "Liberation Sans",
        "DejaVu Sans",
        "FreeSans",
    ] {
        if let Some(f) = fams.iter().find(|f| f.eq_ignore_ascii_case(want)) {
            return f.clone();
        }
    }
    fams.into_iter()
        .next()
        .unwrap_or_else(|| "sans-serif".into())
}

fn font_attrs(spec: &TextSpec) -> String {
    format!(
        "font-family=\"{}\" font-weight=\"{}\" font-style=\"{}\"",
        xml_escape(&spec.font),
        if spec.bold { "bold" } else { "normal" },
        if spec.italic { "italic" } else { "normal" }
    )
}

/// Cap height ÷ font size for this font (measured from an "H").
fn cap_ratio(spec: &TextSpec) -> f64 {
    type Ratios = HashMap<(String, bool, bool), f64>;
    static CACHE: std::sync::Mutex<Option<Ratios>> = std::sync::Mutex::new(None);
    let key = (spec.font.clone(), spec.bold, spec.italic);
    if let Some(v) = CACHE
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .get(&key)
    {
        return *v;
    }
    let probe = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200"><text x="10" y="150" font-size="100" {}>H</text></svg>"#,
        font_attrs(spec)
    );
    let ratio = svg::load(probe.as_bytes())
        .ok()
        .map(|shapes| svg::shapes_bounds(&shapes).height() / PX_TO_UNITS / 100.0)
        .filter(|r| *r > 0.2 && *r < 1.2)
        .unwrap_or(0.7);
    CACHE
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(key, ratio);
    ratio
}

/// Id token holding a text's settings (font, size, spacing, arc…), and the
/// one holding the text itself. Written into the id next to the stitch
/// settings so text survives Illustrator, which keeps ids (as layer names)
/// but drops `data-` attributes: as live type, or as outlines that can be
/// turned back into text.
const TYPE_KEY: &str = "type=";
const TEXT_KEY: &str = "txt=";

/// Percent-encode all but ASCII letters, digits, `.` and `-`, so a value is
/// a single token in an id (ids split on spaces, `_`, `,` and `;`).
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'.' || b == b'-' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn unpct(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `type=` and `txt=` tokens in an id (Illustrator's `_xHH_` escapes
/// undone), as found.
pub fn type_tokens(id: &str) -> Vec<String> {
    crate::params::decode_illustrator_id(id)
        .split([' ', '_', ',', ';'])
        .filter(|t| t.starts_with(TYPE_KEY) || t.starts_with(TEXT_KEY))
        .map(str::to_string)
        .collect()
}

/// True if the id records text (its settings and its characters).
pub fn has_type_tokens(id: &str) -> bool {
    let t = type_tokens(id);
    t.iter().any(|t| t.starts_with(TYPE_KEY)) && t.iter().any(|t| t.starts_with(TEXT_KEY))
}

impl TextSpec {
    /// The tokens that record this text in an id (see [`TYPE_KEY`]). A
    /// baseline path goes in too (mm), since there's no sibling path to
    /// read it from once Illustrator has had the file.
    pub(crate) fn id_tokens(&self) -> Vec<String> {
        let mut data = self.data_attr();
        if let Some(path) = &self.path {
            let mm = crate::bezier::transform(path, [0.1, 0.0, 0.0, 0.1, 0.0, 0.0]);
            data.push_str(";path=");
            data.push_str(&crate::bezier::to_svg_d(&mm));
        }
        vec![
            format!("{TYPE_KEY}{}", pct(&data)),
            format!("{TEXT_KEY}{}", pct(&self.text)),
        ]
    }

    /// Text recorded in an id by [`Self::id_tokens`], if any.
    pub fn from_id(id: &str, color: [u8; 3], kind: Kind) -> Option<TextSpec> {
        let toks = type_tokens(id);
        let data = toks.iter().find_map(|t| t.strip_prefix(TYPE_KEY)).map(unpct)?;
        let text = toks.iter().find_map(|t| t.strip_prefix(TEXT_KEY)).map(unpct)?;
        if text.trim().is_empty() {
            return None;
        }
        let mut spec = TextSpec::parse_data(&data, text, color, kind);
        if data.contains("onpath=1")
            && let Some(d) = data.split(';').find_map(|kv| kv.strip_prefix("path="))
        {
            let p = crate::bezier::from_svg_d(d);
            spec.path = Some(crate::bezier::transform(&p, [10.0, 0.0, 0.0, 10.0, 0.0, 0.0]));
        }
        Some(spec)
    }

    fn data_attr(&self) -> String {
        let n = crate::params::fmt_num;
        let mut s = format!(
            "font={};size={};spacing={};arc={};bold={};italic={}",
            self.font.replace(';', ","),
            n(self.size_mm),
            n(self.spacing_mm),
            n(self.arc_mm),
            self.bold as u8,
            self.italic as u8
        );
        if self.leading != 1.5 {
            s.push_str(&format!(";leading={}", n(self.leading)));
        }
        if self.align != Align::Center {
            s.push_str(&format!(";align={}", self.align.name()));
        }
        if self.h_scale != 100.0 {
            s.push_str(&format!(";hscale={}", n(self.h_scale)));
        }
        if self.v_scale != 100.0 {
            s.push_str(&format!(";vscale={}", n(self.v_scale)));
        }
        if self.case != Case::AsTyped {
            s.push_str(&format!(";case={}", self.case.name()));
        }
        if self.path.is_some() {
            s.push_str(&format!(
                ";onpath=1;offset={};flip={}",
                n(self.path_offset),
                self.path_flip as u8
            ));
        }
        if !self.kern.is_empty() {
            s.push_str(";kern=");
            s.push_str(
                &self
                    .kern
                    .iter()
                    .map(|(k, v)| format!("{k}:{}", n(*v)))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        s
    }

    fn parse_data(v: &str, text: String, color: [u8; 3], kind: Kind) -> TextSpec {
        let mut s = TextSpec {
            text,
            color,
            kind,
            ..TextSpec::default()
        };
        for kv in v.split(';') {
            let Some((k, v)) = kv.split_once('=') else {
                continue;
            };
            let num = v.parse::<f64>().ok().filter(|n| n.is_finite());
            match k {
                "font" => s.font = v.to_string(),
                "size" => s.size_mm = num.unwrap_or(s.size_mm).max(1.0),
                "spacing" => s.spacing_mm = num.unwrap_or(0.0),
                "arc" => s.arc_mm = num.unwrap_or(0.0),
                "bold" => s.bold = v == "1",
                "italic" => s.italic = v == "1",
                "leading" => s.leading = num.unwrap_or(1.5).clamp(0.5, 5.0),
                "align" => s.align = Align::parse(v).unwrap_or_default(),
                "hscale" => s.h_scale = num.unwrap_or(100.0).clamp(20.0, 400.0),
                "vscale" => s.v_scale = num.unwrap_or(100.0).clamp(20.0, 400.0),
                "case" => s.case = Case::parse(v).unwrap_or_default(),
                "offset" => s.path_offset = num.unwrap_or(0.0).clamp(0.0, 1.0),
                "flip" => s.path_flip = v == "1",
                "kern" => {
                    s.kern = v
                        .split(',')
                        .filter_map(|kv| {
                            let (k, v) = kv.split_once(':')?;
                            Some((k.parse().ok()?, v.parse::<f64>().ok()?))
                        })
                        .collect();
                }
                _ => {}
            }
        }
        s
    }

    /// The `<text>` element (plus its baseline path, if any), in mm around
    /// the origin.
    fn element(&self, id: &str, arc_id: &str, transform: Option<&str>) -> String {
        let n = crate::params::fmt_num;
        let font_size = self.size_mm / cap_ratio(self);
        let fill = format!(
            "#{:02x}{:02x}{:02x}",
            self.color[0], self.color[1], self.color[2]
        );
        let t = transform
            .map(|t| format!(" transform=\"{}\"", xml_escape(t)))
            .unwrap_or_default();
        // Horizontal and vertical scale: the glyphs scaled about the origin
        // (the scale sits inside the object's own transform).
        let scaled = self.h_scale != 100.0 || self.v_scale != 100.0;
        let scale_attr = if scaled {
            format!(
                " transform=\"scale({} {})\"",
                n(self.h_scale / 100.0),
                n(self.v_scale / 100.0)
            )
        } else {
            String::new()
        };
        let common = format!(
            "id=\"{}\"{t} data-tl-lettering=\"{}\" {} font-size=\"{}\" letter-spacing=\"{}\"{} fill=\"{fill}\" text-anchor=\"{}\"",
            xml_escape(id),
            xml_escape(&self.data_attr()),
            font_attrs(self),
            n(font_size),
            n(self.spacing_mm),
            self.dx_attr(),
            self.align.anchor(),
        );
        let lines = self.lines();
        let body = |s: &str| xml_escape(s);
        if let Some(path) = &self.path {
            // Type on a path: the baseline is the path (in mm, which is
            // 1/10 of design units), reversed to put the letters on its
            // other side. Lines beyond the first are ignored.
            let mut p = path.clone();
            if self.path_flip {
                p = p.iter().map(crate::bezier::reversed).collect();
            }
            let scale = 0.1;
            for sp in &mut p {
                for a in &mut sp.anchors {
                    a.p = a.p * scale;
                    a.h_in = a.h_in * scale;
                    a.h_out = a.h_out * scale;
                }
            }
            let d = crate::bezier::to_svg_d(&p);
            let text = lines.first().cloned().unwrap_or_default();
            let offset = match self.align {
                Align::Left => self.path_offset * 100.0,
                Align::Center => 50.0 + (self.path_offset - 0.5) * 100.0,
                Align::Right => 100.0 - (1.0 - self.path_offset) * 100.0,
            };
            return format!(
                "<path id=\"{arc_id}\" d=\"{d}\" fill=\"none\" stroke=\"none\"/><text {common}><textPath href=\"#{arc_id}\" startOffset=\"{}%\">{}</textPath></text>",
                n(offset.clamp(0.0, 100.0)),
                body(&text)
            );
        }
        if self.arc_mm.abs() < 0.5 {
            // Lines stacked `leading` cap heights apart, the block centered
            // on the origin; each line's caps sit on its baseline.
            let step = self.size_mm * self.leading;
            let total = step * (lines.len().saturating_sub(1)) as f64;
            let x = match self.align {
                Align::Left => -0.0,
                Align::Center => 0.0,
                Align::Right => 0.0,
            };
            let spans: String = lines
                .iter()
                .enumerate()
                .map(|(k, l)| {
                    let y = self.size_mm / 2.0 - total / 2.0 + k as f64 * step;
                    format!("<tspan x=\"{}\" y=\"{}\">{}</tspan>", n(x), n(y), body(l))
                })
                .collect();
            if scaled {
                return format!("<text {common}{scale_attr}>{spans}</text>");
            }
            return format!("<text {common}>{spans}</text>");
        }
        let r = self.arc_mm.abs();
        let d = if self.arc_mm > 0.0 {
            // Over the top: circle center below, letters standing on the arc.
            format!("M {} {r} A {r} {r} 0 0 1 {r} {r}", -r)
        } else {
            // Along the bottom: circle center above, letters inside it.
            format!("M {} {} A {r} {r} 0 0 0 {r} {}", -r, -r, -r)
        };
        let text = lines.first().cloned().unwrap_or_default();
        let offset = match self.align {
            Align::Left => 0.0,
            Align::Center => 50.0,
            Align::Right => 100.0,
        };
        format!(
            "<path id=\"{arc_id}\" d=\"{d}\" fill=\"none\" stroke=\"none\"/><text {common}{scale_attr}><textPath href=\"#{arc_id}\" startOffset=\"{}%\">{}</textPath></text>",
            n(offset),
            body(&text)
        )
    }
}

impl Document {
    /// mm around the hoop center → this document's root user units.
    pub(super) fn mm_to_user(&self) -> Result<Affine> {
        let r_inv = affine_inv(self.root_transform()?)
            .ok_or_else(|| anyhow!("document root transform isn't invertible"))?;
        let k = 10.0 / PX_TO_UNITS;
        Ok(affine_mul(
            r_inv,
            [
                k,
                0.0,
                0.0,
                k,
                self.offset.x / PX_TO_UNITS,
                self.offset.y / PX_TO_UNITS,
            ],
        ))
    }

    /// Add lettering centered at `center_mm`. Returns the new shape's index.
    pub fn add_lettering(&mut self, spec: &TextSpec, center_mm: (f64, f64)) -> Result<usize> {
        self.add_lettering_named(spec, center_mm, None)
    }

    /// [`Self::add_lettering`] with the element's id (name) chosen, made
    /// unique if it's taken.
    fn add_lettering_named(
        &mut self,
        spec: &TextSpec,
        center_mm: (f64, f64),
        name: Option<&str>,
    ) -> Result<usize> {
        if spec.text.trim().is_empty() {
            bail!("type some text first");
        }
        self.sync()?;
        let m = affine_mul(
            self.mm_to_user()?,
            crate::geom::translation(P::new(center_mm.0, center_mm.1)),
        );
        let words: Vec<String> = spec
            .text
            .split_whitespace()
            .take(3)
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
            })
            .filter(|w| !w.is_empty())
            .collect();
        let mut id = match name {
            Some(given) => given.to_string(),
            None => format!(
                "text_{}",
                if words.is_empty() {
                    "lettering".into()
                } else {
                    words.join("_")
                }
            ),
        };
        let used: HashSet<String> = {
            let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
            doc.descendants()
                .filter_map(|n| n.attribute("id").map(str::to_string))
                .collect()
        };
        let base = id.clone();
        let mut k = 2;
        while used.contains(&id) {
            id = format!("{base}_#{k}");
            k += 1;
        }
        // A group number not in the file yet (it may hold ZigZag groups
        // from an earlier session, or ones that came back from Illustrator).
        self.next_group += 1;
        while used.contains(&format!("tl-lettering-{}", self.next_group))
            || used.contains(&format!("tl-arc-{}", self.next_group))
        {
            self.next_group += 1;
        }
        let n = self.next_group;
        let el = spec.element(&id, &format!("tl-arc-{n}"), None);
        let group = format!(
            "<g id=\"tl-lettering-{n}\" transform=\"matrix({} {} {} {} {} {})\">{el}</g>\n",
            m[0], m[1], m[2], m[3], m[4], m[5]
        );
        let close = self
            .xml
            .rfind("</svg>")
            .ok_or_else(|| anyhow!("document has no closing </svg>"))?;
        self.xml.insert_str(close, &group);
        let map = self.uid_map(&HashMap::new());
        self.import(&map)?;
        let i = self
            .shapes
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| anyhow!("the font produced no outlines for that text"))?;
        self.shapes[i].params.kind = Some(spec.kind);
        // Arcs hang off their baseline: center the letters themselves on the
        // point asked for. Text on a path stays put: the point is the
        // path's center, which the baseline is relative to.
        let b = svg::shapes_bounds(&self.shapes[i..=i]);
        let shift = P::new(center_mm.0 * 10.0, center_mm.1 * 10.0) - b.center();
        if shift.len() > 1.0 && spec.path.is_none() {
            let uid = self.shapes[i].uid;
            self.transform(&[i], crate::geom::translation(shift))?;
            return self
                .shapes
                .iter()
                .position(|s| s.uid == uid)
                .ok_or_else(|| anyhow!("lettering vanished"));
        }
        Ok(i)
    }

    /// The lettering settings of shape `index`, if it's a lettering object.
    pub fn lettering(&self, index: usize) -> Option<TextSpec> {
        let s = self.shapes.get(index)?;
        if !self.lettering_ids.contains(&s.id) {
            return None;
        }
        let kind = s.params.kind.unwrap_or(Kind::Fill);
        if let Ok(memo) = self.lettering_memo.lock()
            && let Some((rev, color, k, spec)) = memo.get(&s.id)
            && *rev == self.rev
            && *color == s.color
            && *k == kind
        {
            return Some(spec.clone());
        }
        let spec = self.read_lettering(s)?;
        if let Ok(mut memo) = self.lettering_memo.lock() {
            // (A new revision: nothing older is worth keeping.)
            if memo.values().any(|(rev, ..)| *rev != self.rev) {
                memo.clear();
            }
            memo.insert(s.id.clone(), (self.rev, s.color, kind, spec.clone()));
        }
        Some(spec)
    }

    /// [`Self::lettering`], read from the SVG (a parse of the whole document).
    fn read_lettering(&self, s: &crate::svg::Shape) -> Option<TextSpec> {
        let doc = roxmltree::Document::parse(&self.xml).ok()?;
        let n = doc.descendants().find(|n| {
            n.is_element()
                && n.tag_name().name() == "text"
                && n.attribute("id") == Some(s.id.as_str())
        })?;
        let kind = s.params.kind.unwrap_or(Kind::Fill);
        let Some(data) = n.attribute("data-tl-lettering") else {
            // Text that has been through Illustrator: what's in its id.
            return TextSpec::from_id(&s.id, s.color, kind);
        };
        // Lines are tspans; a textPath holds one line.
        let text: String = {
            let spans: Vec<String> = n
                .children()
                .filter(|c| c.is_element() && c.tag_name().name() == "tspan")
                .map(|c| {
                    c.descendants()
                        .filter(|d| d.is_text())
                        .filter_map(|d| d.text())
                        .collect()
                })
                .collect();
            if spans.is_empty() {
                n.descendants()
                    .filter(|d| d.is_text())
                    .filter_map(|d| d.text())
                    .collect()
            } else {
                spans.join("\n")
            }
        };
        let mut spec = TextSpec::parse_data(data, text, s.color, kind);
        if data.contains("onpath=1")
            && let Some(prev) = n.prev_sibling_element()
            && prev.tag_name().name() == "path"
            && let Some(d) = prev.attribute("d")
        {
            // The baseline, back in design units.
            let mut p = crate::bezier::from_svg_d(d);
            for sp in &mut p {
                for a in &mut sp.anchors {
                    a.p = a.p * 10.0;
                    a.h_in = a.h_in * 10.0;
                    a.h_out = a.h_out * 10.0;
                }
            }
            if spec.path_flip {
                p = p.iter().map(crate::bezier::reversed).collect();
            }
            spec.path = Some(p);
        }
        // Case was applied when set; what's stored is the set text. Keep
        // the setting but show the text as it is.
        Some(spec)
    }

    /// Re-set lettering with new settings, keeping its place, transform and id.
    pub fn set_lettering(&mut self, index: usize, spec: &TextSpec) -> Result<()> {
        // (A lone space is allowed: the studio sets it while all the text
        // is deleted mid-edit, so the frame and caret have an element.)
        if spec.text.is_empty() || (spec.text.trim().is_empty() && spec.text != " ") {
            bail!("lettering can't be empty; delete the object instead");
        }
        let uid = self
            .shapes
            .get(index)
            .ok_or_else(|| anyhow!("no object {index}"))?
            .uid;
        self.sync()?;
        let s = self
            .shapes
            .iter()
            .find(|s| s.uid == uid)
            .ok_or_else(|| anyhow!("object vanished"))?;
        let id = s.id.clone();
        let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
        let n = doc
            .descendants()
            .find(|n| {
                n.is_element()
                    && n.tag_name().name() == "text"
                    && n.attribute("id") == Some(id.as_str())
            })
            .ok_or_else(|| anyhow!("that object isn't lettering"))?;
        let transform = n.attribute("transform").map(str::to_string);
        // The arc path sits right before the text, sharing its group.
        let mut range = n.range();
        let mut arc_id = None;
        if let Some(prev) = n.prev_sibling_element()
            && prev.tag_name().name() == "path"
            && prev
                .attribute("id")
                .is_some_and(|i| i.starts_with("tl-arc-"))
        {
            range.start = prev.range().start;
            arc_id = prev.attribute("id").map(str::to_string);
        }
        let arc_id = arc_id.unwrap_or_else(|| {
            self.next_group += 1;
            format!("tl-arc-{}", self.next_group)
        });
        // Keep the stitch settings already in the id.
        let el = spec.element(&id, &arc_id, transform.as_deref());
        drop(doc);
        self.xml.replace_range(range, &el);
        let map = self.uid_map(&HashMap::new());
        self.import(&map)?;
        if let Some(s) = self.shapes.iter_mut().find(|s| s.uid == uid) {
            s.params.kind = Some(spec.kind);
            s.color = spec.color;
            s.paint = spec.color;
        }
        Ok(())
    }
}

impl Document {
    /// Type › Create Outlines: the lettering becomes an ordinary shape of
    /// its glyph outlines (no longer editable as text), keeping its stitch
    /// settings and place in the order. Returns the new object's index.
    pub fn expand_lettering(&mut self, index: usize) -> Result<usize> {
        let Some(spec) = self.lettering(index) else {
            bail!("that object isn't lettering");
        };
        let s = &self.shapes[index];
        let Geometry::Fill { rings, .. } = &s.geometry else {
            bail!("the lettering has no outlines");
        };
        // The glyph outlines as drawn (curves come flattened finely; a
        // refit would round the sharp corners of letters like A and V).
        let path = crate::bezier::from_rings(rings, 0.0);
        let (color, params) = (s.color, s.params.clone());
        // The text stays recorded in the name, for Convert to Text.
        let words = svg::name_words(&s.id);
        let base = words
            .iter()
            .filter(|w| w.as_str() != "text")
            .cloned()
            .collect::<Vec<_>>()
            .join("_");
        let name = format!(
            "{}_outlines_{}",
            if base.is_empty() { "text" } else { &base },
            spec.id_tokens().join("_")
        );
        let id = self.insert_shape(&path, Some(index), &name)?;
        let uid_old = self.shapes[index].uid;
        let map = self.uid_map(&HashMap::new());
        self.import(&map)?;
        if let Some(old) = self.shapes.iter().position(|s| s.uid == uid_old) {
            self.delete(old)?;
        }
        let i = self
            .shapes
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| anyhow!("outlines vanished"))?;
        self.shapes[i].params = params;
        self.shapes[i].color = color;
        self.shapes[i].paint = color;
        Ok(i)
    }

    /// The id that records the text shape `k` was made from: its own, or a
    /// group's around it (Illustrator's Create Outlines makes a group of
    /// glyphs and gives it the text's name).
    fn type_source(&self, k: usize) -> Option<String> {
        let s = self.shapes.get(k)?;
        if self.lettering_ids.contains(&s.id) {
            return None;
        }
        std::iter::once(&s.id)
            .chain(s.groups.iter().rev())
            .chain(std::iter::once(&s.label))
            .find(|id| has_type_tokens(id))
            .cloned()
    }

    /// True if shape `k` is outlines that can become text again.
    pub fn records_text(&self, k: usize) -> bool {
        self.type_source(k).is_some()
    }

    /// Type › Convert to Text: outlines that record the text they came from
    /// (ZigZag's Create Outlines, or Illustrator's) become that text again,
    /// editable, with the outlines' stitch settings, centered where they were
    /// and in their place in the sewing order. Every outline from the same
    /// text goes (Illustrator makes one per letter). Point edits made to the
    /// outlines don't carry over. Returns the new text's index.
    pub fn restore_lettering(&mut self, index: usize) -> Result<usize> {
        let src = self
            .type_source(index)
            .ok_or_else(|| anyhow!("these outlines don't record any text"))?;
        self.sync()?;
        // (Syncing may have renamed things: look again.)
        let index = (0..self.shapes.len())
            .find(|&k| self.type_source(k).is_some_and(|t| type_tokens(&t) == type_tokens(&src)))
            .ok_or_else(|| anyhow!("the outlines went away"))?;
        let src = self.type_source(index).unwrap_or(src);
        let s = &self.shapes[index];
        let spec = TextSpec::from_id(&src, s.color, s.params.kind.unwrap_or(Kind::Fill))
            .ok_or_else(|| anyhow!("the text recorded there can't be read"))?;
        let params = s.params.clone();
        let members: Vec<usize> = (0..self.shapes.len())
            .filter(|&k| self.type_source(k).as_deref() == Some(src.as_str()))
            .collect();
        let picked: Vec<crate::svg::Shape> =
            members.iter().map(|&k| self.shapes[k].clone()).collect();
        let c = svg::shapes_bounds(&picked).center();
        let at = members[0];
        let words: Vec<String> = svg::name_words(&src)
            .into_iter()
            .filter(|w| w != "outlines" && w != "text")
            .collect();
        let name = format!(
            "text_{}",
            if words.is_empty() { "lettering".into() } else { words.join("_") }
        );
        for &k in members.iter().rev() {
            self.delete(k)?;
        }
        // An Illustrator outline group left empty goes too.
        self.drop_empty_groups(|id| id == src)?;
        let j = self.add_lettering_named(&spec, (c.x / 10.0, c.y / 10.0), Some(&name))?;
        Ok(self.settle_new_text(j, params, at))
    }

    /// Remove groups chosen by `which` that draw nothing any more (a text's
    /// invisible baseline path doesn't count).
    fn drop_empty_groups(&mut self, which: impl Fn(&str) -> bool) -> Result<()> {
        let ranges: Vec<std::ops::Range<usize>> = {
            let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
            doc.descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "g")
                .filter(|n| n.attribute("id").is_some_and(&which))
                .filter(|n| {
                    !n.descendants().any(|d| {
                        d.is_element()
                            && super::DRAWABLE.contains(&d.tag_name().name())
                            && !d.attribute("id").is_some_and(|i| i.starts_with("tl-arc-"))
                    })
                })
                .map(|n| n.range())
                .collect()
        };
        // Outermost first, then from the end back so ranges stay right.
        let mut ranges: Vec<_> = ranges
            .iter()
            .filter(|r| !ranges.iter().any(|o| o != *r && o.start <= r.start && r.end <= o.end))
            .cloned()
            .collect();
        ranges.sort_by_key(|r| std::cmp::Reverse(r.start));
        for r in ranges {
            self.xml.replace_range(r, "");
        }
        Ok(())
    }

    /// After adding text to stand in for another object: its stitch settings,
    /// and its place in the sewing order. Returns its index.
    fn settle_new_text(&mut self, j: usize, params: crate::params::Params, at: usize) -> usize {
        self.shapes[j].params = params;
        let s = self.shapes.remove(j);
        let at = at.min(self.shapes.len());
        self.shapes.insert(at, s);
        at
    }

    /// Live type that has been through Illustrator (its record in its id,
    /// ZigZag's own attributes gone, Illustrator's idea of the layout)
    /// becomes ZigZag text again on opening: the recorded settings, centered
    /// where it was, at its rotation, in its place in the sewing order.
    pub(super) fn adopt_foreign_lettering(&mut self) -> Result<()> {
        let ids: Vec<String> = {
            let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
            doc.descendants()
                .filter(|n| {
                    n.is_element()
                        && n.tag_name().name() == "text"
                        && n.attribute("data-tl-lettering").is_none()
                })
                .filter_map(|n| n.attribute("id"))
                .filter(|id| has_type_tokens(id))
                .map(str::to_string)
                .collect()
        };
        if ids.is_empty() {
            return Ok(());
        }
        let tree = self.parsed()?;
        let angles: HashMap<String, f64> = ids
            .iter()
            .filter_map(|id| {
                let t = tree.node_by_id(id)?.abs_transform();
                Some((id.clone(), (t.ky as f64).atan2(t.sx as f64).to_degrees()))
            })
            .collect();
        drop(tree);
        for id in ids {
            let Some(i) = self.shapes.iter().position(|s| s.id == id) else {
                continue;
            };
            let s = &self.shapes[i];
            let Some(spec) = TextSpec::from_id(&id, s.color, s.params.kind.unwrap_or(Kind::Fill))
            else {
                continue;
            };
            let params = s.params.clone();
            let c = svg::shapes_bounds(&self.shapes[i..=i]).center();
            self.delete(i)?;
            self.drop_empty_groups(|g| g.starts_with("tl-lettering-"))?;
            let j = self.add_lettering_named(&spec, (c.x / 10.0, c.y / 10.0), Some(&id))?;
            let j = self.settle_new_text(j, params, i);
            let angle = angles.get(&id).copied().unwrap_or(0.0);
            if angle.abs() > 0.05 {
                self.transform(&[j], crate::geom::rotate_about(c, angle))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_alignment_scale_and_case_round_trip() {
        let mut d = Document::empty();
        let spec = TextSpec {
            text: "camp\nfire".into(),
            size_mm: 6.0,
            leading: 1.2,
            align: Align::Left,
            h_scale: 120.0,
            v_scale: 90.0,
            case: Case::Upper,
            ..TextSpec::default()
        };
        let i = d.add_lettering(&spec, (0.0, 0.0)).unwrap();
        let b = svg::shapes_bounds(&d.shapes[i..=i]);
        // Two lines of 6 mm caps at 90% vertical scale, 1.2 leading:
        // about (6 + 7.2) × 0.9 = 11.9 mm tall.
        assert!((b.height() - 119.0).abs() < 12.0, "{}", b.height());
        let back = d.lettering(i).unwrap();
        assert_eq!(back.text, "CAMP\nFIRE", "case applied, lines kept");
        assert_eq!(back.align, Align::Left);
        assert_eq!(back.case, Case::Upper);
        assert!((back.h_scale - 120.0).abs() < 1e-9 && (back.v_scale - 90.0).abs() < 1e-9);
        assert!((back.leading - 1.2).abs() < 1e-9);
        let saved = d.to_svg().unwrap();
        let e = Document::from_svg(&saved, "t", None).unwrap();
        assert_eq!(e.lettering(0).unwrap().text, "CAMP\nFIRE");
        assert_eq!(Case::Title.apply("hello big WORLD"), "Hello Big World");
    }

    #[test]
    fn type_on_a_path_follows_it_and_round_trips() {
        let mut d = Document::empty();
        // A wave, 60 mm wide, in design units relative to the text origin.
        let path = vec![crate::bezier::SubPath {
            anchors: vec![
                crate::bezier::Anchor::corner(P::new(-300.0, 0.0)),
                crate::bezier::Anchor::corner(P::new(0.0, -150.0)),
                crate::bezier::Anchor::corner(P::new(300.0, 0.0)),
            ],
            closed: false,
        }];
        let spec = TextSpec {
            text: "WAVE TEXT".into(),
            size_mm: 5.0,
            path: Some(path.clone()),
            align: Align::Left,
            ..TextSpec::default()
        };
        let i = d.add_lettering(&spec, (0.0, 0.0)).unwrap();
        let b = svg::shapes_bounds(&d.shapes[i..=i]);
        assert!(b.width() > 250.0, "spread along the path: {}", b.width());
        assert!(b.height() > 100.0, "rises with the wave: {}", b.height());
        let back = d.lettering(i).unwrap();
        let p = back.path.clone().expect("path kept");
        assert_eq!(p[0].anchors.len(), 3);
        assert!(
            p[0].anchors[1].p.dist(P::new(0.0, -150.0)) < 1.0,
            "{:?}",
            p[0].anchors[1].p
        );
        // Flipped: still the same path when read back.
        let flipped = TextSpec {
            path_flip: true,
            ..back.clone()
        };
        d.set_lettering(i, &flipped).unwrap();
        let again = d.lettering(i).unwrap();
        assert!(again.path_flip);
        assert!(
            again.path.unwrap()[0].anchors[0]
                .p
                .dist(P::new(-300.0, 0.0))
                < 1.0
        );
        let saved = d.to_svg().unwrap();
        let e = Document::from_svg(&saved, "t", None).unwrap();
        assert!(e.lettering(0).unwrap().path.is_some());
    }

    #[test]
    #[ignore]
    fn bench_typing() {
        let mut d = Document::open(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/badge.svg"
        )))
        .unwrap();
        let i = d
            .add_lettering(
                &TextSpec {
                    text: "C".into(),
                    ..TextSpec::default()
                },
                (0.0, 30.0),
            )
            .unwrap();
        let mut spec = d.lettering(i).unwrap();
        let t = std::time::Instant::now();
        for ch in "AMPFIRE".chars() {
            spec.text.push(ch);
            d.set_lettering(i, &spec).unwrap();
        }
        let per = t.elapsed().as_secs_f64() * 1000.0 / 7.0;
        eprintln!("set_lettering: {per:.1} ms per keystroke");
        let t = std::time::Instant::now();
        let _ = d.plan();
        eprintln!("plan: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        let mut cache = crate::design::PlanCache::default();
        let _ = d.plan_cached(&mut cache);
        spec.text.push('S');
        d.set_lettering(i, &spec).unwrap();
        let t = std::time::Instant::now();
        let _ = d.plan_cached(&mut cache);
        eprintln!(
            "plan_cached after a keystroke: {:.1} ms",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }

    #[test]
    fn kerning_moves_the_pair_and_round_trips() {
        let mut d = Document::empty();
        let spec = TextSpec {
            text: "AV".into(),
            size_mm: 10.0,
            ..TextSpec::default()
        };
        let i = d.add_lettering(&spec, (0.0, 0.0)).unwrap();
        let wide = svg::shapes_bounds(&d.shapes[i..=i]).width();
        let mut tight = spec.clone();
        tight.set_kern(1, -2.0);
        d.set_lettering(i, &tight).unwrap();
        let narrow = svg::shapes_bounds(&d.shapes[i..=i]).width();
        assert!(
            (wide - narrow - 20.0).abs() < 2.0,
            "2 mm tighter: {wide} -> {narrow}"
        );
        let back = d.lettering(i).unwrap();
        assert_eq!(back.kern, vec![(1, -2.0)]);
        assert!((back.kern_at(1) + 2.0).abs() < 1e-9);
        let saved = d.to_svg().unwrap();
        assert!(saved.contains("dx=\"0 -2\""), "{saved}");
        let e = Document::from_svg(&saved, "t", None).unwrap();
        assert_eq!(e.lettering(0).unwrap().kern, vec![(1, -2.0)]);
        // Zero removes it.
        let mut none = back.clone();
        none.set_kern(1, 0.0);
        assert!(none.kern.is_empty());
    }

    /// An id as Illustrator writes it back: `=` and `%` escaped.
    fn illustrator_id(id: &str) -> String {
        id.replace('%', "_x25_").replace('=', "_x3D_")
    }

    #[test]
    fn text_settings_round_trip_through_an_id() {
        let spec = TextSpec {
            text: "Camp_Fire 2026\nHi; #1 ✓".into(),
            font: "Helvetica Neue".into(),
            size_mm: 7.5,
            spacing_mm: 0.4,
            arc_mm: 30.0,
            bold: true,
            align: Align::Left,
            h_scale: 90.0,
            case: Case::Upper,
            kern: vec![(2, -0.3)],
            kind: Kind::Fill,
            ..TextSpec::default()
        };
        let id = format!("text_camp_satin_w=2_{}", spec.id_tokens().join("_"));
        for id in [id.clone(), illustrator_id(&id)] {
            assert!(has_type_tokens(&id), "{id}");
            let back = TextSpec::from_id(&id, [1, 2, 3], Kind::Fill).unwrap();
            assert_eq!(back.text, spec.text);
            assert_eq!(back.font, spec.font);
            assert_eq!(back.size_mm, 7.5);
            assert_eq!(back.spacing_mm, 0.4);
            assert_eq!(back.arc_mm, 30.0);
            assert!(back.bold);
            assert_eq!(back.align, Align::Left);
            assert_eq!(back.h_scale, 90.0);
            assert_eq!(back.case, Case::Upper);
            assert_eq!(back.kern, vec![(2, -0.3)]);
            // The stitch settings in the same id are untouched by it.
            let p = crate::params::Params::parse(&id);
            assert_eq!(p.kind, Some(Kind::Satin));
            assert_eq!(p.width, Some(2.0));
        }
    }

    #[test]
    fn saved_text_records_itself_in_its_id() {
        let mut d = Document::empty();
        d.add_lettering(
            &TextSpec {
                text: "CAMP".into(),
                ..TextSpec::default()
            },
            (0.0, 0.0),
        )
        .unwrap();
        let out = d.to_svg().unwrap();
        assert!(out.contains("txt=CAMP"), "{out}");
        assert!(out.contains("type=font"), "{out}");
    }

    #[test]
    fn live_text_back_from_illustrator_is_text_again() {
        // ZigZag text as Illustrator writes it out: the record in the id,
        // `data-tl-lettering` gone, its own transform and font attributes.
        let spec = TextSpec {
            text: "CAMP".into(),
            size_mm: 10.0,
            kind: Kind::Satin,
            ..TextSpec::default()
        };
        let id = illustrator_id(&format!(
            "text_CAMP_satin_s=0.3_{}",
            spec.id_tokens().join("_")
        ));
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100"><g id="tl-lettering-1"><text id="{id}" transform="matrix(1 0 0 1 30 50)" font-family="{}" font-size="14" fill="#123456">CAMP</text></g><circle id="dot" cx="80" cy="80" r="5" fill="red"/></svg>"##,
            spec.font
        );
        let d = Document::from_svg(&svg, "t", None).unwrap();
        let i = (0..d.shapes.len())
            .find(|&i| d.lettering(i).is_some())
            .expect("the text is text");
        let back = d.lettering(i).unwrap();
        assert_eq!(back.text, "CAMP");
        assert_eq!(back.size_mm, 10.0);
        assert_eq!(d.shapes[i].params.kind, Some(Kind::Satin));
        assert_eq!(d.shapes[i].params.spacing, Some(0.3));
        assert_eq!(d.shapes[i].color, [0x12, 0x34, 0x56]);
        assert_eq!(i, 0, "keeps its place in the sewing order");
        assert!(d.xml.contains("data-tl-lettering"), "ZigZag's own text now");
        assert_eq!(
            d.xml.matches("id=\"tl-lettering-").count(),
            1,
            "the emptied group is gone, only the new one is left: {}",
            d.xml
        );
        // Its cap height is what was recorded (10 mm, design units ×10).
        let b = svg::shapes_bounds(&d.shapes[i..=i]);
        assert!((b.height() - 100.0).abs() < 15.0, "{b:?}");
    }

    #[test]
    fn outlines_convert_back_to_text() {
        let mut d = Document::empty();
        let i = d
            .add_lettering(
                &TextSpec {
                    text: "HIKE".into(),
                    ..TextSpec::default()
                },
                (5.0, -3.0),
            )
            .unwrap();
        d.shapes[i].params.angle = Some(30.0);
        let j = d.expand_lettering(i).unwrap();
        assert!(d.records_text(j), "the outlines remember the text");
        // Still remembered after a save and reopen.
        let mut d = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        let j = (0..d.shapes.len()).find(|&k| d.records_text(k)).unwrap();
        assert!(d.lettering(j).is_none());
        let before = svg::shapes_bounds(&d.shapes[j..=j]);
        let k = d.restore_lettering(j).unwrap();
        let back = d.lettering(k).expect("text again");
        assert_eq!(back.text, "HIKE");
        assert_eq!(d.shapes[k].params.angle, Some(30.0), "stitch settings kept");
        assert_eq!(d.shapes.len(), 1);
        let after = svg::shapes_bounds(&d.shapes[k..=k]);
        assert!((after.center() - before.center()).len() < 3.0, "{before:?} {after:?}");
        assert!((after.width() - before.width()).abs() < 3.0);
    }

    #[test]
    fn illustrator_outlines_of_our_text_convert_back_to_one_text() {
        // Illustrator's Create Outlines: a group named like the text, one
        // path per letter.
        let spec = TextSpec {
            text: "AB".into(),
            ..TextSpec::default()
        };
        let gid = illustrator_id(&format!("text_AB_fill_{}", spec.id_tokens().join("_")));
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100"><g id="{gid}"><path d="M10 10H20V30H10Z" fill="#000"/><path d="M25 10H35V30H25Z" fill="#000"/></g></svg>"##
        );
        let mut d = Document::from_svg(&svg, "t", None).unwrap();
        assert_eq!(d.shapes.len(), 2);
        assert!(d.records_text(0) && d.records_text(1));
        let k = d.restore_lettering(1).unwrap();
        assert_eq!(d.shapes.len(), 1, "both letters became one text");
        assert_eq!(d.lettering(k).unwrap().text, "AB");
        assert_eq!(d.shapes[k].params.kind.unwrap_or(Kind::Fill), Kind::Fill);
        assert!(!d.xml.contains(&gid), "the empty group went too");
    }

    #[test]
    fn create_outlines_makes_a_plain_shape_with_the_same_stitches() {
        let mut d = Document::empty();
        let i = d
            .add_lettering(
                &TextSpec {
                    text: "AB".into(),
                    ..TextSpec::default()
                },
                (0.0, 0.0),
            )
            .unwrap();
        let before = svg::shapes_bounds(&d.shapes[i..=i]);
        let j = d.expand_lettering(i).unwrap();
        assert!(d.lettering(j).is_none(), "no longer text");
        assert_eq!(d.shapes.len(), 1);
        assert_eq!(d.shapes[j].params.kind, Some(Kind::Satin));
        let after = svg::shapes_bounds(&d.shapes[j..=j]);
        assert!(
            (after.width() - before.width()).abs() < 3.0
                && (after.height() - before.height()).abs() < 3.0,
            "{before:?} vs {after:?}"
        );
        assert!(d.edit_path(j).is_ok(), "editable point by point now");
    }

    #[test]
    fn text_is_edited_point_by_point_like_any_shape() {
        let mut d = Document::empty();
        let i = d
            .add_lettering(
                &TextSpec {
                    text: "AB".into(),
                    ..TextSpec::default()
                },
                (0.0, 0.0),
            )
            .unwrap();
        // Its outline reads as a path; writing one back turns it into plain paths.
        let path = d.edit_path(i).unwrap();
        assert!(path.len() >= 2, "two letters, several rings");
        let before = svg::shapes_bounds(&d.shapes);
        let moved = crate::bezier::transform(&path, crate::geom::translation(P::new(0.0, -50.0)));
        d.set_path(i, &moved).unwrap();
        assert_eq!(d.shapes.len(), 1);
        assert!(d.lettering(0).is_none(), "plain paths now");
        let b = svg::shapes_bounds(&d.shapes);
        assert!(
            (b.min.y - (before.min.y - 50.0)).abs() < 1.0,
            "moved up 5 mm: {before:?} -> {b:?}"
        );
        assert_eq!(d.shapes[0].params.kind, Some(Kind::Satin));
        // Simplify and Pathfinder work on text too.
        let mut e = Document::empty();
        let t = e
            .add_lettering(
                &TextSpec {
                    text: "O".into(),
                    ..TextSpec::default()
                },
                (0.0, 0.0),
            )
            .unwrap();
        let sq = e
            .add_drawing(
                &[
                    P::new(-10.0, -10.0),
                    P::new(80.0, -10.0),
                    P::new(80.0, 80.0),
                    P::new(-10.0, 80.0),
                ],
                true,
                [0, 0, 0],
                Kind::Fill,
            )
            .unwrap();
        e.pathfinder(&[t, sq], crate::boolean::PathOp::Unite)
            .unwrap();
        assert_eq!(e.shapes.len(), 1);
        assert!(e.lettering(0).is_none());
    }

    #[test]
    fn straight_lettering_has_requested_cap_height_and_round_trips() {
        let mut d = Document::empty();
        let spec = TextSpec {
            text: "HAT".into(),
            size_mm: 10.0,
            ..TextSpec::default()
        };
        let i = d.add_lettering(&spec, (0.0, -10.0)).unwrap();
        let b = svg::shapes_bounds(&d.shapes[i..=i]);
        assert!(
            (b.height() - 100.0).abs() < 4.0,
            "10 mm caps: {}",
            b.height()
        );
        assert!(
            (b.center().y + 100.0).abs() < 4.0,
            "centered at y = -10 mm: {}",
            b.center().y
        );
        assert_eq!(d.shapes[i].params.kind, Some(Kind::Satin));
        let back = d.lettering(i).unwrap();
        assert_eq!(back.text, "HAT");
        assert_eq!(back.size_mm, 10.0);
        assert_eq!(back.font, spec.font);

        // Saved and reopened, it's still editable lettering with its stitch type.
        let saved = d.to_svg().unwrap();
        assert!(saved.contains(">HAT</tspan></text>"), "{saved}");
        let e = Document::from_svg(&saved, "t", None).unwrap();
        assert!(e.lettering(0).is_some());
        assert_eq!(e.shapes[0].params.kind, Some(Kind::Satin));
    }

    #[test]
    fn editing_keeps_position_and_arcs_bend_the_baseline() {
        let mut d = Document::empty();
        let spec = TextSpec {
            text: "ABC".into(),
            size_mm: 6.0,
            ..TextSpec::default()
        };
        let i = d.add_lettering(&spec, (5.0, 5.0)).unwrap();
        d.transform(&[i], crate::geom::translation(P::new(100.0, 0.0)))
            .unwrap();
        let before = svg::shapes_bounds(&d.shapes[i..=i]);
        let wide = TextSpec {
            text: "ABCDEF".into(),
            ..spec.clone()
        };
        d.set_lettering(i, &wide).unwrap();
        let after = svg::shapes_bounds(&d.shapes[i..=i]);
        assert!(after.width() > before.width() * 1.6);
        assert!(
            (after.center().x - before.center().x).abs() < 3.0,
            "stays centered where it was moved"
        );
        assert_eq!(d.lettering(i).unwrap().text, "ABCDEF");

        let flat_h = after.height();
        let arched = TextSpec {
            arc_mm: 20.0,
            ..wide.clone()
        };
        d.set_lettering(i, &arched).unwrap();
        let arc_b = svg::shapes_bounds(&d.shapes[i..=i]);
        assert!(
            arc_b.height() > flat_h * 1.3,
            "arched text is taller: {} vs {flat_h}",
            arc_b.height()
        );
        let mut e = Document::empty();
        let k = e
            .add_lettering(
                &TextSpec {
                    arc_mm: -20.0,
                    ..spec.clone()
                },
                (0.0, 30.0),
            )
            .unwrap();
        let c = svg::shapes_bounds(&e.shapes[k..=k]).center();
        assert!(
            c.dist(P::new(0.0, 300.0)) < 1.5,
            "arched lettering is centered where asked: {c:?}"
        );
        assert_eq!(d.lettering(i).unwrap().arc_mm, 20.0);
        let n_paths = d.to_svg().unwrap().matches("tl-arc-").count();
        let again = TextSpec {
            arc_mm: -20.0,
            ..wide
        };
        d.set_lettering(i, &again).unwrap();
        assert_eq!(
            d.to_svg().unwrap().matches("tl-arc-").count(),
            n_paths,
            "arc path replaced, not duplicated"
        );
    }
}
