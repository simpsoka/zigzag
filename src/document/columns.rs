//! Object › Break into Columns: a satin worked out from a shape's outline,
//! made the objects it's sewn as, to see and change: each column a
//! two-rail satin of its own (its rails and rungs), in the order and the
//! way round the planner sews them, so that what's sewn is as it was. As
//! Ink/Stitch leaves the result of its Auto-Route Satin Columns: ordinary
//! objects, which the sewing order, start and end points, trims and the
//! path tools all work on as on any other.

use super::{Document, xml_escape};
use crate::bezier::{self, Anchor, BezPath, SubPath};
use crate::geom::{IDENTITY, P, affine_inv, affine_mul};
use crate::params::{Kind, Params};
use crate::stitch::satin::{self, Rails};
use crate::svg::{Geometry, Part};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashMap;

/// Rungs on a piece of satin no further apart than this (units): enough to
/// hold its stitches square to it, as the column it came from had them.
const RUNG_EVERY: f64 = 15.0;

/// How far past each rail a rung reaches (of the column's width there), so
/// that it's seen to cross both.
const RUNG_OVER: f64 = 0.15;

/// What one object comes to, broken into columns: its pieces as paths (in
/// design units) with their settings, in sewing order.
struct Pieces {
    paths: Vec<(BezPath, Params, &'static str)>,
}

impl Document {
    /// Break each of `indices` (a filled shape sewn in satin) into the
    /// columns it's sewn as, routed: see the module. Returns the new
    /// objects' indices.
    pub fn break_into_columns(&mut self, indices: &[usize]) -> Result<Vec<usize>> {
        // All or nothing: a refusal part-way leaves the document as it was.
        let mut next = self.clone();
        let made = next.columns_of(indices)?;
        *self = next;
        Ok(made)
    }

    fn columns_of(&mut self, indices: &[usize]) -> Result<Vec<usize>> {
        let uids: Vec<usize> = indices
            .iter()
            .filter_map(|&i| self.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        self.sync()?;
        // (The order each satin's columns are sewn in, as planned now.)
        let planned = self.plan();
        let order_of = |i: usize| -> Vec<(usize, bool)> {
            planned
                .satin_columns
                .iter()
                .filter(|(s, ..)| *s == i)
                .map(|&(_, c, turned)| (c, turned))
                .collect()
        };
        let orders: HashMap<usize, Vec<(usize, bool)>> = uids
            .iter()
            .filter_map(|&uid| self.shapes.iter().position(|s| s.uid == uid))
            .map(|i| (self.shapes[i].uid, order_of(i)))
            .collect();
        let mut why = "select a filled shape sewn in satin";
        let mut made_ids: Vec<(String, [u8; 3])> = Vec::new();
        for uid in uids {
            let Some(i) = self.shapes.iter().position(|s| s.uid == uid) else {
                continue;
            };
            let s = &self.shapes[i];
            if !matches!(s.geometry, Geometry::Fill { .. })
                || s.part != Part::Fill
                || s.params.kind != Some(Kind::Satin)
                || s.params.is_border()
            {
                continue;
            }
            if self.lettering(i).is_some() {
                why = "turn the text into shapes first (Type › Create Outlines)";
                continue;
            }
            let pieces = pieces_of(
                &self.shapes[i],
                orders.get(&uid).map_or(&[][..], |o| o.as_slice()),
            );
            if pieces.paths.is_empty() {
                why = "that shape has no columns to break into";
                continue;
            }
            let (color, name) = (self.shapes[i].color, self.name_of(i));
            let ids = self.insert_pieces(&pieces, Some(i), &name, color)?;
            let i = self.shapes.iter().position(|s| s.uid == uid).unwrap();
            self.delete(i)?;
            made_ids.extend(ids.into_iter().map(|id| (id, color)));
        }
        if made_ids.is_empty() {
            bail!("{why}");
        }
        let map = self.uid_map(&HashMap::new());
        self.import(&map)?;
        let mut made = Vec::new();
        for (id, color) in made_ids {
            if let Some(n) = self.shapes.iter().position(|s| s.id == id) {
                self.shapes[n].color = color;
                made.push(n);
            }
        }
        Ok(made)
    }

    /// `pieces` as a group of paths named for `name`, where object
    /// `behind` is (just before it, in the groups it's in, so sewn where
    /// it was among what's beside it): their element ids.
    fn insert_pieces(
        &mut self,
        pieces: &Pieces,
        behind: Option<usize>,
        name: &str,
        color: [u8; 3],
    ) -> Result<Vec<String>> {
        let m = self.mm_to_user()?;
        let used: std::collections::HashSet<String> = {
            let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
            doc.descendants()
                .filter_map(|n| n.attribute("id").map(str::to_string))
                .collect()
        };
        let fresh = |base: &str, taken: &std::collections::HashSet<String>| -> String {
            let mut k = 1;
            let mut id = base.to_string();
            while taken.contains(&id) {
                k += 1;
                id = format!("{base}_#{k}");
            }
            id
        };
        let hex = format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2]);
        let mut taken = used;
        let group = fresh(&format!("{name}_columns"), &taken);
        taken.insert(group.clone());
        let mut ids = Vec::new();
        let mut body = String::new();
        for (k, (path, params, what)) in pieces.paths.iter().enumerate() {
            let tokens = params.tokens().join("_");
            let id = fresh(&format!("{name}_{what}{}_{tokens}", k + 1), &taken);
            taken.insert(id.clone());
            let d = bezier::to_svg_d(&bezier::transform(path, [0.1, 0.0, 0.0, 0.1, 0.0, 0.0]));
            let paint = if params.kind == Some(Kind::Fill) {
                format!("fill=\"{hex}\" fill-rule=\"evenodd\"")
            } else {
                format!(
                    "fill=\"none\" stroke=\"{hex}\" stroke-width=\"0.2\" stroke-linejoin=\"round\""
                )
            };
            body.push_str(&format!(
                "<path id=\"{}\" d=\"{d}\" {paint}/>",
                xml_escape(&id)
            ));
            ids.push(id);
        }
        // Where it goes, and what the groups it's then in do to it (undone
        // in its own transform, as a Shape Builder result's is).
        let (at, undo) = match behind.and_then(|i| self.shapes.get(i)) {
            Some(s) => {
                let doc = roxmltree::Document::parse(&self.xml).context("parsing document")?;
                let n = doc
                    .descendants()
                    .find(|n| n.is_element() && n.attribute("id") == Some(s.id.as_str()))
                    .ok_or_else(|| anyhow!("element {} not found", s.id))?;
                let groups: Vec<_> = n
                    .ancestors()
                    .skip(1)
                    .filter(|a| a.is_element() && *a != doc.root_element())
                    .collect();
                let inside = groups
                    .iter()
                    .rev()
                    .filter_map(|g| g.attribute("transform"))
                    .fold(IDENTITY, |all, t| {
                        affine_mul(all, super::edit::parse_transform(t))
                    });
                match affine_inv(inside) {
                    Some(out) if groups.iter().all(|g| g.tag_name().name() == "g") => {
                        (n.range().start, out)
                    }
                    // (Inside something that isn't a plain group, or one
                    // that can't be undone: before the whole of that.)
                    _ => {
                        let top = groups.last().copied().unwrap_or(n);
                        (top.range().start, IDENTITY)
                    }
                }
            }
            None => (
                self.xml
                    .rfind("</svg>")
                    .ok_or_else(|| anyhow!("document has no closing </svg>"))?,
                IDENTITY,
            ),
        };
        self.next_group += 1;
        let el = format!(
            "<g id=\"tl-drawing-{}\" transform=\"{}\"><g id=\"{}\">{body}</g></g>\n",
            self.next_group,
            super::edit::fmt_matrix(affine_mul(undo, m)),
            xml_escape(&group)
        );
        self.xml.insert_str(at, &el);
        Ok(ids)
    }
}

/// The shape's columns as paths, in `order` (each column, by its place
/// among the shape's, and whether it's sewn from its end: as the planner
/// sewed them, [`crate::design::Design::satin_columns`]), any it left out
/// after: each as its two rails and its rungs (sewn together, as one
/// satin's columns, they're lapped where they meet as the shape's were:
/// see `design::joined_satin`); then the parts that couldn't be columns,
/// filled as they were.
fn pieces_of(shape: &crate::svg::Shape, order: &[(usize, bool)]) -> Pieces {
    let parts = crate::design::satin_rails(shape);
    let columns: Vec<Rails> = parts
        .iter()
        .filter_map(|p| p.as_ref().ok())
        .cloned()
        .collect();
    let mut satin_params = shape.params.clone();
    satin_params.kind = Some(Kind::Satin);
    satin_params.rails = Some((1, 2));
    satin_params.border = None;
    satin_params.overlap = None;
    satin_params.cut = None;
    satin_params.mitred = None;
    satin_params.width = None;
    satin_params.start = None;
    satin_params.end = None;
    satin_params.trim = None;
    satin_params.trim_before = None;
    satin_params.stop = None;
    satin_params.order = None;
    let mut paths: Vec<(BezPath, Params, &'static str)> = Vec::new();
    let mut order = order.to_vec();
    order.extend(
        (0..columns.len())
            .filter(|c| !order.iter().any(|(o, _)| o == c))
            .map(|c| (c, false))
            .collect::<Vec<_>>(),
    );
    for (c, turned) in order {
        let Some(r) = columns.get(c) else {
            continue;
        };
        let r = if turned {
            satin::reversed(r)
        } else {
            r.clone()
        };
        paths.push((rails_path(&r), satin_params.clone(), "satin"));
    }
    let fill_params = Params {
        kind: Some(Kind::Fill),
        start: None,
        end: None,
        trim: None,
        trim_before: None,
        stop: None,
        order: None,
        ..shape.params.clone()
    };
    for (rings, _) in parts.iter().filter_map(|p| p.as_ref().err()) {
        let path: BezPath = rings
            .iter()
            .filter(|r| r.len() >= 3)
            .map(|r| SubPath {
                anchors: r.iter().map(|&p| Anchor::corner(p)).collect(),
                closed: true,
            })
            .collect();
        if !path.is_empty() {
            paths.push((path, fill_params.clone(), "fill"));
        }
    }
    if let Some(first) = paths.first_mut() {
        first.1.trim_before = shape.params.trim_before;
    }
    // What the shape said happens after it (a cut, a stop) is after the
    // last of its pieces.
    if let Some(last) = paths.last_mut() {
        (last.1.trim, last.1.stop) = (shape.params.trim, shape.params.stop);
    }
    Pieces { paths }
}

/// A piece of satin as a path: rail a, rail b, then its rungs, each a
/// little longer than the column is wide there.
fn rails_path(r: &Rails) -> BezPath {
    let mut path = vec![rail(&r.a), rail(&r.b)];
    let pairs = satin::pairs(r, RUNG_EVERY);
    for (a, b) in pairs.iter().skip(1).take(pairs.len().saturating_sub(2)) {
        let over = (*b - *a) * RUNG_OVER;
        path.push(open(&[*a - over, *b + over]));
    }
    path
}

/// A rail as few points as follow it (curves fitted as path operations'
/// results are, corners kept), so it can be reshaped by hand.
fn rail(pts: &[P]) -> SubPath {
    crate::bezier::fit_outline(pts, false, super::paths::RESULT_TOL, 40.0)
        .filter(|sp| sp.anchors.len() >= 2)
        .unwrap_or_else(|| open(pts))
}

fn open(pts: &[P]) -> SubPath {
    SubPath {
        anchors: pts.iter().map(|&p| Anchor::corner(p)).collect(),
        closed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An H, 20 mm tall, its stems 3 mm wide and its bar 2 mm, as one
    /// filled shape sewn in satin.
    const H: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
      <path id="H_satin" fill="#123456" d="M10 10 L13 10 L13 19 L27 19 L27 10 L30 10 L30 30 L27 30 L27 21 L13 21 L13 30 L10 30 Z"/>
    </svg>"##;

    #[test]
    fn a_satin_breaks_into_its_columns_and_sews_as_it_did() {
        let mut d = Document::from_svg(H, "t", None).unwrap();
        assert_eq!(d.shapes.len(), 1);
        let made = d.break_into_columns(&[0]).unwrap();
        assert!(made.len() >= 3, "{} objects", made.len());
        assert!(
            d.shapes.iter().all(|s| s.id != "H_satin"),
            "the shape it came from is gone"
        );
        let kinds: Vec<Option<Kind>> = d.shapes.iter().map(|s| s.params.kind).collect();
        assert!(kinds.iter().all(|k| *k == Some(Kind::Satin)), "{kinds:?}");
        assert!(
            d.shapes
                .iter()
                .filter(|s| s.params.kind == Some(Kind::Satin))
                .all(|s| s.params.rails == Some((1, 2))),
            "each piece of satin says which subpaths are its rails"
        );
        assert!(
            d.shapes.iter().all(|s| s.color == [0x12, 0x34, 0x56]),
            "in the shape's thread"
        );
        // The columns are sewn as the shape's were: the letter in one
        // piece of thread.
        let was = Document::from_svg(H, "t", None).unwrap().plan();
        let design = d.plan();
        assert_eq!(
            design.trim_count(),
            was.trim_count(),
            "cut between the columns"
        );
        assert_eq!(design.trim_count(), 0);
        // Saved and opened again, it's the same objects.
        let again = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        assert_eq!(again.shapes.len(), d.shapes.len());
        assert_eq!(
            again
                .shapes
                .iter()
                .map(|s| (s.params.kind, s.params.rails))
                .collect::<Vec<_>>(),
            d.shapes
                .iter()
                .map(|s| (s.params.kind, s.params.rails))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn overlapping_columns_break_out_and_sew_as_they_did() {
        let svg = H.replace("H_satin", "H_satin_overlap");
        let was = Document::from_svg(&svg, "t", None).unwrap();
        assert_eq!(was.shapes[0].params.overlap, Some(true));
        let before = was.plan();
        let mut d = was.clone();
        d.break_into_columns(&[0]).unwrap();
        assert!(
            d.shapes.iter().all(|s| s.params.overlap.is_none()),
            "a column has no letter to lay over"
        );
        let after = d.plan();
        assert_eq!(after.trim_count(), before.trim_count());
        let stitches = |x: &crate::design::Design| x.stitch_count() as f64;
        assert!(
            (stitches(&after) - stitches(&before)).abs() <= 0.1 * stitches(&before),
            "{} vs {}",
            stitches(&after),
            stitches(&before)
        );
    }

    #[test]
    fn broken_out_columns_are_sewn_in_the_order_theyre_in() {
        let mut d = Document::from_svg(H, "t", None).unwrap();
        d.break_into_columns(&[0]).unwrap();
        let n = d.shapes.len();
        let first_sewn = |d: &Document| {
            let design = d.plan();
            design.stitch_shapes[0][0][0].1
        };
        assert_eq!(first_sewn(&d), 0);
        // Moved to the front, the last column is sewn first; the thread
        // still travels on under the rest.
        let last = d.shapes.remove(n - 1);
        d.shapes.insert(0, last);
        assert_eq!(first_sewn(&d), 0);
        let design = d.plan();
        let order: Vec<usize> = design.stitch_shapes[0]
            .iter()
            .flatten()
            .map(|&(_, si)| si)
            .fold(Vec::new(), |mut v, si| {
                if v.last() != Some(&si) {
                    v.push(si);
                }
                v
            });
        assert_eq!(order, (0..n).collect::<Vec<_>>(), "in the order they're in");
    }

    #[test]
    fn a_shape_in_a_group_breaks_where_it_is_and_cuts_where_it_did() {
        // An H between two bars in a group that's moved and scaled, the H
        // to be cut after: its columns are sewn between the bars, where
        // the H was, at the size it was, and the cut is after the last.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
          <g id="set" transform="translate(5 5) scale(1.5)">
            <path id="before_fill" fill="#aa0000" d="M0 0 H6 V6 H0 Z"/>
            <path id="H_satin_trim_trimbefore=1" fill="#123456" d="M10 10 L13 10 L13 19 L27 19 L27 10 L30 10 L30 30 L27 30 L27 21 L13 21 L13 30 L10 30 Z"/>
            <path id="after_fill" fill="#00aa00" d="M40 0 H46 V6 H40 Z"/>
          </g>
        </svg>"##;
        let mut d = Document::from_svg(svg, "t", None).unwrap();
        let was = crate::svg::shapes_bounds(&d.shapes[1..=1]);
        let cuts = d.plan().trim_count();
        let made = d.break_into_columns(&[1]).unwrap();
        let n = d.shapes.len();
        assert_eq!(made, (1..n - 1).collect::<Vec<_>>(), "between the bars");
        assert!(d.shapes[0].id.starts_with("before") && d.shapes[n - 1].id.starts_with("after"));
        // (Its rungs reach a little past its rails.)
        let now = crate::svg::shapes_bounds(&d.shapes[1..n - 1]);
        assert!(
            now.min.dist(was.min) < 8.0 && now.max.dist(was.max) < 8.0,
            "{was:?} {now:?}"
        );
        let trims: Vec<Option<bool>> = made.iter().map(|&i| d.shapes[i].params.trim).collect();
        assert_eq!(trims.last(), Some(&Some(true)), "{trims:?}");
        assert!(
            trims[..trims.len() - 1].iter().all(|t| t.is_none()),
            "{trims:?}"
        );
        assert_eq!(d.shapes[made[0]].params.trim_before, Some(true));
        assert!(
            made.iter()
                .skip(1)
                .all(|&i| d.shapes[i].params.trim_before.is_none())
        );
        // Still one satin up to the cut: as many cuts as before.
        assert_eq!(d.plan().trim_count(), cuts);
    }

    #[test]
    fn a_satin_of_two_threads_breaks_into_each_column_once() {
        let plain = Document::from_svg(H, "t", None)
            .unwrap()
            .clone()
            .break_into_columns(&[0])
            .unwrap()
            .len();
        let svg = H.replace("H_satin", "H_satin_colors=aa0000+0000aa");
        let mut d = Document::from_svg(&svg, "t", None).unwrap();
        assert!(d.shapes[0].params.colors.is_some());
        assert_eq!(d.break_into_columns(&[0]).unwrap().len(), plain);
    }

    #[test]
    fn a_shape_that_isnt_satin_is_left_alone() {
        let svg = H.replace("H_satin", "H_fill");
        let mut d = Document::from_svg(&svg, "t", None).unwrap();
        assert!(d.break_into_columns(&[0]).is_err());
        assert_eq!(d.shapes.len(), 1);
        assert_eq!(d.shapes[0].id, "H_fill");
    }
}

#[cfg(test)]
mod corpus {
    use super::*;

    /// Every `.svg` in the folder `ZIGZAG_BREAK` names, its satin shapes
    /// broken into columns: the trims before and after, and the result
    /// written beside it as `<name>-columns.svg`.
    #[test]
    #[ignore]
    fn designs_broken_into_columns() {
        let Ok(dir) = std::env::var("ZIGZAG_BREAK") else {
            return;
        };
        let mut paths: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension().is_some_and(|x| x == "svg")
                    && !p.to_string_lossy().ends_with("-columns.svg")
            })
            .collect();
        paths.sort();
        let (mut before, mut after, mut whole) = (0, 0, 0);
        for p in paths {
            let svg = std::fs::read_to_string(&p).unwrap();
            let Ok(mut d) = Document::from_svg(&svg, "t", None) else {
                continue;
            };
            let was = d.plan().trim_count();
            let satins: Vec<usize> = (0..d.shapes.len())
                .filter(|&i| {
                    d.shapes[i].params.kind == Some(Kind::Satin)
                        && matches!(d.shapes[i].geometry, Geometry::Fill { .. })
                })
                .collect();
            if satins.is_empty() || d.break_into_columns(&satins).is_err() {
                println!("{}: not broken", p.display());
                continue;
            }
            let now = d.plan().trim_count();
            before += was;
            after += now;
            whole += 1;
            println!(
                "{}: trims {was} -> {now}",
                p.file_name().unwrap().to_string_lossy()
            );
            let out = p.with_file_name(format!(
                "{}-columns.svg",
                p.file_stem().unwrap().to_string_lossy()
            ));
            std::fs::write(out, d.to_svg().unwrap()).unwrap();
        }
        println!("{whole} designs: trims {before} -> {after}");
    }
}
