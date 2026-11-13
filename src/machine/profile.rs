//! Machine profiles: what stays the same from design to design because the
//! machine does. A profile is a name for a bundle of ordinary settings (the
//! hoops the machine has, the file it reads, the shortest stitch it sews
//! and how it wants the thread locked), nothing a machine gets code
//! of its own for: the ones built in ([`builtin`]) are a data file in the
//! same form as the ones people make themselves.
//!
//! Choosing a machine copies its settings into the document
//! ([`Profile::apply`]), which keeps the machine's id only as a label. So a
//! design opens the same on a computer that has never heard of the machine.

use crate::document::{DEFAULT_MIN_STITCH_MM, MIN_STITCH_MM, Settings, format_hoop, parse_hoop};
use crate::formats::{Lock, LockStyle, MACHINE_FORMATS};
use serde_json::{Value, json};
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    /// The name files and agents write: lowercase letters, digits and
    /// dashes (see [`slug`]).
    pub id: String,
    pub name: String,
    /// The sewing fields of the machine's hoops, mm (never empty).
    pub hoops: Vec<(f64, f64)>,
    /// The hoop a new design starts in: one of `hoops`.
    pub default_hoop: (f64, f64),
    /// The machine format it reads (one of [`MACHINE_FORMATS`]), if known.
    pub format: Option<&'static str>,
    /// The shortest stitch it sews, mm, if it drops shorter ones.
    pub min_stitch_mm: Option<f64>,
    /// The lock stitches it wants, if not the usual ones.
    pub lock: Option<Lock>,
    /// How ZigZag sends a design straight to it (one of [`LINKS`]), if it
    /// does: the rest take a file.
    pub link: Option<&'static str>,
    /// Where the values come from.
    pub source: String,
    /// Someone has sewn on this machine from ZigZag with these values.
    pub tested: bool,
}

/// Whether two hoop sizes are the same hoop.
pub fn same_hoop(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 0.005 && (a.1 - b.1).abs() < 0.005
}

/// The ways ZigZag sends a design straight to a machine instead of by a
/// file: what files and agents call each, and what people do. (A machine
/// with one is offered Send; one without isn't.)
pub const LINKS: [(&str, &str); 1] = [("skitch", "Bluetooth (Brother Skitch)")];

/// The link named `name` (any case), as [`LINKS`] has it.
pub fn machine_link(name: &str) -> Option<&'static str> {
    LINKS
        .iter()
        .map(|l| l.0)
        .find(|l| l.eq_ignore_ascii_case(name))
}

/// The machine format named `ext` (any case), as [`MACHINE_FORMATS`] has it.
pub fn machine_format(ext: &str) -> Option<&'static str> {
    MACHINE_FORMATS
        .iter()
        .map(|f| f.0)
        .find(|f| f.eq_ignore_ascii_case(ext))
}

/// Whether `id` can name a machine in a file: what [`slug`] makes.
pub fn is_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// An id for a machine called `name`: its letters and digits in lowercase,
/// everything else a dash.
pub fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.is_empty() && !s.ends_with('-') {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-');
    if s.is_empty() {
        "machine".into()
    } else {
        s.into()
    }
}

impl Profile {
    /// A machine of your own with one hoop.
    pub fn new(id: String, name: &str, hoop: (f64, f64)) -> Profile {
        Profile {
            id,
            name: name.trim().to_string(),
            hoops: vec![hoop],
            default_hoop: hoop,
            format: None,
            min_stitch_mm: None,
            lock: None,
            link: None,
            source: String::new(),
            tested: false,
        }
    }

    /// Read one machine. `None` without an id, a name or a hoop; a format
    /// ZigZag doesn't write is left out, and a default hoop the machine
    /// doesn't have is its first.
    pub fn from_json(v: &Value) -> Option<Profile> {
        let id = v["id"].as_str().filter(|s| is_id(s))?.to_string();
        let name = v["name"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        let mut hoops: Vec<(f64, f64)> = Vec::new();
        for h in v["hoops"].as_array()?.iter().filter_map(Value::as_str) {
            if let Some(h) = parse_hoop(h)
                && !hoops.iter().any(|&k| same_hoop(k, h))
            {
                hoops.push(h);
            }
        }
        let first = *hoops.first()?;
        let default_hoop = v["default_hoop"]
            .as_str()
            .and_then(parse_hoop)
            .filter(|&d| hoops.iter().any(|&h| same_hoop(h, d)))
            .unwrap_or(first);
        Some(Profile {
            id,
            name: name.to_string(),
            hoops,
            default_hoop,
            format: v["format"].as_str().and_then(machine_format),
            min_stitch_mm: v["min_stitch_mm"]
                .as_f64()
                .filter(|m| MIN_STITCH_MM.contains(m)),
            lock: lock_from_json(&v["lock"]),
            link: v["link"].as_str().and_then(machine_link),
            source: v["source"].as_str().unwrap_or("").to_string(),
            tested: v["tested"].as_bool().unwrap_or(false),
        })
    }

    /// [`Profile::from_json`]'s inverse.
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "id": self.id,
            "name": self.name,
            "hoops": self.hoops.iter().map(|&h| format_hoop(h)).collect::<Vec<_>>(),
            "default_hoop": format_hoop(self.default_hoop),
        });
        if let Some(f) = self.format {
            v["format"] = json!(f);
        }
        if let Some(m) = self.min_stitch_mm {
            v["min_stitch_mm"] = json!(m);
        }
        if let Some(l) = self.lock {
            v["lock"] = lock_json(l);
        }
        if let Some(l) = self.link {
            v["link"] = json!(l);
        }
        if !self.source.is_empty() {
            v["source"] = json!(self.source);
        }
        if self.tested {
            v["tested"] = json!(true);
        }
        v
    }

    /// Whether the machine has a hoop of this size.
    pub fn has_hoop(&self, hoop: (f64, f64)) -> bool {
        self.hoops.iter().any(|&h| same_hoop(h, hoop))
    }

    /// Make this the document's machine. With `keep_hoop` a hoop the
    /// machine has stays (a design already laid out in it); otherwise, and
    /// for a hoop it hasn't, the document moves to the machine's default.
    /// The shortest stitch and the locks become the machine's (the usual
    /// ones for a machine that doesn't say).
    pub fn apply(&self, s: &mut Settings, keep_hoop: bool) {
        s.machine = Some(self.id.clone());
        if !(keep_hoop && self.has_hoop(s.hoop)) {
            s.hoop = self.default_hoop;
        }
        s.min_stitch_mm = self.min_stitch_mm.unwrap_or(DEFAULT_MIN_STITCH_MM);
        s.lock = self.lock.unwrap_or_default();
    }
}

/// A lock as agents and machine files write it: its style, and (unless
/// there's none) each stitch's length and how many.
pub fn lock_json(l: Lock) -> Value {
    match l.style {
        LockStyle::None => json!({ "style": "none" }),
        s => json!({ "style": s.name(), "length_mm": l.len_mm, "count": l.count }),
    }
}

/// [`lock_json`]'s inverse; a style's name alone will do. What's left out
/// is as in `base` (the usual lock, if none is given).
pub fn lock_from(v: &Value, base: Lock) -> Option<Lock> {
    if let Some(s) = v.as_str() {
        return Lock::parse(s);
    }
    let style = match v.get("style") {
        Some(s) => LockStyle::parse(s.as_str()?)?,
        None if v.is_object() => base.style,
        None => return None,
    };
    let len = match v.get("length_mm") {
        Some(l) => l.as_f64()?,
        None => base.len_mm,
    };
    let count = match v.get("count") {
        Some(c) => u32::try_from(c.as_u64()?).ok()?,
        None => base.count,
    };
    Some(Lock::new(style, len, count))
}

fn lock_from_json(v: &Value) -> Option<Lock> {
    lock_from(v, Lock::default())
}

/// The machines in a `{"machines": [...]}` file, leaving out any that
/// can't be read and any whose id an earlier one has.
pub fn parse_list(v: &Value) -> Vec<Profile> {
    let mut out: Vec<Profile> = Vec::new();
    for p in v["machines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Profile::from_json)
    {
        if !out.iter().any(|o| o.id == p.id) {
            out.push(p);
        }
    }
    out
}

/// The machines ZigZag comes with.
pub fn builtin() -> &'static [Profile] {
    static BUILTIN: OnceLock<Vec<Profile>> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        serde_json::from_str::<Value>(include_str!("profiles.json"))
            .map(|v| parse_list(&v))
            .unwrap_or_default()
    })
}

/// The built-in machine with this id, or (any case) this name.
pub fn builtin_named(name: &str) -> Option<&'static Profile> {
    let n = name.trim();
    builtin()
        .iter()
        .find(|p| p.id == n)
        .or_else(|| builtin().iter().find(|p| p.name.eq_ignore_ascii_case(n)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_machine_reads_whole() {
        let raw: Value = serde_json::from_str(include_str!("profiles.json")).unwrap();
        let listed = raw["machines"].as_array().unwrap();
        // None dropped for a bad id, name or hoop, or for a repeated id.
        assert_eq!(builtin().len(), listed.len());
        assert!(builtin().len() >= 5);
        for (p, v) in builtin().iter().zip(listed) {
            assert_eq!(p.id, slug(&p.id), "{}", p.id);
            assert!(p.has_hoop(p.default_hoop), "{}", p.id);
            // Every hoop written was a size that reads, and none repeats.
            assert_eq!(
                p.hoops.len(),
                v["hoops"].as_array().unwrap().len(),
                "{}",
                p.id
            );
            assert_eq!(
                v["default_hoop"].as_str().and_then(parse_hoop),
                Some(p.default_hoop),
                "{}",
                p.id
            );
            // A format that's named is one ZigZag writes.
            assert_eq!(v["format"].as_str(), p.format, "{}", p.id);
            assert!(
                !p.source.is_empty(),
                "{} says where its values come from",
                p.id
            );
        }
        // Only what has been sewn from ZigZag says so.
        let tested: Vec<&str> = builtin()
            .iter()
            .filter(|p| p.tested)
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(tested, ["brother-skitch"]);
    }

    #[test]
    fn a_machine_round_trips_and_bad_ones_are_left_out() {
        let mut p = Profile::new(slug("Shop's Melco #2"), " Shop's Melco #2 ", (300.0, 210.5));
        assert_eq!(p.id, "shop-s-melco-2");
        assert_eq!(p.name, "Shop's Melco #2");
        p.hoops.push((150.0, 150.0));
        p.format = machine_format("EXP");
        assert_eq!(p.format, Some("exp"));
        assert_eq!(Profile::from_json(&p.to_json()), Some(p.clone()));
        p.min_stitch_mm = Some(0.4);
        p.lock = Some(Lock::new(LockStyle::Triangle, 0.5, 3));
        assert_eq!(Profile::from_json(&p.to_json()), Some(p.clone()));
        p.lock = Some(Lock::new(LockStyle::None, 0.4, 4));
        assert_eq!(Profile::from_json(&p.to_json()), Some(p.clone()));
        // A shortest stitch out of range is none.
        let mut v = p.to_json();
        v["min_stitch_mm"] = json!(7.0);
        assert_eq!(Profile::from_json(&v).unwrap().min_stitch_mm, None);
        // A default hoop the machine hasn't is its first; a format ZigZag
        // doesn't write is none.
        let mut v = p.to_json();
        v["default_hoop"] = json!("90x90");
        v["format"] = json!("xxx");
        let q = Profile::from_json(&v).unwrap();
        assert_eq!(q.default_hoop, (300.0, 210.5));
        assert_eq!(q.format, None);
        // No hoop that reads, no machine; nor with an id a file can't hold.
        v["hoops"] = json!(["5x5", "big"]);
        assert_eq!(Profile::from_json(&v), None);
        let mut v = p.to_json();
        v["id"] = json!("two words");
        assert_eq!(Profile::from_json(&v), None);
        // The first of two with one id is the one kept.
        let two = json!({ "machines": [p.to_json(), p.to_json()] });
        assert_eq!(parse_list(&two).len(), 1);
        assert_eq!(slug("  ¡!  "), "machine");
    }

    #[test]
    fn applying_a_machine_sets_the_label_and_the_hoop() {
        let p = builtin_named("brother-5x7").unwrap();
        assert_eq!(builtin_named("BROTHER 5×7 (se2000, pe900)"), Some(p));
        // A new design starts in the machine's default hoop.
        let mut s = Settings::default();
        p.apply(&mut s, false);
        assert_eq!(
            (s.machine.as_deref(), s.hoop),
            (Some("brother-5x7"), (130.0, 180.0))
        );
        // A design in a hoop the machine has stays in it.
        let mut s = Settings::default();
        p.apply(&mut s, true);
        assert_eq!(s.hoop, (100.0, 100.0));
        // One in a hoop it hasn't moves to the default.
        s.hoop = (200.0, 300.0);
        p.apply(&mut s, true);
        assert_eq!(s.hoop, (130.0, 180.0));
        // A machine that drops short stitches says how short; the next
        // machine, which doesn't, puts the usual back.
        let melco = builtin_named("melco-summit").unwrap();
        melco.apply(&mut s, false);
        assert_eq!(s.min_stitch_mm, 0.4);
        s.lock = Lock::new(LockStyle::Triangle, 0.6, 3);
        p.apply(&mut s, false);
        assert_eq!(
            (s.min_stitch_mm, s.lock),
            (DEFAULT_MIN_STITCH_MM, Lock::default())
        );
    }
}
