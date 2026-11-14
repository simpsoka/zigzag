//! Simplify Design: make a design quicker and easier to sew by giving it
//! fewer threads, fewer small details and fewer trims. (Illustrator's
//! Simplify takes points out of a path; this takes work out of a design.)
//!
//! It does nothing the rest of the app can't: threads are merged as the
//! thread picker would recolor them, details go as Clean Up's specks do,
//! gaps are sewn across by the document's Connect setting and the order is
//! Group Colors'. This is those together, with one answer for "how much".

use super::{CleanUp, Document};
use crate::params::Kind;
use crate::svg::{Geometry, Shape};
use anyhow::Result;

/// How far to simplify. The default changes nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimplifyDesign {
    /// Merge the threads most alike until this many are left (`None`:
    /// leave the threads as they are).
    pub threads: Option<usize>,
    /// Take out objects smaller than this across, mm (0: none). Lettering
    /// is left alone.
    pub min_size_mm: f64,
    /// Sew across gaps up to this long, mm, instead of cutting the thread
    /// (the document's Connect; never lowered from what it is).
    pub connect_mm: f64,
    /// Sew each thread in as few passes as the layering allows.
    pub group: bool,
    /// Make one shape of layers that touch, sew one after another and
    /// share a thread: no stitching under or between them.
    pub unite: Unite,
}

/// Which neighbors in one thread become one shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unite {
    /// None: every layer stays a layer.
    #[default]
    Off,
    /// The ones alike: the same stitch type too. A satin on a fill stays
    /// a satin on a fill; the fills beside each other become one.
    Similar,
    /// All of them, whatever they're sewn as: the fewest stitches, and
    /// what told them apart in the one thread goes.
    All,
}

impl Unite {
    pub const ALL: [Unite; 3] = [Unite::Off, Unite::Similar, Unite::All];
    /// The name agents use.
    pub fn id(self) -> &'static str {
        match self {
            Unite::Off => "off",
            Unite::Similar => "similar",
            Unite::All => "all",
        }
    }
    pub fn by_id(id: &str) -> Option<Unite> {
        Unite::ALL.into_iter().find(|u| u.id() == id)
    }
}

impl Default for SimplifyDesign {
    fn default() -> Self {
        SimplifyDesign {
            threads: None,
            min_size_mm: 0.0,
            connect_mm: 0.0,
            group: false,
            unite: Unite::Off,
        }
    }
}

impl SimplifyDesign {
    /// The largest "smaller than" Simplify offers, mm.
    pub const MAX_SIZE_MM: f64 = 10.0;
    /// The longest gap Simplify offers to sew across, mm.
    pub const MAX_CONNECT_MM: f64 = 10.0;
}

/// What [`Document::simplify_design`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SimplifyReport {
    /// How many threads the design had, and has now.
    pub threads: (usize, usize),
    /// Each thread that went, and the one its objects sew in now.
    pub merged: Vec<([u8; 3], [u8; 3])>,
    /// Objects taken out for being too small.
    pub removed: usize,
    /// Layers that went into a neighbor in the same thread.
    pub united: usize,
    /// The sewing order changed.
    pub reordered: bool,
    /// The Connect setting, before and now (mm).
    pub connect_mm: (f64, f64),
}

impl SimplifyReport {
    /// Whether anything changed.
    pub fn changed(&self) -> bool {
        !self.merged.is_empty()
            || self.removed > 0
            || self.united > 0
            || self.reordered
            || self.connect_mm.0 != self.connect_mm.1
    }
}

/// Whether an object is sewn at all (not turned off, not hidden).
fn sewn(s: &Shape) -> bool {
    s.params.kind != Some(Kind::Skip) && !s.params.hidden()
}

/// About how much sewing an object is, to weigh its thread by: its area
/// (mm²), or for a line its length times its width.
fn weight(s: &Shape) -> f64 {
    match &s.geometry {
        Geometry::Fill { rings, .. } => {
            rings
                .iter()
                .map(|r| crate::boolean::ring_area(r))
                .sum::<f64>()
                .abs()
                / 100.0
        }
        Geometry::Stroke { lines, width } => {
            let len: f64 = lines
                .iter()
                .map(|(l, _)| l.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>())
                .sum();
            len / 10.0 * (width / 10.0).max(0.4)
        }
    }
}

impl Document {
    /// The threads the design sews in, with about how much each sews
    /// (see [`weight`]), most first.
    pub fn threads(&self) -> Vec<([u8; 3], f64)> {
        let mut out: Vec<([u8; 3], f64)> = Vec::new();
        for s in self.shapes.iter().filter(|s| sewn(s)) {
            let w = weight(s);
            match out.iter_mut().find(|(c, _)| *c == s.color) {
                Some((_, total)) => *total += w,
                None => out.push((s.color, w)),
            }
        }
        // (Stable, so threads that sew the same amount keep sewing order.)
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }

    /// Simplify the design as far as `opt` says: see [`SimplifyDesign`].
    pub fn simplify_design(&mut self, opt: &SimplifyDesign) -> Result<SimplifyReport> {
        let mut report = SimplifyReport {
            connect_mm: (self.settings.connect_mm, self.settings.connect_mm),
            ..SimplifyReport::default()
        };
        let before = self.threads().len();

        // Details too small to be worth the stop: out.
        let size = opt.min_size_mm.clamp(0.0, SimplifyDesign::MAX_SIZE_MM);
        if size > 0.0 {
            let small = CleanUp {
                stray: true,
                specks: true,
                speck: size * 10.0,
                points: false,
            };
            let found = self.clean_up(&small, false)?;
            report.removed = found.stray + found.specks;
        }

        // The two threads most alike become one (the one that sews more:
        // a real spool, not a blend of the two), until few enough are left.
        if let Some(keep) = opt.threads {
            let mut threads = self.threads();
            while threads.len() > keep.max(1) {
                let mut best: Option<(i64, usize, usize)> = None;
                for a in 0..threads.len() {
                    for b in a + 1..threads.len() {
                        let d = crate::formats::color_distance(threads[a].0, threads[b].0);
                        if best.is_none_or(|(least, _, _)| d < least) {
                            best = Some((d, a, b));
                        }
                    }
                }
                let Some((_, a, b)) = best else { break };
                // (Sorted most first, so `a` sews at least as much as `b`.)
                let (gone, _) = threads.remove(b);
                let stays = threads[a].0;
                threads[a].1 += weight_of(&self.shapes, gone);
                for s in self.shapes.iter_mut().filter(|s| sewn(s)) {
                    if s.color == gone {
                        s.color = stays;
                    }
                }
                // (A thread merged earlier into the one that just went.)
                for m in &mut report.merged {
                    if m.1 == gone {
                        m.1 = stays;
                    }
                }
                report.merged.push((gone, stays));
                threads.sort_by(|x, y| y.1.total_cmp(&x.1));
            }
        }

        // Each thread in as few passes as the layering allows.
        if opt.group {
            let order = crate::design::grouped_by_color(&self.shapes);
            if order.iter().enumerate().any(|(k, &i)| k != i) {
                let mut slots: Vec<Option<Shape>> = std::mem::take(&mut self.shapes)
                    .into_iter()
                    .map(Some)
                    .collect();
                self.shapes = order
                    .iter()
                    .map(|&i| slots[i].take().expect("a permutation"))
                    .collect();
                report.reordered = true;
            }
        }

        // Neighbors in one thread: one shape.
        if opt.unite != Unite::Off {
            report.united = self.unite_touching(opt.unite == Unite::Similar)?;
        }

        // Short gaps sewn across rather than cut.
        let connect = opt.connect_mm.clamp(0.0, SimplifyDesign::MAX_CONNECT_MM);
        if connect > self.settings.connect_mm {
            self.settings.connect_mm = connect;
            report.connect_mm.1 = connect;
        }

        report.threads = (before, self.threads().len());
        Ok(report)
    }
}

/// How much the objects in thread `c` sew.
fn weight_of(shapes: &[Shape], c: [u8; 3]) -> f64 {
    shapes
        .iter()
        .filter(|s| sewn(s) && s.color == c)
        .map(weight)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::P;

    fn square(d: &mut Document, x: f64, size: f64, color: [u8; 3]) -> usize {
        let pts = vec![
            P::new(x, 0.0),
            P::new(x + size, 0.0),
            P::new(x + size, size),
            P::new(x, size),
        ];
        d.add_drawing(&pts, true, color, Kind::Fill).unwrap()
    }

    /// Two reds and two blues (one of each big, one small), and a green
    /// speck.
    fn design() -> Document {
        let mut d = Document::empty();
        square(&mut d, 0.0, 200.0, [200, 0, 0]);
        square(&mut d, 300.0, 200.0, [0, 0, 200]);
        square(&mut d, 600.0, 60.0, [215, 20, 10]);
        square(&mut d, 700.0, 60.0, [10, 20, 180]);
        square(&mut d, 800.0, 8.0, [0, 160, 0]);
        d
    }

    #[test]
    fn the_default_changes_nothing() {
        let mut d = design();
        let before = d.to_svg().unwrap();
        let r = d.simplify_design(&SimplifyDesign::default()).unwrap();
        assert!(!r.changed());
        assert_eq!(r.threads, (5, 5));
        assert_eq!(d.to_svg().unwrap(), before);
    }

    #[test]
    fn threads_most_alike_merge_into_the_one_that_sews_more() {
        let mut d = design();
        // Most sewing first: the big squares, then the small, then the speck.
        let t: Vec<[u8; 3]> = d.threads().iter().map(|t| t.0).collect();
        assert_eq!(t[..2], [[200, 0, 0], [0, 0, 200]]);
        assert_eq!(t.len(), 5);
        let r = d
            .simplify_design(&SimplifyDesign {
                threads: Some(3),
                ..SimplifyDesign::default()
            })
            .unwrap();
        assert_eq!(r.threads, (5, 3));
        // The light red went to the red, the dark blue to the blue: real
        // threads kept, not a mix of the two.
        assert_eq!(r.merged.len(), 2);
        assert!(r.merged.contains(&([215, 20, 10], [200, 0, 0])));
        assert!(r.merged.contains(&([10, 20, 180], [0, 0, 200])));
        let colors: Vec<[u8; 3]> = d.shapes.iter().map(|s| s.color).collect();
        assert_eq!(
            colors,
            vec![
                [200, 0, 0],
                [0, 0, 200],
                [200, 0, 0],
                [0, 0, 200],
                [0, 160, 0]
            ]
        );
        // It's what's sewn, and what's saved.
        assert_eq!(d.plan().blocks.len(), 5, "same order: five blocks still");
        let back = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        assert_eq!(back.threads().len(), 3);
        // Down to one: everything in the thread that sews most.
        let r = d
            .simplify_design(&SimplifyDesign {
                threads: Some(1),
                ..SimplifyDesign::default()
            })
            .unwrap();
        assert_eq!(r.threads, (3, 1));
        assert!(d.shapes.iter().all(|s| s.color == [200, 0, 0]));
        // (And never to none.)
        let r = d
            .simplify_design(&SimplifyDesign {
                threads: Some(0),
                ..SimplifyDesign::default()
            })
            .unwrap();
        assert_eq!(r.threads, (1, 1));
    }

    #[test]
    fn grouping_sews_each_thread_in_one_pass() {
        let mut d = design();
        let opt = SimplifyDesign {
            threads: Some(3),
            group: true,
            ..SimplifyDesign::default()
        };
        let r = d.simplify_design(&opt).unwrap();
        assert!(r.reordered);
        assert_eq!(d.plan().blocks.len(), 3, "one block a thread");
        // Again: nothing left to do.
        let r = d.simplify_design(&opt).unwrap();
        assert!(!r.changed(), "{r:?}");
    }

    #[test]
    fn small_details_go_and_short_gaps_are_sewn_across() {
        let mut d = design();
        let r = d
            .simplify_design(&SimplifyDesign {
                min_size_mm: 1.0,
                connect_mm: 3.0,
                ..SimplifyDesign::default()
            })
            .unwrap();
        // The 0.8 mm speck (and its thread) went; the 6 mm squares didn't.
        assert_eq!(r.removed, 1);
        assert_eq!(d.shapes.len(), 4);
        assert_eq!(r.threads, (5, 4));
        assert_eq!(r.connect_mm, (0.0, 3.0));
        assert_eq!(d.settings.connect_mm, 3.0);
        // Connect is never lowered, and sizes are capped.
        let r = d
            .simplify_design(&SimplifyDesign {
                connect_mm: 1.0,
                min_size_mm: 500.0,
                ..SimplifyDesign::default()
            })
            .unwrap();
        assert_eq!(d.settings.connect_mm, 3.0);
        assert_eq!(r.removed, 2, "the two 6 mm squares, under the 10 mm cap");
        assert_eq!(d.shapes.len(), 2);
    }

    fn area_mm2(d: &Document, i: usize) -> f64 {
        match &d.shapes[i].geometry {
            Geometry::Fill { rings, .. } => {
                rings
                    .iter()
                    .map(|r| crate::boolean::ring_area(r))
                    .sum::<f64>()
                    / 100.0
            }
            g => panic!("{g:?}"),
        }
    }

    #[test]
    fn neighbors_in_one_thread_unite_into_one_shape() {
        let red = [200, 0, 0];
        let mut d = Document::empty();
        // A big red square, a blue one well away, a smaller red one over
        // the big one's edge, and a red one on its own.
        let a = square(&mut d, 0.0, 200.0, red);
        let far = square(&mut d, 1000.0, 100.0, [0, 0, 200]);
        let b = square(&mut d, 150.0, 100.0, red);
        let alone = square(&mut d, 500.0, 100.0, red);
        d.shapes[a].params.angle = Some(30.0);
        d.shapes[b].params.angle = Some(75.0);
        let (far_uid, alone_uid) = (d.shapes[far].uid, d.shapes[alone].uid);
        let plain = d.plan().stitch_count();
        // As they are, the blue sews between the reds: nothing to unite.
        let mut as_is = d.clone();
        let r = as_is
            .simplify_design(&SimplifyDesign {
                unite: Unite::All,
                ..SimplifyDesign::default()
            })
            .unwrap();
        assert_eq!((r.united, as_is.shapes.len()), (0, 4));
        // Each thread together: the two reds that touch are one shape, in
        // the big one's settings, where the first of them was.
        let r = d
            .simplify_design(&SimplifyDesign {
                unite: Unite::All,
                group: true,
                ..SimplifyDesign::default()
            })
            .unwrap();
        assert_eq!(r.united, 1);
        assert!(r.changed());
        assert_eq!(d.shapes.len(), 3);
        assert_eq!(d.shapes[0].color, red);
        assert_eq!(d.shapes[0].params.angle, Some(30.0));
        // 20 x 20 mm and 10 x 10 mm, 5 x 10 mm of it shared: 450 mm².
        assert!((area_mm2(&d, 0) - 450.0).abs() < 2.0, "{}", area_mm2(&d, 0));
        // The red on its own is still on its own, and the blue still there.
        let uids: Vec<usize> = d.shapes.iter().map(|s| s.uid).collect();
        assert_eq!(uids[1..], [alone_uid, far_uid]);
        // Less to sew: the overlap is sewn once.
        assert!(d.plan().stitch_count() < plain);
        // It's what's saved, order and all.
        let back = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        assert_eq!(back.shapes.len(), 3);
        assert_eq!(back.shapes[0].params.angle, Some(30.0));
        assert!((area_mm2(&back, 0) - 450.0).abs() < 2.0);
        assert_eq!(back.shapes[2].color, [0, 0, 200]);
        // Again: nothing left to unite.
        let r = d
            .simplify_design(&SimplifyDesign {
                unite: Unite::All,
                group: true,
                ..SimplifyDesign::default()
            })
            .unwrap();
        assert!(!r.changed(), "{r:?}");
    }

    #[test]
    fn a_united_shape_keeps_its_name_its_group_and_its_holes() {
        // A C (a path) and a bar across its open side (a basic shape,
        // filled by the nonzero rule): together a frame, with a hole.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm" viewBox="0 0 40 40">
            <g id="Badge">
                <path id="Frame" d="M0 0H30V5H5V25H30V30H0Z" fill="#205080"/>
                <rect id="Bar" x="25" y="0" width="5" height="30" fill="#205080"/>
                <rect id="Dot" x="34" y="0" width="4" height="4" fill="#c00000"/>
            </g>
        </svg>"##;
        let mut d = Document::from_svg(svg, "t", None).unwrap();
        let r = d
            .simplify_design(&SimplifyDesign {
                unite: Unite::All,
                ..SimplifyDesign::default()
            })
            .unwrap();
        assert_eq!((r.united, d.shapes.len()), (1, 2));
        // The frame, by name, still in its group; 30 x 30 less the 20 x 20
        // hole, which is still a hole.
        let names = crate::svg::display_names(&d.shapes);
        assert_eq!(names, vec!["Frame", "Dot"]);
        assert_eq!(d.shapes[0].groups, d.shapes[1].groups);
        assert!(!d.shapes[0].groups.is_empty());
        let middle = {
            let mut b = crate::geom::Bounds::empty();
            if let Geometry::Fill { rings, .. } = &d.shapes[0].geometry {
                rings.iter().flatten().for_each(|&p| b.add(p));
            }
            b.center()
        };
        match &d.shapes[0].geometry {
            Geometry::Fill { rings, rule } => {
                assert_eq!(rings.len(), 2, "an outline and a hole");
                assert!(!crate::geom::point_inside(rings, *rule, middle));
            }
            g => panic!("{g:?}"),
        }
        let back = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        assert_eq!(
            crate::svg::display_names(&back.shapes),
            vec!["Frame", "Dot"]
        );
        match &back.shapes[0].geometry {
            Geometry::Fill { rings, rule } => {
                assert!(!crate::geom::point_inside(rings, *rule, middle))
            }
            g => panic!("{g:?}"),
        }
    }

    #[test]
    fn similar_unites_only_what_is_sewn_the_same_way() {
        let brown = [150, 90, 50];
        let mut d = Document::empty();
        // A face, a cheek over its edge, and a brow across both, all in
        // one thread; the brow a satin.
        let face = square(&mut d, 0.0, 200.0, brown);
        let cheek = square(&mut d, 150.0, 100.0, brown);
        let brow = square(&mut d, 100.0, 60.0, brown);
        d.shapes[brow].params.kind = Some(Kind::Satin);
        let brow_uid = d.shapes[brow].uid;
        let _ = (face, cheek);
        let mut all = d.clone();
        // Similar: the two fills are one; the satin is still a satin, and
        // still sews after them.
        let r = d
            .simplify_design(&SimplifyDesign {
                unite: Unite::Similar,
                ..SimplifyDesign::default()
            })
            .unwrap();
        assert_eq!((r.united, d.shapes.len()), (1, 2));
        assert_eq!(d.shapes[0].params.kind.unwrap_or(Kind::Fill), Kind::Fill);
        assert!((area_mm2(&d, 0) - 450.0).abs() < 2.0);
        assert_eq!(d.shapes[1].uid, brow_uid);
        assert_eq!(d.shapes[1].params.kind, Some(Kind::Satin));
        // All: the satin goes into the fill too.
        let r = all
            .simplify_design(&SimplifyDesign {
                unite: Unite::All,
                ..SimplifyDesign::default()
            })
            .unwrap();
        assert_eq!((r.united, all.shapes.len()), (2, 1));
        assert_eq!(all.shapes[0].params.kind.unwrap_or(Kind::Fill), Kind::Fill);
        // Two satins that touch are alike, and unite as a satin.
        let mut d = Document::empty();
        for x in [0.0, 50.0] {
            let i = square(&mut d, x, 60.0, brown);
            d.shapes[i].params.kind = Some(Kind::Satin);
        }
        let r = d
            .simplify_design(&SimplifyDesign {
                unite: Unite::Similar,
                ..SimplifyDesign::default()
            })
            .unwrap();
        assert_eq!((r.united, d.shapes.len()), (1, 1));
        assert_eq!(d.shapes[0].params.kind, Some(Kind::Satin));
        assert_eq!(Unite::by_id("similar"), Some(Unite::Similar));
        assert_eq!(Unite::by_id("some"), None);
    }

    #[test]
    fn a_layer_in_another_thread_between_keeps_them_apart() {
        let red = [200, 0, 0];
        let mut d = Document::empty();
        // Red, blue over part of it, red over part of the blue: the top
        // red can't sew before the blue, so the reds stay two layers.
        square(&mut d, 0.0, 200.0, red);
        square(&mut d, 150.0, 200.0, [0, 0, 200]);
        square(&mut d, 100.0, 120.0, red);
        let r = d
            .simplify_design(&SimplifyDesign {
                unite: Unite::All,
                group: true,
                ..SimplifyDesign::default()
            })
            .unwrap();
        assert_eq!((r.united, d.shapes.len()), (0, 3));
        let colors: Vec<[u8; 3]> = d.shapes.iter().map(|s| s.color).collect();
        assert_eq!(colors, vec![red, [0, 0, 200], red]);
    }

    #[test]
    fn uniting_inside_a_group_keeps_the_sewing_order() {
        // (What a traced or drawn design looks like: everything in a group.)
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="30mm" viewBox="0 0 60 30">
            <rect id="Back" x="0" y="0" width="60" height="30" fill="#7030a0"/>
            <g id="Face">
                <rect id="Skin" x="5" y="5" width="30" height="20" fill="#a06030"/>
                <rect id="Cheek" x="20" y="10" width="20" height="8" fill="#a8683a"/>
                <rect id="Eye" x="10" y="8" width="4" height="4" fill="#101010"/>
            </g>
        </svg>"##;
        let mut d = Document::from_svg(svg, "t", None).unwrap();
        assert_eq!(d.shapes.len(), 4);
        let r = d
            .simplify_design(&SimplifyDesign {
                threads: Some(3),
                unite: Unite::All,
                group: true,
                ..SimplifyDesign::default()
            })
            .unwrap();
        // The cheek took the skin's thread and went into it; the face is
        // still over the back and under the eye.
        assert_eq!(r.merged, vec![([168, 104, 58], [160, 96, 48])]);
        assert_eq!(r.united, 1);
        let look: Vec<[u8; 3]> = d.shapes.iter().map(|s| s.color).collect();
        assert_eq!(look, vec![[112, 48, 160], [160, 96, 48], [16, 16, 16]]);
        assert!((area_mm2(&d, 1) - (600.0 + 5.0 * 8.0)).abs() < 3.0);
        let back = Document::from_svg(&d.to_svg().unwrap(), "t", None).unwrap();
        let look: Vec<[u8; 3]> = back.shapes.iter().map(|s| s.color).collect();
        assert_eq!(look, vec![[112, 48, 160], [160, 96, 48], [16, 16, 16]]);
    }

    #[test]
    fn what_isnt_sewn_keeps_its_color() {
        let mut d = design();
        d.shapes[2].params.kind = Some(Kind::Skip);
        d.shapes[3].params.hide = Some(true);
        assert_eq!(d.threads().len(), 3);
        d.simplify_design(&SimplifyDesign {
            threads: Some(1),
            ..SimplifyDesign::default()
        })
        .unwrap();
        assert_eq!(d.shapes[2].color, [215, 20, 10]);
        assert_eq!(d.shapes[3].color, [10, 20, 180]);
        assert_eq!(d.shapes[4].color, [200, 0, 0]);
    }
}
