//! Per-shape stitch parameters, read from SVG element and group ids, and the
//! fabric presets that supply their defaults.
//!
//! Illustrator turns layer and object names into `id`s, so naming a layer
//! `satin w=3` or `fill a=45 s=0.3` is how parameters are attached. Illustrator
//! escapes characters in ids (`satin_w_x3D_3`), which is decoded here.
//! Parameters on a group apply to everything inside it; children override.
//!
//! Tokens (separated by spaces, underscores, commas or semicolons):
//!   fill | satin | run | contour | blanket | applique | meander | ripple | cross | tartan | manual | skip   stitch type
//!   a=DEG      fill angle
//!   s=MM       row / zigzag spacing
//!   l=MM       max stitch length
//!   u=0|1      underlay on/off
//!   ul=LIST    satin underlay: center+contour+zigzag, or none
//!   pc=MM      pull compensation per side
//!   r=N        running-stitch passes (3 = bean)
//!   w=MM       satin width (overrides stroke width)
//!   border     satin on a filled shape follows its outline instead of filling it
//!   spiral     contour fill as one spiral from the center (Ink/Stitch's circular fill)
//!   style=S    satin top stitches: satin, s (S-stitch) or zigzag
//!   colors=LIST  multicolor satin: thread colors across the column (RRGGBB+RRGGBB…)
//!   sett=LIST  tartan stripes: COLOR:MM+COLOR:MM… (mirrored to a symmetric sett)
//!   cell=MM    cross-stitch cell size
//!   ss=0|1     short stitches on tight satin curves
//!   t=RRGGBB   thread color (overrides the SVG paint)
//!   o=N        sewing order
//!   st=T       start sewing at this point of the outline (0–1 along it)
//!   en=T       end sewing at this point of the outline
//!   trim       cut the thread after this object
//!   stop       pause the machine after this object (add foam, trim appliqué)
//! A `stroke.` prefix (e.g. `stroke.run`) applies a token only to the stroke of
//! an element that has both a fill and a stroke.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Fill,
    Satin,
    Run,
    Skip,
    /// Concentric rows that follow the outline (Hatch "contour fill").
    Contour,
    /// Blanket / E-stitch edge: a run with prongs into the shape.
    Blanket,
    /// Fabric appliqué: placement line, stop, tack-down, stop, satin cover.
    Applique,
    /// Every path vertex is a needle point (imported machine files, hand digitizing).
    Manual,
    /// One wandering line through the shape (quilting-style meander).
    Meander,
    /// Open running-stitch rows following the outline (or one spiral).
    Ripple,
    /// A grid of X's.
    Cross,
    /// Woven stripes in several threads.
    Tartan,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Fill => "fill",
            Kind::Satin => "satin",
            Kind::Run => "run",
            Kind::Skip => "skip",
            Kind::Contour => "contour",
            Kind::Blanket => "blanket",
            Kind::Applique => "applique",
            Kind::Manual => "manual",
            Kind::Meander => "meander",
            Kind::Ripple => "ripple",
            Kind::Cross => "cross",
            Kind::Tartan => "tartan",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        match s.to_ascii_lowercase().as_str() {
            "fill" | "tatami" => Some(Kind::Fill),
            "satin" => Some(Kind::Satin),
            "run" | "running" => Some(Kind::Run),
            "skip" | "off" | "nostitch" => Some(Kind::Skip),
            "contour" => Some(Kind::Contour),
            "blanket" | "estitch" => Some(Kind::Blanket),
            "applique" | "appliqué" => Some(Kind::Applique),
            "manual" => Some(Kind::Manual),
            "meander" => Some(Kind::Meander),
            "ripple" => Some(Kind::Ripple),
            "cross" | "crossstitch" | "cross-stitch" => Some(Kind::Cross),
            "tartan" | "plaid" => Some(Kind::Tartan),
            _ => None,
        }
    }
}

/// Satin underlay passes (bit flags).
pub mod underlay {
    pub const CENTER: u8 = 1;
    pub const CONTOUR: u8 = 2;
    pub const ZIGZAG: u8 = 4;

    pub fn parse(v: &str) -> Option<u8> {
        if v == "none" || v == "0" {
            return Some(0);
        }
        let mut m = 0;
        for part in v.split(['+', '|', '/']) {
            m |= match part {
                "center" | "c" => CENTER,
                "contour" | "edge" | "e" => CONTOUR,
                "zigzag" | "z" => ZIGZAG,
                _ => return None,
            };
        }
        Some(m)
    }

    pub fn format(m: u8) -> String {
        if m == 0 {
            return "none".into();
        }
        let mut v = Vec::new();
        for (bit, name) in [(CENTER, "center"), (CONTOUR, "contour"), (ZIGZAG, "zigzag")] {
            if m & bit != 0 {
                v.push(name);
            }
        }
        v.join("+")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Params {
    pub kind: Option<Kind>,
    pub angle: Option<f64>,
    pub spacing: Option<f64>,
    pub len: Option<f64>,
    pub underlay: Option<bool>,
    pub satin_underlay: Option<u8>,
    pub pull: Option<f64>,
    pub repeats: Option<u32>,
    pub width: Option<f64>,
    pub border: Option<bool>,
    pub short_stitches: Option<bool>,
    pub thread: Option<[u8; 3]>,
    pub order: Option<u32>,
    /// Satin auto-split: stitches longer than this (mm) are split; 0 = off.
    pub split: Option<f64>,
    /// Where on the outline sewing starts and ends (0–1 along it; see
    /// [`crate::design::outline_at`]), like Ink/Stitch's start and end
    /// commands.
    pub start: Option<f64>,
    pub end: Option<f64>,
    /// Contour (and ripple) fill as a spiral out from the center, not rings.
    pub spiral: Option<bool>,
    /// Satin top-stitch pattern.
    pub style: Option<crate::stitch::satin::SatinStyle>,
    /// Multicolor satin: the threads across the column, first at rail a.
    pub colors: Option<Vec<[u8; 3]>>,
    /// Tartan stripes (color, width mm), mirrored into a symmetric sett.
    pub sett: Option<Vec<([u8; 3], f64)>>,
    /// Cross-stitch cell size, mm.
    pub cell: Option<f64>,
    /// Cut the thread after this object, whatever comes next.
    pub trim: Option<bool>,
    /// Pause the machine after this object (same thread, new color block).
    pub stop: Option<bool>,
}

/// Fabric presets: the defaults every stitch setting falls back to. Picking the
/// right one matters more than any single slider.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fabric {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub fill_spacing: f64,
    pub fill_len: f64,
    pub fill_pull: f64,
    /// Run an inset outline before the fill (stops edges from curling).
    pub fill_edge_walk: bool,
    /// Spacing of the perpendicular fill underlay rows, mm.
    pub fill_underlay_spacing: f64,
    pub satin_spacing: f64,
    pub satin_pull: f64,
    pub run_len: f64,
}

pub const FABRICS: [Fabric; 4] = [
    Fabric {
        id: "standard",
        name: "Standard (twill, denim, patches)",
        about: "Stable woven fabric. Classic densities.",
        fill_spacing: 0.25,
        fill_len: 3.0,
        fill_pull: 0.0,
        fill_edge_walk: false,
        fill_underlay_spacing: 1.5,
        satin_spacing: 0.4,
        satin_pull: 0.15,
        run_len: 2.5,
    },
    Fabric {
        id: "thin-cap",
        name: "Thin cotton cap (unstructured)",
        about: "Light, flexible crown. Lighter fill, more pull compensation, edge walk to keep shapes flat.",
        fill_spacing: 0.35,
        fill_len: 3.0,
        fill_pull: 0.2,
        fill_edge_walk: true,
        fill_underlay_spacing: 2.0,
        satin_spacing: 0.42,
        satin_pull: 0.2,
        run_len: 2.2,
    },
    Fabric {
        id: "structured-cap",
        name: "Structured cap (buckram front)",
        about: "Stiff front panel. Medium density.",
        fill_spacing: 0.3,
        fill_len: 3.5,
        fill_pull: 0.15,
        fill_edge_walk: true,
        fill_underlay_spacing: 1.8,
        satin_spacing: 0.4,
        satin_pull: 0.2,
        run_len: 2.5,
    },
    Fabric {
        id: "knit",
        name: "Knit (beanie, jersey)",
        about: "Stretchy. Strong underlay and pull compensation so stitches don't sink.",
        fill_spacing: 0.3,
        fill_len: 2.8,
        fill_pull: 0.3,
        fill_edge_walk: true,
        fill_underlay_spacing: 1.2,
        satin_spacing: 0.38,
        satin_pull: 0.3,
        run_len: 2.0,
    },
];

impl Fabric {
    pub fn by_id(id: &str) -> Option<&'static Fabric> {
        FABRICS.iter().find(|f| f.id == id)
    }
    pub fn standard() -> &'static Fabric {
        &FABRICS[0]
    }
}

/// Fixed defaults that don't depend on fabric (mm / degrees).
pub mod defaults {
    pub const FILL_ANGLE: f64 = 0.0;
    /// Width of a satin border that follows a filled shape's outline.
    pub const OUTLINE_SATIN_WIDTH: f64 = 2.0;
    pub const RUN_REPEATS: u32 = 1;
    pub const UNDERLAY: bool = true;
    pub const SHORT_STITCHES: bool = true;
    /// Satin stitches longer than this are split (mm).
    pub const SATIN_SPLIT: f64 = 7.0;
    pub const CONTOUR_SPACING: f64 = 0.45;
    pub const BLANKET_SPACING: f64 = 2.5;
    pub const BLANKET_WIDTH: f64 = 2.5;
    pub const APPLIQUE_WIDTH: f64 = 3.0;
    pub const CROSS_CELL: f64 = 2.5;
    pub const MEANDER_SPACING: f64 = 3.0;
    pub const RIPPLE_SPACING: f64 = 2.5;
}

impl Params {
    pub fn fill_angle(&self) -> f64 {
        self.angle.unwrap_or(defaults::FILL_ANGLE)
    }
    pub fn fill_spacing(&self, f: &Fabric) -> f64 {
        self.spacing.unwrap_or(f.fill_spacing)
    }
    pub fn fill_len(&self, f: &Fabric) -> f64 {
        self.len.unwrap_or(f.fill_len)
    }
    pub fn fill_pull(&self, f: &Fabric) -> f64 {
        self.pull.unwrap_or(f.fill_pull)
    }
    pub fn satin_spacing(&self, f: &Fabric) -> f64 {
        self.spacing.unwrap_or(f.satin_spacing)
    }
    /// Auto spacing: unless set explicitly, narrow columns are sewn more
    /// openly (they'd pile up) and wide ones a little denser.
    pub fn satin_spacing_for(&self, f: &Fabric, width_mm: f64) -> f64 {
        if let Some(s) = self.spacing {
            return s;
        }
        let k = if width_mm < 1.5 {
            1.25
        } else if width_mm < 2.5 {
            1.1
        } else if width_mm > 5.0 {
            0.92
        } else {
            1.0
        };
        f.satin_spacing * k
    }
    /// Longest satin stitch before it's split, mm (`None` = never split).
    pub fn satin_split(&self) -> Option<f64> {
        let v = self.split.unwrap_or(defaults::SATIN_SPLIT);
        (v > 0.0).then_some(v.max(3.0))
    }
    pub fn contour_spacing(&self) -> f64 {
        self.spacing.unwrap_or(defaults::CONTOUR_SPACING)
    }
    pub fn satin_pull(&self, f: &Fabric) -> f64 {
        self.pull.unwrap_or(f.satin_pull)
    }
    pub fn run_len(&self, f: &Fabric) -> f64 {
        self.len.unwrap_or(f.run_len)
    }
    pub fn run_repeats(&self) -> u32 {
        self.repeats.unwrap_or(defaults::RUN_REPEATS)
    }
    pub fn underlay_on(&self) -> bool {
        self.underlay.unwrap_or(defaults::UNDERLAY)
    }
    pub fn short_stitches_on(&self) -> bool {
        self.short_stitches.unwrap_or(defaults::SHORT_STITCHES)
    }
    pub fn is_border(&self) -> bool {
        self.border.unwrap_or(false)
    }
    pub fn satin_style(&self) -> crate::stitch::satin::SatinStyle {
        self.style.unwrap_or_default()
    }
    /// Cross-stitch cell, mm.
    pub fn cross_cell(&self) -> f64 {
        self.cell.unwrap_or(defaults::CROSS_CELL)
    }
    /// Meander line spacing, mm.
    pub fn meander_spacing(&self) -> f64 {
        self.spacing.unwrap_or(defaults::MEANDER_SPACING)
    }
    /// Ripple row spacing, mm.
    pub fn ripple_spacing(&self) -> f64 {
        self.spacing.unwrap_or(defaults::RIPPLE_SPACING)
    }
    /// The tartan's stripes, or a default two-colour sett in the object's
    /// thread and a darker one.
    pub fn tartan_sett(&self, base: [u8; 3]) -> Vec<([u8; 3], f64)> {
        self.sett.clone().unwrap_or_else(|| {
            // A lighter stripe of the same hue (towards white).
            let light = [base[0] / 2 + 128, base[1] / 2 + 128, base[2] / 2 + 128];
            vec![(base, 4.0), (light, 2.0)]
        })
    }
    /// Satin underlay passes for a column of `width_mm`: explicit setting, or
    /// none under 1.5 mm, center walk up to 4 mm, contour + zigzag above.
    pub fn satin_underlay_for(&self, width_mm: f64) -> u8 {
        if !self.underlay_on() {
            return 0;
        }
        self.satin_underlay.unwrap_or(if width_mm < 1.5 {
            0
        } else if width_mm <= 4.0 {
            underlay::CENTER
        } else {
            underlay::CONTOUR | underlay::ZIGZAG
        })
    }

    /// `self` is the parent; values set on `child` win.
    pub fn merged(&self, child: &Params) -> Params {
        Params {
            kind: child.kind.or(self.kind),
            angle: child.angle.or(self.angle),
            spacing: child.spacing.or(self.spacing),
            len: child.len.or(self.len),
            underlay: child.underlay.or(self.underlay),
            satin_underlay: child.satin_underlay.or(self.satin_underlay),
            pull: child.pull.or(self.pull),
            repeats: child.repeats.or(self.repeats),
            width: child.width.or(self.width),
            border: child.border.or(self.border),
            short_stitches: child.short_stitches.or(self.short_stitches),
            thread: child.thread.or(self.thread),
            order: child.order.or(self.order),
            split: child.split.or(self.split),
            start: child.start.or(self.start),
            end: child.end.or(self.end),
            spiral: child.spiral.or(self.spiral),
            style: child.style.or(self.style),
            colors: child.colors.clone().or_else(|| self.colors.clone()),
            sett: child.sett.clone().or_else(|| self.sett.clone()),
            cell: child.cell.or(self.cell),
            trim: child.trim.or(self.trim),
            stop: child.stop.or(self.stop),
        }
    }

    /// Parameters for one part of an element: tokens prefixed `stroke.` apply
    /// only when `stroke_part` is true, and override unprefixed ones.
    pub fn parse_part(id: &str, stroke_part: bool) -> Params {
        let decoded = decode_illustrator_id(id);
        let mut base = Params::default();
        let mut stroke = Params::default();
        for tok in decoded
            .split([' ', '_', ',', ';'])
            .filter(|t| !t.is_empty())
        {
            let lower = tok.to_ascii_lowercase();
            match lower.strip_prefix("stroke.") {
                Some(rest) => stroke.apply_token(rest),
                None => base.apply_token(&lower),
            }
        }
        if stroke_part {
            base.merged(&stroke)
        } else {
            base
        }
    }

    pub fn parse(id: &str) -> Params {
        Params::parse_part(id, false)
    }

    fn apply_token(&mut self, tok: &str) {
        if let Some(k) = Kind::parse(tok) {
            self.kind = Some(k);
            return;
        }
        match tok {
            "border" => {
                self.border = Some(true);
                return;
            }
            "trim" => {
                self.trim = Some(true);
                return;
            }
            "spiral" => {
                self.spiral = Some(true);
                return;
            }
            "stop" => {
                self.stop = Some(true);
                return;
            }
            _ => {}
        }
        let Some((k, v)) = tok.split_once('=') else {
            return;
        };
        let num = v.parse::<f64>().ok().filter(|n| n.is_finite());
        let flag = match v {
            "1" | "on" | "yes" | "true" => Some(true),
            "0" | "off" | "no" | "false" => Some(false),
            _ => None,
        };
        match k {
            "a" | "angle" => self.angle = num,
            "s" | "spacing" | "d" | "density" => self.spacing = num.filter(|v| *v > 0.05),
            "l" | "len" | "length" => self.len = num.filter(|v| *v > 0.3),
            "u" | "underlay" => self.underlay = flag,
            "ul" => self.satin_underlay = underlay::parse(v),
            "pc" | "pull" => self.pull = num.filter(|v| *v >= 0.0),
            "r" | "repeats" => self.repeats = num.map(|v| v.clamp(1.0, 9.0) as u32),
            "w" | "width" => self.width = num.filter(|v| *v > 0.0),
            "border" => self.border = flag,
            "ss" => self.short_stitches = flag,
            "t" | "thread" => self.thread = parse_hex(v),
            "o" | "order" => self.order = num.filter(|v| *v >= 0.0).map(|v| v as u32),
            "split" => self.split = num.filter(|v| *v >= 0.0),
            "st" | "start" => self.start = num.map(|v| v.rem_euclid(1.0)),
            "en" | "end" => self.end = num.map(|v| v.rem_euclid(1.0)),
            "trim" => self.trim = flag,
            "spiral" => self.spiral = flag,
            "style" => self.style = crate::stitch::satin::SatinStyle::parse(v),
            "colors" => {
                let cs: Option<Vec<[u8; 3]>> = v.split('+').map(parse_hex).collect();
                self.colors = cs.filter(|c| !c.is_empty());
            }
            "sett" => {
                let st: Option<Vec<([u8; 3], f64)>> = v
                    .split('+')
                    .map(|part| {
                        let (c, w) = part.split_once(':')?;
                        let w = w.parse::<f64>().ok().filter(|w| *w > 0.0)?;
                        Some((parse_hex(c)?, w))
                    })
                    .collect();
                self.sett = st.filter(|s| !s.is_empty());
            }
            "cell" => self.cell = num.filter(|v| *v >= 1.0),
            "stop" => self.stop = flag,
            _ => {}
        }
    }

    /// Canonical tokens for this parameter set (the inverse of `parse`).
    pub fn tokens(&self) -> Vec<String> {
        let mut t = Vec::new();
        if let Some(k) = self.kind {
            t.push(k.name().to_string());
        }
        if self.border == Some(true) {
            t.push("border".into());
        }
        if self.spiral == Some(true) {
            t.push("spiral".into());
        }
        if self.trim == Some(true) {
            t.push("trim".into());
        }
        if self.stop == Some(true) {
            t.push("stop".into());
        }
        let mut num = |k: &str, v: Option<f64>| {
            if let Some(v) = v {
                t.push(format!("{k}={}", fmt_num(v)));
            }
        };
        num("a", self.angle);
        num("s", self.spacing);
        num("l", self.len);
        num("pc", self.pull);
        num("w", self.width);
        num("r", self.repeats.map(|r| r as f64));
        num("split", self.split);
        num("st", self.start);
        num("en", self.end);
        num("cell", self.cell);
        if let Some(st) = self.style {
            t.push(format!("style={}", st.name()));
        }
        if let Some(cs) = &self.colors {
            t.push(format!(
                "colors={}",
                cs.iter()
                    .map(|c| format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
                    .collect::<Vec<_>>()
                    .join("+")
            ));
        }
        if let Some(sett) = &self.sett {
            t.push(format!(
                "sett={}",
                sett.iter()
                    .map(|(c, w)| format!("{:02x}{:02x}{:02x}:{}", c[0], c[1], c[2], fmt_num(*w)))
                    .collect::<Vec<_>>()
                    .join("+")
            ));
        }
        if let Some(u) = self.underlay {
            t.push(format!("u={}", u as u8));
        }
        if let Some(m) = self.satin_underlay {
            t.push(format!("ul={}", underlay::format(m)));
        }
        if let Some(s) = self.short_stitches {
            t.push(format!("ss={}", s as u8));
        }
        if let Some(c) = self.thread {
            t.push(format!("t={:02x}{:02x}{:02x}", c[0], c[1], c[2]));
        }
        if let Some(o) = self.order {
            t.push(format!("o={o}"));
        }
        t
    }
}

pub fn fmt_num(v: f64) -> String {
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

pub fn parse_hex(v: &str) -> Option<[u8; 3]> {
    let h = v.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?])
}

/// True for tokens that carry settings (as opposed to plain name words).
pub fn is_param_token(tok: &str) -> bool {
    let lower = tok.to_ascii_lowercase();
    let lower = lower.strip_prefix("stroke.").unwrap_or(&lower);
    Kind::parse(lower).is_some() || lower == "border" || lower.contains('=')
}

/// Decode Illustrator's `_xHH_` escapes (e.g. `_x3D_` → `=`, `_x2E_` → `.`).
pub fn decode_illustrator_id(id: &str) -> String {
    let b = id.as_bytes();
    let mut out = String::with_capacity(id.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'_'
            && b.get(i + 1) == Some(&b'x')
            && b.get(i + 4) == Some(&b'_')
            && let Some(v) = id
                .get(i + 2..i + 4)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v as char);
            i += 5;
            continue;
        }
        let c = id[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_illustrator_ids() {
        let p = Params::parse("satin w=3 pc=0.2");
        assert_eq!(p.kind, Some(Kind::Satin));
        assert_eq!(p.width, Some(3.0));
        assert_eq!(p.pull, Some(0.2));

        let p = Params::parse("fill_a_x3D_45_s_x3D_0_x2E_3");
        assert_eq!(p.kind, Some(Kind::Fill));
        assert_eq!(p.angle, Some(45.0));
        assert_eq!(p.spacing, Some(0.3));

        let p = Params::parse("Layer_1");
        assert_eq!(p, Params::default());
    }

    #[test]
    fn child_overrides_parent() {
        let parent = Params::parse("fill a=30 u=0");
        let child = Params::parse("a=60");
        let m = parent.merged(&child);
        assert_eq!(m.angle, Some(60.0));
        assert_eq!(m.underlay, Some(false));
        assert_eq!(m.kind, Some(Kind::Fill));
    }

    #[test]
    fn tokens_round_trip() {
        let p = Params {
            kind: Some(Kind::Satin),
            angle: Some(-30.0),
            spacing: Some(0.35),
            width: Some(2.5),
            satin_underlay: Some(underlay::CENTER | underlay::ZIGZAG),
            short_stitches: Some(false),
            border: Some(true),
            thread: Some([0xe8, 0xa3, 0x3c]),
            order: Some(4),
            repeats: Some(3),
            split: Some(0.0),
            start: Some(0.25),
            end: Some(0.875),
            trim: Some(true),
            stop: Some(true),
            spiral: Some(true),
            style: Some(crate::stitch::satin::SatinStyle::SStitch),
            colors: Some(vec![[255, 0, 0], [0, 0, 255]]),
            sett: Some(vec![([10, 20, 30], 4.0), ([200, 200, 0], 1.5)]),
            cell: Some(3.0),
            ..Params::default()
        };
        assert_eq!(Params::parse(&p.tokens().join(" ")), p);
        assert_eq!(Params::parse(&p.tokens().join("_")), p);
    }

    #[test]
    fn stroke_prefixed_tokens_only_apply_to_stroke_part() {
        let id = "badge fill a=45 stroke.run stroke.l=2";
        assert_eq!(Params::parse_part(id, false).kind, Some(Kind::Fill));
        let s = Params::parse_part(id, true);
        assert_eq!(s.kind, Some(Kind::Run));
        assert_eq!(s.len, Some(2.0));
        assert_eq!(s.angle, Some(45.0));
        let illustrator = "badge_fill_stroke_x2E_run";
        assert_eq!(Params::parse_part(illustrator, true).kind, Some(Kind::Run));
    }

    #[test]
    fn satin_underlay_defaults_by_width() {
        let p = Params::default();
        assert_eq!(p.satin_underlay_for(1.0), 0);
        assert_eq!(p.satin_underlay_for(3.0), underlay::CENTER);
        assert_eq!(
            p.satin_underlay_for(6.0),
            underlay::CONTOUR | underlay::ZIGZAG
        );
        let off = Params {
            underlay: Some(false),
            ..Params::default()
        };
        assert_eq!(off.satin_underlay_for(6.0), 0);
    }
}
