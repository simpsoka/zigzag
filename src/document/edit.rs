//! Layout edits: move, scale, rotate, mirror, duplicate and copy/paste.
//!
//! Every edit is written into the SVG as a `transform` on the element itself,
//! so the file stays clean artwork that Illustrator opens as you'd expect.

use super::{Document, xml_escape};
use crate::geom::{Affine, IDENTITY, P, affine_apply, affine_inv, affine_mul, translation};
use crate::params::Kind;
use crate::svg::{self, Geometry, PX_TO_UNITS, Part};
use anyhow::{Context, Result, anyhow};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// Copied groups, rebuilt on paste: each node holds fragments and subgroups
/// in first-seen order.
#[derive(Default)]
struct GroupTree {
    id: String,
    items: Vec<TreeItem>,
}

enum TreeItem {
    Fragment(usize),
    Group(GroupTree),
}

impl GroupTree {
    fn insert(&mut self, chain: &[String], k: usize) {
        let Some((first, rest)) = chain.split_first() else {
            self.items.push(TreeItem::Fragment(k));
            return;
        };
        let pos = self
            .items
            .iter()
            .position(|it| matches!(it, TreeItem::Group(g) if &g.id == first));
        let pos = pos.unwrap_or_else(|| {
            self.items.push(TreeItem::Group(GroupTree {
                id: first.clone(),
                items: Vec::new(),
            }));
            self.items.len() - 1
        });
        if let TreeItem::Group(g) = &mut self.items[pos] {
            g.insert(rest, k);
        }
    }

    fn emit(&self, pieces: &[String], used: &mut HashSet<String>, out: &mut String) {
        for it in &self.items {
            match it {
                TreeItem::Fragment(k) => out.push_str(&pieces[*k]),
                TreeItem::Group(g) => {
                    // Same name, no settings (the objects carry their own).
                    let words = svg::name_words(&g.id);
                    let base = if words.is_empty() {
                        "group".to_string()
                    } else {
                        words.join("_")
                    };
                    let mut k = 2;
                    let mut id = format!("{base}_#{k}");
                    while used.contains(&id) {
                        k += 1;
                        id = format!("{base}_#{k}");
                    }
                    used.insert(id.clone());
                    out.push_str(&format!("<g id=\"{}\">\n", xml_escape(&id)));
                    g.emit(pieces, used, out);
                    out.push_str("</g>\n");
                }
            }
        }
    }
}

/// A copied element, ready to paste into this or another document.
#[derive(Clone, Debug)]
pub struct Fragment {
    pub id: String,
    xml: String,
    /// Where the id's value sits in `xml`.
    id_range: Range<usize>,
    /// The element's parent coordinate system → px.
    parent_px: Affine,
    /// The source document's hoop offset (px are relative to it).
    offset: P,
    /// The object's name (it may come from a parent group that isn't copied).
    label: String,
    /// Groups it sits in that were copied whole, outermost first; pasting
    /// rebuilds them so a copied group stays a group.
    groups: Vec<String>,
}

pub(super) fn fmt_matrix(m: Affine) -> String {
    let f = |v: f64| {
        let s = format!("{:.6}", v);
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s == "-0" {
            "0".to_string()
        } else {
            s.to_string()
        }
    };
    format!(
        "matrix({} {} {} {} {} {})",
        f(m[0]),
        f(m[1]),
        f(m[2]),
        f(m[3]),
        f(m[4]),
        f(m[5])
    )
}

fn parse_transform(v: &str) -> Affine {
    match v.parse::<svgtypes::Transform>() {
        Ok(t) => [t.a, t.b, t.c, t.d, t.e, t.f],
        Err(_) => IDENTITY,
    }
}

/// Fill angle after the linear part of `m`, normalized to (-90, 90].
/// Move a shape's points by `m`; a stroke's width scales with it.
fn transform_geometry(g: &mut Geometry, m: Affine) {
    let scale = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
    match g {
        Geometry::Fill { rings, .. } => rings
            .iter_mut()
            .flatten()
            .for_each(|p| *p = affine_apply(m, *p)),
        Geometry::Stroke { lines, width } => {
            lines
                .iter_mut()
                .flat_map(|(l, _)| l)
                .for_each(|p| *p = affine_apply(m, *p));
            *width *= scale;
        }
    }
}

fn turned_angle(deg: f64, m: Affine) -> f64 {
    let (s, c) = deg.to_radians().sin_cos();
    let d = affine_apply([m[0], m[1], m[2], m[3], 0.0, 0.0], P::new(c, s));
    let mut a = d.y.atan2(d.x).to_degrees();
    while a > 90.0 {
        a -= 180.0;
    }
    while a <= -90.0 {
        a += 180.0;
    }
    (a * 10.0).round() / 10.0
}

impl Document {
    /// px → design units (0.1 mm, hoop-centered).
    fn px_to_units(&self) -> Affine {
        [
            PX_TO_UNITS,
            0.0,
            0.0,
            PX_TO_UNITS,
            -self.offset.x,
            -self.offset.y,
        ]
    }

    /// Absolute px transform of each element's parent, measured with probe
    /// paths inserted next to the elements (so usvg resolves everything).
    fn parent_transforms(&self, ids: &[String]) -> Result<HashMap<String, Affine>> {
        let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
        let mut inserts = Vec::new();
        for (k, id) in ids.iter().enumerate() {
            let n = doc
                .descendants()
                .find(|n| n.is_element() && n.attribute("id") == Some(id.as_str()))
                .ok_or_else(|| anyhow!("element {id} not found"))?;
            inserts.push((
                n.range().start,
                format!(r#"<path id="tl-probe-{k}" d="M0 0 L1 0 L0 1 Z" fill="black"/>"#),
            ));
        }
        let mut probe = self.xml.clone();
        inserts.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        for (at, text) in inserts {
            probe.insert_str(at, &text);
        }
        let tree = svg::parse_tree(probe.as_bytes())?;
        let mut out = HashMap::new();
        for (k, id) in ids.iter().enumerate() {
            let node = tree
                .node_by_id(&format!("tl-probe-{k}"))
                .ok_or_else(|| anyhow!("{id} isn't drawn (hidden?)"))?;
            let t = node.abs_transform();
            out.insert(
                id.clone(),
                [
                    t.sx as f64,
                    t.ky as f64,
                    t.kx as f64,
                    t.sy as f64,
                    t.tx as f64,
                    t.ty as f64,
                ],
            );
        }
        Ok(out)
    }

    fn element_ids(&self, indices: &[usize]) -> Vec<String> {
        let mut seen = HashSet::new();
        indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.id.clone())
            .filter(|id| seen.insert(id.clone()))
            .collect()
    }

    /// Transform shapes by `m` (in design units). Fill angles turn with them.
    /// Both parts of an element (fill and outline) move together.
    pub fn transform(&mut self, indices: &[usize], m: Affine) -> Result<()> {
        if m == IDENTITY || indices.is_empty() {
            return Ok(());
        }
        affine_inv(m).ok_or_else(|| anyhow!("that would flatten the object to nothing"))?;
        let uids: HashSet<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        self.sync()?;
        let idx: Vec<usize> = self
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| uids.contains(&s.uid))
            .map(|(i, _)| i)
            .collect();
        let ids = self.element_ids(&idx);
        let affected: HashSet<&str> = ids.iter().map(String::as_str).collect();
        // Every part of a moved element turns its fill angle.
        let angles: Vec<(usize, f64)> = self
            .shapes
            .iter()
            .filter(|s| affected.contains(s.id.as_str()))
            .filter(|s| {
                matches!(s.geometry, Geometry::Fill { .. })
                    && s.params.kind.unwrap_or(Kind::Fill) == Kind::Fill
            })
            .map(|s| (s.uid, s.params.fill_angle()))
            .collect();
        let parents = self.parent_transforms(&ids)?;
        let u = self.px_to_units();
        let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        for id in &ids {
            let n = doc
                .descendants()
                .find(|n| n.is_element() && n.attribute("id") == Some(id.as_str()))
                .expect("found above");
            let up = affine_mul(u, parents[id]);
            let w = affine_mul(
                affine_inv(up).ok_or_else(|| anyhow!("{id} has a degenerate transform"))?,
                affine_mul(m, up),
            );
            match n.attributes().find(|a| a.name() == "transform") {
                Some(a) => edits.push((
                    a.range_value(),
                    fmt_matrix(affine_mul(w, parse_transform(a.value()))),
                )),
                None => {
                    let at = n.range().start
                        + 1
                        + n.tag_name().name().len()
                        + super::root_prefix_len(&self.xml, n.range().start);
                    edits.push((at..at, format!(" transform=\"{}\"", fmt_matrix(w))));
                }
            }
        }
        drop(doc);
        edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
        for (r, t) in edits {
            self.xml.replace_range(r, &t);
        }
        let map = self.uid_map(&HashMap::new());
        self.import(&map)?;
        for (uid, a) in angles {
            if let Some(s) = self.shapes.iter_mut().find(|s| s.uid == uid) {
                let b = turned_angle(a, m);
                if (b - a).abs() > 0.05 {
                    s.params.angle = Some(b);
                }
            }
        }
        Ok(())
    }

    /// Move geometry only (no SVG edit): a cheap live preview while dragging.
    pub fn preview_transform(&mut self, indices: &[usize], m: Affine) {
        let ids: HashSet<String> = self.element_ids(indices).into_iter().collect();
        for s in self.shapes.iter_mut().filter(|s| ids.contains(&s.id)) {
            transform_geometry(&mut s.geometry, m);
        }
    }

    /// [`Self::preview_transform`] for a gesture in progress, frame after
    /// frame: the moved shapes' geometry is taken from `base` (the document
    /// as it was when the gesture began) and transformed by `m`; nothing
    /// else is touched or cloned. `self` must be `base` as left by an
    /// earlier call (or `base` itself); otherwise it becomes one.
    pub fn preview_transform_from(&mut self, base: &Document, indices: &[usize], m: Affine) {
        let same = self.shapes.len() == base.shapes.len()
            && self
                .shapes
                .iter()
                .zip(&base.shapes)
                .all(|(a, b)| a.uid == b.uid && a.id == b.id);
        if !same {
            *self = base.clone();
            self.preview_transform(indices, m);
            return;
        }
        let ids: HashSet<String> = base.element_ids(indices).into_iter().collect();
        for (s, b) in self.shapes.iter_mut().zip(&base.shapes) {
            if ids.contains(&s.id) {
                s.geometry = b.geometry.clone();
                transform_geometry(&mut s.geometry, m);
            }
        }
    }

    /// Copy elements (with everything needed to paste them elsewhere).
    pub fn copy(&mut self, indices: &[usize]) -> Result<Vec<Fragment>> {
        let uids: HashSet<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        self.sync()?;
        let idx: Vec<usize> = self
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| uids.contains(&s.uid))
            .map(|(i, _)| i)
            .collect();
        let ids = self.element_ids(&idx);
        let parents = self.parent_transforms(&ids)?;
        let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
        // Groups every member of which is being copied.
        let picked: HashSet<usize> = idx.iter().copied().collect();
        let whole = |g: &String| {
            self.shapes
                .iter()
                .enumerate()
                .filter(|(_, s)| s.groups.contains(g))
                .all(|(i, _)| picked.contains(&i))
        };
        let mut out = Vec::new();
        for id in ids {
            let n = doc
                .descendants()
                .find(|n| n.is_element() && n.attribute("id") == Some(id.as_str()))
                .expect("found above");
            let r = n.range();
            let a = n
                .attributes()
                .find(|a| a.name() == "id")
                .expect("has id")
                .range_value();
            out.push(Fragment {
                xml: self.xml[r.clone()].to_string(),
                id_range: a.start - r.start..a.end - r.start,
                parent_px: parents[&id],
                offset: self.offset,
                label: self
                    .shapes
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.label.clone())
                    .unwrap_or_default(),
                groups: self
                    .shapes
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.groups.iter().filter(|g| whole(g)).cloned().collect())
                    .unwrap_or_default(),
                id,
            });
        }
        Ok(out)
    }

    /// Paste copied elements, shifted by `offset` (design units). They sew
    /// last. Returns the new shapes' indices.
    pub fn paste(&mut self, fragments: &[Fragment], offset: P) -> Result<Vec<usize>> {
        if fragments.is_empty() {
            return Ok(Vec::new());
        }
        self.sync()?;
        let r_inv = affine_inv(self.root_transform()?)
            .ok_or_else(|| anyhow!("document root transform isn't invertible"))?;
        let mut used: HashSet<String> = {
            let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
            doc.descendants()
                .filter_map(|n| n.attribute("id").map(str::to_string))
                .collect()
        };
        // Rebuild copied groups: fragments nest under fresh group ids, in
        // the order their groups first appear.
        let mut tree = GroupTree::default();
        for (k, f) in fragments.iter().enumerate() {
            tree.insert(&f.groups, k);
        }
        let mut pieces: Vec<String> = Vec::with_capacity(fragments.len());
        for f in fragments {
            let mut base =
                f.id.rsplit_once("_#")
                    .map(|(b, _)| b)
                    .unwrap_or(&f.id)
                    .to_string();
            if svg::name_words(&base).is_empty() {
                // Named by a parent group: carry the name over with the settings.
                let decoded = crate::params::decode_illustrator_id(&base);
                let mut parts = svg::name_words(&f.label);
                parts.extend(
                    decoded
                        .split('_')
                        .filter(|t| crate::params::is_param_token(t))
                        .map(str::to_string),
                );
                if !parts.is_empty() {
                    base = parts.join("_");
                }
            }
            let mut k = 2;
            let mut new_id = format!("{base}_#{k}");
            while used.contains(&new_id) {
                k += 1;
                new_id = format!("{base}_#{k}");
            }
            used.insert(new_id.clone());
            let mut x = f.xml.clone();
            x.replace_range(f.id_range.clone(), &xml_escape(&new_id));
            self.next_group += 1;
            // Same place on the hoop as the original, plus `offset`.
            let d = offset + self.offset - f.offset;
            let shift = translation(P::new(d.x / PX_TO_UNITS, d.y / PX_TO_UNITS));
            let g = affine_mul(r_inv, affine_mul(shift, f.parent_px));
            pieces.push(format!(
                "<g id=\"tl-paste-{}\" transform=\"{}\">{x}</g>\n",
                self.next_group,
                fmt_matrix(g)
            ));
        }
        let mut body = String::new();
        tree.emit(&pieces, &mut used, &mut body);
        let close = self
            .xml
            .rfind("</svg>")
            .ok_or_else(|| anyhow!("document has no closing </svg>"))?;
        self.xml.insert_str(close, &body);
        let before: HashSet<(String, Part)> =
            self.shapes.iter().map(|s| (s.id.clone(), s.part)).collect();
        let (xml, _) = super::prepare_ids(&self.xml, &mut self.next_group)?;
        self.xml = xml;
        let map = self.uid_map(&HashMap::new());
        self.import(&map)?;
        Ok(self
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| !before.contains(&(s.id.clone(), s.part)))
            .map(|(i, _)| i)
            .collect())
    }

    /// Duplicate shapes, offset by `offset`. Returns the copies' indices.
    pub fn duplicate(&mut self, indices: &[usize], offset: P) -> Result<Vec<usize>> {
        let frags = self.copy(indices)?;
        self.paste(&frags, offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{rotate_about, scale_about};

    const ART: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="40mm" viewBox="0 0 60 40">
      <g transform="translate(5 5)">
        <rect id="box_fill_a=30" x="0" y="0" width="20" height="10" fill="#123456"/>
      </g>
      <circle id="dot" cx="45" cy="25" r="5" fill="#ff0000" stroke="#000" stroke-width="1"/>
    </svg>"##;

    fn bounds(d: &Document, i: usize) -> crate::geom::Bounds {
        svg::shapes_bounds(&d.shapes[i..=i])
    }

    fn reopen(d: &Document) -> Document {
        let mut e = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        e.offset = d.offset;
        let m = HashMap::new();
        e.import(&m).unwrap();
        e
    }

    #[test]
    fn move_scale_rotate_persist_in_svg() {
        let mut d = Document::from_svg(ART, "t", None).unwrap();
        let b0 = bounds(&d, 0);
        d.transform(&[0], translation(P::new(30.0, -20.0))).unwrap();
        let b1 = bounds(&d, 0);
        assert!(
            (b1.min.x - b0.min.x - 30.0).abs() < 0.01 && (b1.min.y - b0.min.y + 20.0).abs() < 0.01
        );
        assert_eq!(
            d.shapes[0].params.angle,
            Some(30.0),
            "moving keeps the angle"
        );

        d.transform(&[0], scale_about(b1.center(), 2.0, 2.0))
            .unwrap();
        let b2 = bounds(&d, 0);
        assert!(
            (b2.width() - 2.0 * b1.width()).abs() < 0.05,
            "{} vs {}",
            b2.width(),
            b1.width()
        );

        d.transform(&[0], rotate_about(b2.center(), 90.0)).unwrap();
        let b3 = bounds(&d, 0);
        assert!((b3.width() - b2.height()).abs() < 0.05 && (b3.height() - b2.width()).abs() < 0.05);
        assert_eq!(
            d.shapes[0].params.angle,
            Some(-60.0),
            "fill angle turns with the object (30° + 90°)"
        );

        let e = reopen(&d);
        let be = bounds(&e, 0);
        assert!(
            (be.center().x - b3.center().x).abs() < 0.05 && (be.width() - b3.width()).abs() < 0.05,
            "saved in the SVG"
        );
        assert_eq!(e.shapes[0].params.angle, Some(-60.0));
        assert!(
            d.to_svg().unwrap().contains("transform=\"translate(5 5)\""),
            "parent group untouched"
        );
    }

    #[test]
    fn drawn_paths_become_objects() {
        let mut d = Document::empty();
        let tri = [P::new(0.0, 0.0), P::new(100.0, 0.0), P::new(50.0, 80.0)];
        let i = d.add_drawing(&tri, true, [9, 9, 9], Kind::Fill).unwrap();
        let b = bounds(&d, i);
        assert!((b.width() - 100.0).abs() < 0.5 && (b.height() - 80.0).abs() < 0.5);
        assert_eq!(d.shapes[i].params.kind, Some(Kind::Fill));
        let j = d
            .add_drawing(&tri[..2], false, [9, 9, 9], Kind::Manual)
            .unwrap();
        assert_eq!(d.shapes[j].params.kind, Some(Kind::Manual));
        assert!(matches!(d.shapes[j].geometry, Geometry::Stroke { .. }));
        assert_eq!(
            svg::display_names(&d.shapes),
            vec!["drawing 1", "drawing 2"]
        );
    }

    #[test]
    fn mirror_and_parts_move_together() {
        let mut d = Document::from_svg(ART, "t", None).unwrap();
        let dot: Vec<usize> = (0..d.shapes.len())
            .filter(|&i| d.shapes[i].id == "dot")
            .collect();
        assert_eq!(dot.len(), 2, "fill + stroke");
        let c = bounds(&d, dot[0]).center();
        d.transform(&dot[..1], translation(P::new(-50.0, 0.0)))
            .unwrap();
        assert!(
            (bounds(&d, dot[1]).center().x - (c.x - 50.0)).abs() < 0.05,
            "outline follows its fill"
        );
        let w0 = match d.shapes[dot[1]].geometry {
            Geometry::Stroke { width, .. } => width,
            _ => 0.0,
        };
        d.transform(&dot[..1], scale_about(P::new(0.0, 0.0), -1.0, 1.0))
            .unwrap();
        assert!(
            (bounds(&d, dot[0]).center().x - (50.0 - c.x)).abs() < 0.05,
            "mirrored across the hoop center"
        );
        let w1 = match d.shapes[dot[1]].geometry {
            Geometry::Stroke { width, .. } => width,
            _ => 0.0,
        };
        assert!((w0 - w1).abs() < 1e-6);
    }

    #[test]
    fn duplicate_and_paste_keep_settings_and_place_copies() {
        let mut d = Document::from_svg(ART, "t", None).unwrap();
        d.shapes[0].params.spacing = Some(0.3);
        d.shapes[0].color = [1, 2, 3];
        let b = bounds(&d, 0);
        let new = d.duplicate(&[0], P::new(50.0, 50.0)).unwrap();
        assert_eq!(new.len(), 1);
        let c = &d.shapes[new[0]];
        assert_eq!(svg::name_words(&c.id), vec!["box"]);
        assert_eq!(c.params.spacing, Some(0.3));
        assert_eq!(c.params.angle, Some(30.0));
        assert_eq!(c.color, [1, 2, 3]);
        let bc = bounds(&d, new[0]);
        assert!(
            (bc.min.x - b.min.x - 50.0).abs() < 0.05 && (bc.min.y - b.min.y - 50.0).abs() < 0.05
        );
        assert_eq!(svg::display_names(&d.shapes)[new[0]], "box (2)");

        // Paste into another document at the same place on the hoop.
        let frags = d.copy(&[0]).unwrap();
        let mut other = Document::empty();
        let got = other.paste(&frags, P::default()).unwrap();
        assert_eq!(got.len(), 1);
        let bo = svg::shapes_bounds(&other.shapes[got[0]..=got[0]]);
        assert!(
            (bo.center().x - b.center().x).abs() < 0.1 && (bo.width() - b.width()).abs() < 0.1,
            "{bo:?} vs {b:?}"
        );
    }
}
