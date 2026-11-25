//! Groups made and unmade: objects put into a group, taken out of one,
//! and grouped anew.
//!
//! A group is a `<g>` in the SVG, so each of these moves elements from
//! one place in the file to another. An element moved keeps its place on
//! the cloth and its look: the difference between where it was drawn from
//! and where it's drawn from now goes into its own `transform`, and what it
//! took from the groups it leaves (a fill, a stroke) is written on it. It
//! keeps its place in the sewing order too, which is in its id.
//!
//! (After VectorCraft's `layer.move`, which puts rows above, below or into
//! a group. There the document is a tree and a node is moved in it; here
//! it's the file's text.)

use super::Document;
use super::edit::{fmt_matrix, parse_transform};
use crate::geom::{IDENTITY, affine_inv, affine_mul};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// What an element takes from the groups it's in when it doesn't say
/// itself, and what each is when nothing says.
const INHERITED: [(&str, &str); 8] = [
    ("fill", "#000000"),
    ("stroke", "none"),
    ("stroke-width", "1"),
    ("stroke-linecap", "butt"),
    ("stroke-linejoin", "miter"),
    ("stroke-miterlimit", "4"),
    ("stroke-dasharray", "none"),
    ("fill-rule", "nonzero"),
];

/// `name` as `n` itself says it: an attribute, or in its `style`.
fn own(n: roxmltree::Node, name: &str) -> Option<String> {
    let styled = n.attribute("style").and_then(|s| {
        s.split(';').find_map(|d| {
            let (k, v) = d.split_once(':')?;
            (k.trim() == name).then(|| v.trim().to_string())
        })
    });
    styled.or_else(|| n.attribute(name).map(str::to_string))
}

/// `name` as `n` and the elements it's in say it, nearest first.
fn said(n: roxmltree::Node, name: &str) -> Option<String> {
    n.ancestors()
        .filter(|a| a.is_element())
        .find_map(|a| own(a, name))
}

fn is_group(n: roxmltree::Node) -> bool {
    n.is_element() && n.tag_name().name() == "g"
}

fn element_children<'a, 'i>(
    n: roxmltree::Node<'a, 'i>,
) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> {
    n.children().filter(|c| c.is_element())
}

/// What moves when the element `n` does: itself, or the group that holds
/// only it (ZigZag's own wrapper round lettering or a paste, Illustrator's
/// round each object, with the object's name and settings on it), as far
/// out as that goes short of `stop`.
fn unit<'a, 'i>(
    mut n: roxmltree::Node<'a, 'i>,
    stop: roxmltree::Node<'a, 'i>,
) -> roxmltree::Node<'a, 'i> {
    while let Some(p) = n.parent_element() {
        if p == stop || !is_group(p) || element_children(p).count() != 1 {
            break;
        }
        n = p;
    }
    n
}

/// Where `n`'s start tag ends its name: where a new attribute goes.
fn after_name(text: &str) -> usize {
    1 + text[1..]
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(0)
}

/// `range`, with the white space after it up to and including a line end.
fn with_line_end(xml: &str, range: Range<usize>) -> Range<usize> {
    let rest = &xml[range.end..];
    let blank = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let end = match &rest[blank..] {
        r if r.starts_with("\r\n") => blank + 2,
        r if r.starts_with('\n') => blank + 1,
        _ => 0,
    };
    range.start..range.end + end
}

/// Where what's put into `n` goes (the end of what's in it), and what to
/// put round it there: nothing, or for an element written `<g/>` the rest
/// of its start tag and an end tag.
fn inside_end(xml: &str, n: roxmltree::Node) -> (Range<usize>, String, String) {
    let r = n.range();
    let text = &xml[r.clone()];
    match text.rfind("</") {
        Some(at) if !text.ends_with("/>") => {
            (r.start + at..r.start + at, String::new(), String::new())
        }
        _ => {
            let name = &text[1..after_name(text)];
            let at = r.end - 2;
            (at..r.end, ">\n".to_string(), format!("</{name}>"))
        }
    }
}

impl Document {
    /// Put the objects at `indices` into the group `into` (an element's
    /// id), or with `None` out of every group, at the top of the file. Each
    /// stays where it is on the cloth, as it looks, and where it is in the
    /// sewing order. False when they're all there already.
    pub fn regroup(&mut self, indices: &[usize], into: Option<&str>) -> Result<bool> {
        let uids: HashSet<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        if uids.is_empty() {
            return Ok(false);
        }
        self.sync()?;
        let idx: Vec<usize> = (0..self.shapes.len())
            .filter(|&i| uids.contains(&self.shapes[i].uid))
            .collect();
        let ids = self.element_ids(&idx);
        let edits = {
            let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
            let by_id = super::elements_by_id(&doc);
            let target = match into {
                None => doc.root_element(),
                Some(g) => *by_id
                    .get(g)
                    .filter(|n| is_group(**n))
                    .ok_or_else(|| anyhow!("there's no group {g}"))?,
            };
            // What moves: each element, or the wrapper that's only its.
            let mut seen = HashSet::new();
            let mut units = Vec::new();
            for id in &ids {
                let n = *by_id
                    .get(id.as_str())
                    .ok_or_else(|| anyhow!("element {id} not found"))?;
                let u = unit(n, target);
                if u.parent_element() == Some(target) || !seen.insert(u.range().start) {
                    continue;
                }
                if target.ancestors().any(|a| a == u) {
                    bail!("a group can't be put inside itself");
                }
                let Some(uid) = u.attribute("id") else {
                    bail!("{id} is in a group with no id");
                };
                units.push((u, uid.to_string()));
            }
            if units.is_empty() {
                return Ok(false);
            }
            // Where each is drawn from now, and where the group draws from.
            let unit_ids: Vec<String> = units.iter().map(|(_, id)| id.clone()).collect();
            let from = self.parent_transforms(&by_id, &unit_ids)?;
            let to = match into {
                None => self.root_transform()?,
                Some(g) => affine_mul(
                    self.parent_transforms(&by_id, &[g.to_string()])?[g],
                    target
                        .attribute("transform")
                        .map_or(IDENTITY, parse_transform),
                ),
            };
            let back =
                affine_inv(to).ok_or_else(|| anyhow!("that group has a degenerate transform"))?;
            let mut edits: Vec<(Range<usize>, String)> = Vec::new();
            let mut moved = String::new();
            // The groups left, innermost first: emptied, they go.
            let mut left: Vec<String> = Vec::new();
            for (u, uid) in &units {
                let mut text = self.xml[u.range()].to_string();
                let at = after_name(&text);
                let mut add = String::new();
                // What it took from the groups it leaves, that the one it
                // joins doesn't give it.
                for (name, unsaid) in INHERITED {
                    if own(*u, name).is_some() {
                        continue;
                    }
                    let was = u.parent_element().and_then(|p| said(p, name));
                    let now = said(target, name);
                    if was != now {
                        add.push_str(&format!(
                            " {name}=\"{}\"",
                            super::xml_escape(was.as_deref().unwrap_or(unsaid))
                        ));
                    }
                }
                let w = affine_mul(back, from[uid]);
                let same = w
                    .iter()
                    .zip(IDENTITY)
                    .all(|(a, b)| (a - b).abs() < 1e-9);
                match u.attributes().find(|a| a.name() == "transform") {
                    _ if same => {}
                    Some(a) => {
                        let r = a.range_value();
                        let (s, e) = (r.start - u.range().start, r.end - u.range().start);
                        text.replace_range(
                            s..e,
                            &fmt_matrix(affine_mul(w, parse_transform(a.value()))),
                        );
                    }
                    None => add.push_str(&format!(" transform=\"{}\"", fmt_matrix(w))),
                }
                text.insert_str(at, &add);
                moved.push_str(&text);
                moved.push('\n');
                edits.push((with_line_end(&self.xml, u.range()), String::new()));
                left.extend(
                    u.ancestors()
                        .skip(1)
                        .filter(|a| is_group(*a) && *a != target)
                        .filter_map(|a| a.attribute("id").map(str::to_string)),
                );
            }
            let (at, open, close) = inside_end(&self.xml, target);
            edits.push((at, format!("{open}{moved}{close}")));
            (edits, left)
        };
        let (mut edits, left) = edits;
        // (From the end of the file back, so the places before stay put;
        // what's put in at a place before what's taken out from it.)
        edits.sort_by_key(|(r, _)| (std::cmp::Reverse(r.start), std::cmp::Reverse(r.end)));
        for (r, t) in edits {
            self.xml.replace_range(r, &t);
        }
        self.drop_emptied(&left)?;
        self.read_again()?;
        Ok(true)
    }

    /// Read the objects from the file again, sewn in the order they were.
    /// (As read, they come in the file's order where their ids don't say
    /// another.)
    pub(super) fn read_again(&mut self) -> Result<()> {
        let order: HashMap<usize, usize> = self
            .shapes
            .iter()
            .enumerate()
            .map(|(k, s)| (s.uid, k))
            .collect();
        let map = self.uid_map(&HashMap::new());
        self.import(&map)?;
        self.shapes
            .sort_by_key(|s| order.get(&s.uid).copied().unwrap_or(usize::MAX));
        Ok(())
    }

    /// Give the group `id` a name. Its id takes the name's letters, digits
    /// and spaces (a word that's a stitch setting would be taken for one,
    /// and is left out); the name itself is kept whole beside it, which is
    /// what the group is called. Returns the id it has then.
    pub fn rename_group(&mut self, id: &str, name: &str) -> Result<String> {
        let plain: String = name
            .trim()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        let words = crate::svg::name_words(&plain);
        if words.is_empty() {
            bail!("a group's name needs a word that isn't a stitch setting");
        }
        self.sync()?;
        let old = format!("id=\"{}\"", super::xml_escape(id));
        if !self.xml.contains(&old) {
            bail!("there's no group {id}");
        }
        let base = words.join("_");
        let (mut new, mut k) = (base.clone(), 2);
        while self.xml.contains(&format!("id=\"{new}\"")) {
            new = format!("{base}_#{k}");
            k += 1;
        }
        self.xml = self.xml.replacen(&old, &format!("id=\"{new}\""), 1);
        self.xml = super::with_group_name(&self.xml, &new, name.trim())?;
        self.read_again()?;
        Ok(new)
    }

    /// Take the groups with these ids out of the file where nothing is left
    /// in them (and then the groups that leaves empty).
    fn drop_emptied(&mut self, ids: &[String]) -> Result<()> {
        loop {
            let gone = {
                let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
                let by_id = super::elements_by_id(&doc);
                ids.iter()
                    .filter_map(|id| by_id.get(id.as_str()))
                    .find(|n| is_group(**n) && element_children(**n).next().is_none())
                    .map(|n| with_line_end(&self.xml, n.range()))
            };
            match gone {
                Some(r) => self.xml.replace_range(r, ""),
                None => return Ok(()),
            }
        }
    }

    /// Make a group of the objects at `indices`: inside the group they're
    /// all in, if there is one, else at the top. Returns its id.
    pub fn group(&mut self, indices: &[usize]) -> Result<String> {
        let uids: HashSet<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        if uids.is_empty() {
            bail!("select something to group");
        }
        self.sync()?;
        let of: Vec<&crate::svg::Shape> = self
            .shapes
            .iter()
            .filter(|s| uids.contains(&s.uid))
            .collect();
        // The groups every one of them is in, outermost first.
        let mut shared: Vec<String> = of[0].groups.clone();
        for s in &of[1..] {
            let n = shared
                .iter()
                .zip(&s.groups)
                .take_while(|(a, b)| a == b)
                .count();
            shared.truncate(n);
        }
        let id = loop {
            self.next_group += 1;
            let id = format!("tl-{}", self.next_group);
            if !self.xml.contains(&format!("id=\"{id}\"")) {
                break id;
            }
        };
        let (at, open, close) = {
            let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
            let by_id = super::elements_by_id(&doc);
            let parent = match shared.last() {
                Some(g) => *by_id
                    .get(g.as_str())
                    .ok_or_else(|| anyhow!("there's no group {g}"))?,
                None => doc.root_element(),
            };
            inside_end(&self.xml, parent)
        };
        self.xml
            .replace_range(at, &format!("{open}<g id=\"{id}\"></g>\n{close}"));
        let idx: Vec<usize> = (0..self.shapes.len())
            .filter(|&i| uids.contains(&self.shapes[i].uid))
            .collect();
        self.regroup(&idx, Some(&id))?;
        Ok(id)
    }

    /// Take the objects at `indices` out of the group each is in, into the
    /// one that group is in (or to the top). A group left with nothing in
    /// it goes. How many came out.
    pub fn ungroup(&mut self, indices: &[usize]) -> Result<usize> {
        let uids: Vec<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        self.sync()?;
        // Where each goes: the group its group is in.
        let mut to: Vec<(Option<String>, Vec<usize>)> = Vec::new();
        {
            let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
            let by_id = super::elements_by_id(&doc);
            for s in self.shapes.iter().filter(|s| uids.contains(&s.uid)) {
                let Some(n) = by_id.get(s.id.as_str()) else {
                    continue;
                };
                let u = unit(*n, doc.root_element());
                let Some(group) = u.parent_element().filter(|p| is_group(*p)) else {
                    continue;
                };
                // (Out past ZigZag's own wrappers, which aren't groups
                // anyone sees: an object taken out of the only group it
                // shows in is out of every group.)
                let outer = group
                    .ancestors()
                    .skip(1)
                    .filter(|a| is_group(*a))
                    .find(|a| !a.attribute("id").is_some_and(crate::svg::is_wrapper_group))
                    .and_then(|a| a.attribute("id"))
                    .map(str::to_string);
                if crate::svg::is_wrapper_group(group.attribute("id").unwrap_or("")) {
                    continue;
                }
                match to.iter_mut().find(|(g, _)| *g == outer) {
                    Some((_, uids)) => uids.push(s.uid),
                    None => to.push((outer, vec![s.uid])),
                }
            }
        }
        let mut out = 0;
        for (group, uids) in to {
            let idx: Vec<usize> = (0..self.shapes.len())
                .filter(|&i| uids.contains(&self.shapes[i].uid))
                .collect();
            if self.regroup(&idx, group.as_deref())? {
                out += idx.len();
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::P;
    use crate::svg::Geometry;

    const ART: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40">
      <g id="tl-added-1" transform="translate(4 2) scale(2)">
        <g id="set" fill="#c00000" stroke="#000080" stroke-width="0.5">
          <rect id="a_fill_a=30" x="1" y="1" width="6" height="4"/>
          <rect id="b" x="10" y="1" width="6" height="4" fill="#00a000"/>
        </g>
      </g>
      <g id="pair" transform="rotate(10)">
        <rect id="c" x="40" y="5" width="8" height="8" fill="#123456"/>
        <rect id="d" x="52" y="5" width="8" height="8" fill="#123456"/>
      </g>
      <rect id="e" x="66" y="20" width="8" height="8" fill="#654321"/>
    </svg>"##;

    /// The first word of an object's id: `a`, `b`…
    fn name_of(s: &crate::svg::Shape) -> &str {
        s.id.split('_').next().unwrap_or("")
    }

    fn doc() -> Document {
        Document::from_svg(ART, "t", None).unwrap()
    }

    /// Where each object is, how it looks and where it's sewn, by its name:
    /// its first point, its thread, its place in the order.
    fn look(d: &Document) -> Vec<(String, (i64, i64), [u8; 3], usize, Option<f64>)> {
        let mut out: Vec<_> = d
            .shapes
            .iter()
            .enumerate()
            .map(|(k, s)| {
                let p = match &s.geometry {
                    Geometry::Fill { rings, .. } => rings[0][0],
                    Geometry::Stroke { lines, .. } => lines[0].0[0],
                };
                let near = |p: P| ((p.x * 100.0).round() as i64, (p.y * 100.0).round() as i64);
                (
                    format!("{}/{:?}", name_of(s), s.part),
                    near(p),
                    s.color,
                    k,
                    s.params.angle,
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn groups(d: &Document, name: &str) -> Vec<String> {
        d.shapes
            .iter()
            .find(|s| name_of(s) == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .groups
            .clone()
    }

    fn at(d: &Document, name: &str) -> Vec<usize> {
        (0..d.shapes.len())
            .filter(|&i| name_of(&d.shapes[i]) == name)
            .collect()
    }

    fn reopened(d: &Document) -> Document {
        Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap()
    }

    /// Every design in a folder, its objects taken out of their groups,
    /// grouped anew, and put into one another's groups: each stays where
    /// it was, in its color, in its place in the order, with its settings,
    /// and so it comes back from the file.
    /// `ZIGZAG_DESIGNS=<folder> cargo test --lib -- --ignored groups_of_your_own`
    #[test]
    #[ignore]
    fn groups_of_your_own() {
        let Ok(dir) = std::env::var("ZIGZAG_DESIGNS") else {
            return;
        };
        type Seen = Vec<(usize, [f64; 4], [u8; 3], String)>;
        let seen = |d: &Document| -> Seen {
            d.shapes
                .iter()
                .map(|s| {
                    let (mut lo, mut hi) = (
                        P::new(f64::INFINITY, f64::INFINITY),
                        P::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
                    );
                    let pts: Vec<P> = match &s.geometry {
                        Geometry::Fill { rings, .. } => rings.iter().flatten().copied().collect(),
                        Geometry::Stroke { lines, .. } => {
                            lines.iter().flat_map(|l| l.0.iter().copied()).collect()
                        }
                    };
                    for p in pts {
                        lo = P::new(lo.x.min(p.x), lo.y.min(p.y));
                        hi = P::new(hi.x.max(p.x), hi.y.max(p.y));
                    }
                    (
                        s.uid,
                        [lo.x, lo.y, hi.x, hi.y],
                        s.color,
                        format!("{:?}", s.params.kind),
                    )
                })
                .collect()
        };
        let mut files = 0;
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("svg") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut d) = Document::from_svg(&text, "t", None) else {
                continue;
            };
            let n = d.shapes.len();
            if n < 2 {
                continue;
            }
            files += 1;
            let was = seen(&d);
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            // The same objects in the same order, each within a hundredth
            // of a millimetre of where it was (`ids`: by uid too).
            let same = |now: &Seen, was: &Seen, ids: bool, what: &str| {
                assert_eq!(now.len(), was.len(), "{name}: {what}: how many");
                for (a, b) in now.iter().zip(was) {
                    let near = a.1.iter().zip(b.1).all(|(x, y)| (x - y).abs() <= 0.1);
                    assert!(
                        near && a.2 == b.2 && a.3 == b.3 && (!ids || a.0 == b.0),
                        "{name}: {what}: {a:?} was {b:?}"
                    );
                }
            };
            let all: Vec<usize> = (0..n).collect();
            // Every object out of its group.
            d.ungroup(&all).unwrap_or_else(|e| panic!("{name}: ungroup: {e}"));
            same(&seen(&d), &was, true, "ungrouped");
            // Every other one grouped; then the rest put into that group.
            let half: Vec<usize> = (0..d.shapes.len()).step_by(2).collect();
            let g = d
                .group(&half)
                .unwrap_or_else(|e| panic!("{name}: group: {e}"));
            same(&seen(&d), &was, true, "grouped");
            let rest: Vec<usize> = (0..d.shapes.len())
                .filter(|&i| !d.shapes[i].groups.contains(&g))
                .collect();
            d.regroup(&rest, Some(&g))
                .unwrap_or_else(|e| panic!("{name}: regroup: {e}"));
            same(&seen(&d), &was, true, "all in one group");
            // And out to the top again, and back from the file.
            d.regroup(&(0..d.shapes.len()).collect::<Vec<_>>(), None)
                .unwrap_or_else(|e| panic!("{name}: out: {e}"));
            same(&seen(&d), &was, true, "out again");
            let again = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
            same(&seen(&again), &was, false, "reopened");
        }
        eprintln!("{files} designs");
    }

    #[test]
    fn an_object_put_into_a_group_stays_where_it_is_as_it_looks() {
        let mut d = doc();
        let was = look(&d);
        assert_eq!(groups(&d, "e"), Vec::<String>::new());
        // Into a group that's turned, from the top.
        assert!(d.regroup(&at(&d, "e"), Some("pair")).unwrap());
        assert_eq!(groups(&d, "e"), ["pair"]);
        assert_eq!(look(&d), was);
        // Out of a group that's scaled, inside a wrapper that's moved, and
        // that gave it its fill and its outline: into the turned one.
        assert!(d.regroup(&at(&d, "a"), Some("pair")).unwrap());
        assert_eq!(groups(&d, "a"), ["pair"]);
        assert_eq!(look(&d), was);
        // There already: nothing to do.
        assert!(!d.regroup(&at(&d, "a"), Some("pair")).unwrap());
        // And saved and opened again, the same.
        let again = reopened(&d);
        assert_eq!(look(&again), was);
        assert_eq!(groups(&again, "a"), ["pair"]);
        assert_eq!(groups(&again, "b"), ["set"]);
    }

    #[test]
    fn objects_taken_out_of_their_group_and_a_group_left_empty_goes() {
        let mut d = doc();
        let was = look(&d);
        // One of two: out to the top, the other still in the group.
        assert_eq!(d.ungroup(&at(&d, "b")).unwrap(), at(&d, "b").len());
        assert_eq!(groups(&d, "b"), Vec::<String>::new());
        assert_eq!(groups(&d, "a"), ["set"]);
        assert_eq!(look(&d), was);
        // The other is alone in it now, which is no group to speak of (a
        // group of one is its object, wherever groups are shown): there's
        // nothing to take it out of.
        assert_eq!(d.ungroup(&at(&d, "a")).unwrap(), 0);
        // Both out at once: the group's gone from the file.
        let mut d = doc();
        let both: Vec<usize> = ["a", "b"].iter().flat_map(|n| at(&d, n)).collect();
        assert_eq!(d.ungroup(&both).unwrap(), both.len());
        assert_eq!(groups(&d, "a"), Vec::<String>::new());
        assert_eq!(groups(&d, "b"), Vec::<String>::new());
        assert_eq!(look(&d), was);
        assert!(!d.to_svg().unwrap().contains("id=\"set\""));
        assert!(d.regroup(&at(&d, "a"), Some("set")).is_err(), "no such group");
        // Nothing in a group: nothing comes out.
        assert_eq!(d.ungroup(&at(&d, "e")).unwrap(), 0);
        assert_eq!(look(&reopened(&d)), was);
    }

    #[test]
    fn objects_are_grouped_where_they_all_are_and_a_group_goes_into_another() {
        let mut d = doc();
        let was = look(&d);
        // From three places: a group of them, at the top.
        let picked: Vec<usize> = ["a", "c", "e"].iter().flat_map(|n| at(&d, n)).collect();
        let g = d.group(&picked).unwrap();
        for name in ["a", "c", "e"] {
            assert_eq!(groups(&d, name), [g.clone()], "{name}");
        }
        assert_eq!(groups(&d, "b"), ["set"]);
        assert_eq!(groups(&d, "d"), ["pair"]);
        assert_eq!(look(&d), was);
        // Two in the same group: a group inside it.
        let picked: Vec<usize> = ["a", "c"].iter().flat_map(|n| at(&d, n)).collect();
        let inner = d.group(&picked).unwrap();
        assert_eq!(groups(&d, "a"), [g.clone(), inner.clone()]);
        assert_eq!(groups(&d, "e"), [g.clone()]);
        assert_eq!(look(&d), was);
        // Out of the inner one: back in the outer.
        d.ungroup(&at(&d, "a")).unwrap();
        assert_eq!(groups(&d, "a"), [g.clone()]);
        // Every object of one group into another: the two are one. (`b` is
        // the last in its group, which goes with it as what's round it:
        // its name and settings may be on that.)
        let rest: Vec<usize> = ["b"].iter().flat_map(|n| at(&d, n)).collect();
        d.regroup(&rest, Some(&g)).unwrap();
        assert_eq!(groups(&d, "b").first(), Some(&g));
        assert_eq!(look(&d), was);
        // And two whole groups merged: every object of one into the other.
        let mut d = doc();
        let pair: Vec<usize> = ["c", "d"].iter().flat_map(|n| at(&d, n)).collect();
        d.regroup(&pair, Some("set")).unwrap();
        for name in ["a", "b", "c", "d"] {
            assert_eq!(groups(&d, name), ["set"], "{name}");
        }
        assert!(!d.to_svg().unwrap().contains("id=\"pair\""));
        assert_eq!(look(&d), was);
        assert_eq!(look(&reopened(&d)), was);
    }

    #[test]
    fn a_group_is_given_a_name() {
        let mut d = doc();
        let was = look(&d);
        let picked: Vec<usize> = ["c", "e"].iter().flat_map(|n| at(&d, n)).collect();
        let g = d.group(&picked).unwrap();
        let named = d.rename_group(&g, "Left wing").unwrap();
        assert_eq!(named, "Left_wing");
        assert_eq!(groups(&d, "e"), ["Left_wing"]);
        // A name another element has: told apart, and read as the same name.
        assert_eq!(d.rename_group("pair", "left wing!").unwrap(), "left_wing");
        assert_eq!(d.rename_group("set", "Left wing").unwrap(), "Left_wing_#2");
        assert_eq!(crate::svg::name_words("Left_wing_#2"), ["Left", "wing"]);
        // Nothing but a stitch setting isn't a name; nor is a group that isn't.
        assert!(d.rename_group("Left_wing", "satin").is_err());
        assert!(d.rename_group("nowhere", "wing").is_err());
        assert_eq!(look(&d), was);
        assert_eq!(look(&reopened(&d)), was);
    }
}
