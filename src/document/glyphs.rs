//! Where each letter of a lettering object sits, from the layout usvg made
//! when it set the text. The Type tool uses this to put the caret between
//! letters (a click, the arrow keys) and to draw it the height of the font,
//! following the baseline on arcs and paths.

use super::Document;
use crate::geom::P;
use crate::svg::PX_TO_UNITS;

/// One glyph of a text object: which characters of the text it shows and
/// the caret lines (top, bottom; design units) just before and after it.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBox {
    /// Character indices (into the text as typed, newlines included).
    pub chars: std::ops::Range<usize>,
    pub left: (P, P),
    pub right: (P, P),
}

impl GlyphBox {
    pub fn left_mid(&self) -> P {
        (self.left.0 + self.left.1) * 0.5
    }
    pub fn right_mid(&self) -> P {
        (self.right.0 + self.right.1) * 0.5
    }
}

/// The caret lines for every position in a text, 0..=chars.
#[derive(Clone, Debug, Default)]
pub struct Carets {
    pub at: Vec<(P, P)>,
}

impl Carets {
    /// The position nearest a point, preferring the line clicked (vertical
    /// distance counts three times).
    pub fn nearest(&self, p: P) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (k, (a, b)) in self.at.iter().enumerate() {
            let m = (*a + *b) * 0.5;
            let d = (m.x - p.x).powi(2) + (3.0 * (m.y - p.y)).powi(2);
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((k, d));
            }
        }
        best.map(|(k, _)| k)
    }
}

fn find_text<'a>(g: &'a usvg::Group, id: &str) -> Option<&'a usvg::Text> {
    for node in g.children() {
        match node {
            usvg::Node::Text(t) if t.id() == id => return Some(t),
            usvg::Node::Group(g) => {
                if let Some(t) = find_text(g, id) {
                    return Some(t);
                }
            }
            _ => {}
        }
    }
    None
}

impl Document {
    /// The glyphs of lettering object `index`, in text order, or empty if
    /// it isn't lettering (or nothing was laid out).
    pub fn glyph_boxes(&self, index: usize) -> Vec<GlyphBox> {
        let Some(spec) = self.lettering(index) else {
            return Vec::new();
        };
        let Some(tree) = &self.tree else {
            return Vec::new();
        };
        let id = &self.shapes[index].id;
        let Some(text) = find_text(tree.root(), id) else {
            return Vec::new();
        };
        let base = text.abs_transform();
        let offset = self.offset;
        let db = tree.fontdb();
        let chars: Vec<char> = spec.text.chars().collect();
        let mut pos = 0usize;
        let mut out = Vec::new();
        for span in text.layouted() {
            for g in &span.positioned_glyphs {
                // Font metrics: the advance of this glyph and the font's
                // ascent/descent, in font units.
                let Some((adv, asc, desc)) = db
                    .with_face_data(g.font, |data, idx| {
                        let face = ttf_parser::Face::parse(data, idx).ok()?;
                        let gid = ttf_parser::GlyphId(g.id.0);
                        Some((
                            face.glyph_hor_advance(gid).unwrap_or(0) as f32,
                            face.ascender() as f32,
                            face.descender() as f32,
                        ))
                    })
                    .flatten()
                else {
                    continue;
                };
                // transform() maps font units (y down) to the text's space;
                // the glyph's own units_per_em scale is inside it.
                let ts = base.pre_concat(g.transform());
                let map = |x: f32, y: f32| {
                    let (x, y) = (x as f64, y as f64);
                    let nx = ts.sx as f64 * x + ts.kx as f64 * y + ts.tx as f64;
                    let ny = ts.ky as f64 * x + ts.sy as f64 * y + ts.ty as f64;
                    P::new(nx * PX_TO_UNITS, ny * PX_TO_UNITS) - offset
                };
                let left = (map(0.0, -asc), map(0.0, -desc));
                let right = (map(adv, -asc), map(adv, -desc));
                // Which characters this glyph shows: the cluster text, found
                // from where the last one left off (whitespace the layout
                // collapsed is skipped over).
                let cluster: Vec<char> = g.text.chars().collect();
                let n = cluster.len().max(1);
                let mut at = pos;
                let found = loop {
                    if at + cluster.len() > chars.len() {
                        break None;
                    }
                    if !cluster.is_empty() && chars[at..at + cluster.len()] == cluster[..] {
                        break Some(at);
                    }
                    if at < chars.len() && (chars[at].is_whitespace() || chars[at] == '\n') {
                        at += 1;
                        continue;
                    }
                    break None;
                };
                let start = found.unwrap_or(pos);
                let end = (start + n).min(chars.len().max(start + 1));
                pos = end;
                out.push(GlyphBox {
                    chars: start..end,
                    left,
                    right,
                });
            }
        }
        out
    }

    /// Caret lines for every position of lettering object `index`, from
    /// before the first character to after the last. Positions with no
    /// glyph of their own (a newline, a collapsed space) take the end of
    /// the letter before them.
    pub fn carets(&self, index: usize) -> Carets {
        let Some(spec) = self.lettering(index) else {
            return Carets::default();
        };
        let n = spec.text.chars().count();
        let boxes = self.glyph_boxes(index);
        if boxes.is_empty() {
            return Carets::default();
        }
        let mut at: Vec<Option<(P, P)>> = vec![None; n + 1];
        for b in &boxes {
            if b.chars.start <= n && at[b.chars.start].is_none() {
                at[b.chars.start] = Some(b.left);
            }
            if b.chars.end <= n && at[b.chars.end].is_none() {
                at[b.chars.end] = Some(b.right);
            }
        }
        // Fill the gaps from the nearest known caret before (or after).
        let mut last: Option<(P, P)> = None;
        for c in at.iter_mut() {
            match c {
                Some(v) => last = Some(*v),
                None => *c = last,
            }
        }
        let mut next: Option<(P, P)> = None;
        for c in at.iter_mut().rev() {
            match c {
                Some(v) => next = Some(*v),
                None => *c = next,
            }
        }
        Carets {
            at: at.into_iter().flatten().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::TextSpec;

    fn doc_with(text: &str, spec: TextSpec) -> (Document, usize) {
        let mut d = Document::empty();
        let spec = TextSpec {
            text: text.into(),
            ..spec
        };
        let i = d.add_lettering(&spec, (0.0, 0.0)).unwrap();
        (d, i)
    }

    #[test]
    fn glyphs_cover_the_characters_in_order_and_sit_on_the_letters() {
        let (d, i) = doc_with("CAMP", TextSpec::default());
        let g = d.glyph_boxes(i);
        assert_eq!(g.len(), 4);
        let ranges: Vec<_> = g.iter().map(|b| b.chars.clone()).collect();
        assert_eq!(ranges, vec![0..1, 1..2, 2..3, 3..4]);
        // Left to right, each glyph's right edge is at or before the next's left.
        for w in g.windows(2) {
            assert!(w[0].right_mid().x <= w[1].left_mid().x + 1.0);
        }
        // The first glyph starts about where the outlines start, and the
        // caret is at least as tall as the capitals (8 mm).
        let b = crate::svg::shapes_bounds(&d.shapes[i..=i]);
        assert!((g[0].left_mid().x - b.min.x).abs() < 15.0, "{g:?} vs {b:?}");
        assert!((g[0].left.1.y - g[0].left.0.y) >= 80.0);
        // The carets: 5 positions, increasing in x.
        let c = d.carets(i);
        assert_eq!(c.at.len(), 5);
        for w in c.at.windows(2) {
            assert!(w[0].0.x < w[1].0.x);
        }
        // Clicking in the right half of the A lands after it.
        let a = &g[1];
        let click = P::new(a.left_mid().x * 0.3 + a.right_mid().x * 0.7, a.left_mid().y);
        assert_eq!(c.nearest(click), Some(2));
    }

    #[test]
    fn spaces_and_lines_keep_the_character_count() {
        let (d, i) = doc_with("AB CD\nEF", TextSpec::default());
        let c = d.carets(i);
        assert_eq!(c.at.len(), 9);
        // The second line is below the first.
        assert!(c.at[6].0.y > c.at[0].0.y);
        // Clicking near the start of the second line goes there.
        let p = c.at[6].0 + P::new(-5.0, 40.0);
        assert_eq!(c.nearest(p), Some(6));
    }

    #[test]
    fn carets_follow_an_arc() {
        let spec = TextSpec {
            arc_mm: 30.0,
            ..TextSpec::default()
        };
        let (d, i) = doc_with("ARC TEXT", spec);
        let c = d.carets(i);
        assert_eq!(c.at.len(), 9);
        // The outer carets tilt: not vertical.
        let (a, b) = c.at[0];
        assert!((a.x - b.x).abs() > 5.0, "{a:?} {b:?}");
    }
}
