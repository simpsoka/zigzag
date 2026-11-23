//! ZigZag's own stitch-plan file: a planned design and its export settings,
//! everything the machine-file writers read, kept exactly (`f64`s as
//! they are). The app uploads it beside a design so a server with no fonts
//! and no planner can write any machine format, byte for byte what the app
//! would write itself.
//!
//! Little-endian:
//!
//! ```text
//! "ZZPLAN" u8 version (1) u8 0
//! u32 name length, name (UTF-8)
//! u8 lock style (0 none, 1 back, 2 triangle), f64 lock mm, u32 lock count
//! f64 shortest stitch (units)
//! u32 blocks; each: u8 r, g, b, u32 runs; each: u8 trim before, u32 points, (f64 x, f64 y)…
//! ```
//!
//! Reading checks every length against what's left, so a hostile file
//! can't make it allocate more than its own size.

use super::{ExportOptions, Lock, LockStyle};
use crate::design::{Block, Design, Run};
use crate::geom::P;
use anyhow::{Result, bail};

const MAGIC: &[u8; 6] = b"ZZPLAN";
const VERSION: u8 = 1;
/// More stitches than any machine takes.
const MAX_POINTS: usize = 5_000_000;
/// Further from the middle than any hoop reaches (units of 0.1 mm: 10 m).
const MAX_COORD: f64 = 100_000.0;

/// The plan file for a planned design and the settings it's exported with.
pub fn write(d: &Design, opt: &ExportOptions) -> Vec<u8> {
    let points: usize = d
        .blocks
        .iter()
        .flat_map(|b| &b.runs)
        .map(|r| r.pts.len())
        .sum();
    let mut o = Vec::with_capacity(64 + d.name.len() + points * 16);
    o.extend_from_slice(MAGIC);
    o.extend_from_slice(&[VERSION, 0]);
    o.extend_from_slice(&(d.name.len() as u32).to_le_bytes());
    o.extend_from_slice(d.name.as_bytes());
    o.push(match opt.lock.style {
        LockStyle::None => 0,
        LockStyle::Back => 1,
        LockStyle::Triangle => 2,
    });
    o.extend_from_slice(&opt.lock.len_mm.to_le_bytes());
    o.extend_from_slice(&opt.lock.count.to_le_bytes());
    o.extend_from_slice(&opt.min_stitch.to_le_bytes());
    o.extend_from_slice(&(d.blocks.len() as u32).to_le_bytes());
    for b in &d.blocks {
        o.extend_from_slice(&b.color);
        o.extend_from_slice(&(b.runs.len() as u32).to_le_bytes());
        for r in &b.runs {
            o.push(r.trim_before as u8);
            o.extend_from_slice(&(r.pts.len() as u32).to_le_bytes());
            for p in &r.pts {
                o.extend_from_slice(&p.x.to_le_bytes());
                o.extend_from_slice(&p.y.to_le_bytes());
            }
        }
    }
    o
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        if self.b.len() - self.at < n {
            bail!("the plan file ends early");
        }
        let s = &self.b[self.at..self.at + n];
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64> {
        let v = f64::from_le_bytes(self.take(8)?.try_into().unwrap());
        if !v.is_finite() {
            bail!("the plan file has a number that isn't one");
        }
        Ok(v)
    }
    /// A count of items at least `each` bytes long: no more than could fit.
    fn count(&mut self, each: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(each) > self.b.len() - self.at {
            bail!("the plan file is damaged");
        }
        Ok(n)
    }
}

/// A plan file back into the design and its export settings.
pub fn read(bytes: &[u8]) -> Result<(Design, ExportOptions)> {
    let mut r = Reader { b: bytes, at: 0 };
    if r.take(6)? != MAGIC {
        bail!("not a ZigZag plan file");
    }
    let v = r.u8()?;
    if v != VERSION {
        bail!("plan file version {v} (this reads {VERSION})");
    }
    r.u8()?;
    let n = r.count(1)?;
    let name = String::from_utf8(r.take(n)?.to_vec())?;
    let style = match r.u8()? {
        0 => LockStyle::None,
        1 => LockStyle::Back,
        2 => LockStyle::Triangle,
        s => bail!("unknown lock style {s}"),
    };
    let len_mm = r.f64()?;
    let count = r.u32()?;
    if !Lock::LEN_MM.contains(&len_mm) || count > Lock::MAX_COUNT {
        bail!("the plan file's lock is out of range");
    }
    let lock = Lock {
        style,
        len_mm,
        count,
    };
    let min_stitch = r.f64()?;
    if !(0.0..=MAX_COORD).contains(&min_stitch) {
        bail!("the plan file's shortest stitch is out of range");
    }
    let mut points = 0usize;
    let mut blocks = Vec::new();
    for _ in 0..r.count(7)? {
        let t = r.take(3)?;
        let color = [t[0], t[1], t[2]];
        let mut runs = Vec::new();
        for _ in 0..r.count(5)? {
            let trim_before = match r.u8()? {
                0 => false,
                1 => true,
                _ => bail!("the plan file is damaged"),
            };
            let n = r.count(16)?;
            points += n;
            if points > MAX_POINTS {
                bail!("more stitches than a machine takes");
            }
            let mut pts = Vec::with_capacity(n);
            for _ in 0..n {
                let p = P {
                    x: r.f64()?,
                    y: r.f64()?,
                };
                if p.x.abs() > MAX_COORD || p.y.abs() > MAX_COORD {
                    bail!("the plan file has a stitch far outside any hoop");
                }
                pts.push(p);
            }
            runs.push(Run { pts, trim_before });
        }
        blocks.push(Block { color, runs });
    }
    if r.at != bytes.len() {
        bail!("the plan file has more after its end");
    }
    let d = Design {
        name,
        blocks,
        ..Design::default()
    };
    Ok((d, ExportOptions { lock, min_stitch }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="40mm" viewBox="0 0 60 40">
      <rect id="fill" x="5" y="5" width="30" height="20" fill="#cc2222"/>
      <circle id="dot" cx="45" cy="25" r="8" fill="#2255aa"/>
      <path d="M5 35 L55 35" stroke="#111111" stroke-width="1" fill="none"/>
    </svg>"##;

    #[test]
    fn every_machine_format_is_written_the_same_from_the_plan_file() {
        let doc = Document::from_svg(SVG, "badge", None).unwrap();
        let (d, opt) = (doc.plan(), doc.settings.export_options());
        let (back, opt2) = read(&write(&d, &opt)).unwrap();
        assert_eq!(opt, opt2);
        assert_eq!(back.name, d.name);
        for ext in ["pes", "pec", "dst", "exp", "jef", "vp3"] {
            assert_eq!(
                super::super::write_format_with(&back, ext, &opt2).unwrap(),
                super::super::write_format_with(&d, ext, &opt).unwrap(),
                "{ext}"
            );
        }
    }

    #[test]
    fn refuses_damaged_or_hostile_files() {
        let doc = Document::from_svg(SVG, "badge", None).unwrap();
        let good = write(&doc.plan(), &doc.settings.export_options());
        assert!(read(&good[..good.len() - 1]).is_err(), "cut short");
        assert!(
            read(&[good.as_slice(), &[0]].concat()).is_err(),
            "more after the end"
        );
        assert!(read(b"ZZPLAN\x09\x00").is_err(), "another version");
        assert!(read(b"PK\x03\x04").is_err(), "not a plan");
        // A count far beyond the file's size is refused before allocating.
        let mut huge = good.clone();
        let blocks_at = 8 + 4 + 5 + 1 + 8 + 4 + 8;
        huge[blocks_at..blocks_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&huge).is_err());
        // A NaN coordinate.
        let mut nan = good.clone();
        let n = nan.len();
        nan[n - 8..].copy_from_slice(&f64::NAN.to_le_bytes());
        assert!(read(&nan).is_err());
        // A stitch a light-year away, and a shortest stitch below none.
        let mut far = good.clone();
        far[n - 8..].copy_from_slice(&1e300f64.to_le_bytes());
        assert!(read(&far).is_err());
        let mut short = good.clone();
        let min_at = blocks_at - 8;
        short[min_at..min_at + 8].copy_from_slice(&(-1.0f64).to_le_bytes());
        assert!(read(&short).is_err());
    }
}
