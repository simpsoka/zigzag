//! Production worksheet: a one-page HTML summary to keep next to the machine
//! (picture, size, stitch count, time, and the thread for every color stop).

use super::jef_data::JEF_PALETTE;
use super::pec_data::PEC_PALETTE;
use super::preview::{PreviewOptions, render_png};
use crate::design::Design;
use anyhow::Result;
use std::fmt::Write;

/// Details the design itself doesn't carry.
#[derive(Clone, Debug, Default)]
pub struct WorksheetInfo {
    pub title: String,
    pub fabric: String,
    /// Hoop label and size (mm).
    pub hoop: Option<(String, f64, f64)>,
    /// Preview fabric color.
    pub background: [u8; 3],
    /// Object names, by shape index (for "what's sewn in this color").
    pub names: Vec<String>,
    /// Machine speed for the time estimate, stitches per minute.
    pub spm: f64,
}

/// Nearest thread in a palette: (name, catalog number if known).
fn nearest(
    c: [u8; 3],
    palette: impl Iterator<Item = ([u8; 3], String, Option<String>)>,
) -> (String, Option<String>) {
    palette
        .min_by(|(p, _, _), (q, _, _)| {
            super::perceptual_distance(c, *p).total_cmp(&super::perceptual_distance(c, *q))
        })
        .map(|(_, n, k)| (n, k))
        .unwrap_or_default()
}

pub fn brother_name(c: [u8; 3]) -> String {
    nearest(
        c,
        PEC_PALETTE
            .iter()
            .skip(1)
            .map(|&(r, g, b, n)| ([r, g, b], n.to_string(), None)),
    )
    .0
}

pub fn janome_thread(c: [u8; 3]) -> (String, String) {
    let (n, k) = nearest(
        c,
        JEF_PALETTE
            .iter()
            .skip(1)
            .map(|&(r, g, b, n, k)| ([r, g, b], n.to_string(), Some(k.to_string()))),
    );
    (n, k.unwrap_or_default())
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The worksheet as a self-contained HTML page (preview embedded).
pub fn html(d: &Design, info: &WorksheetInfo) -> Result<String> {
    use base64::Engine;
    let opt = PreviewOptions {
        hoop_mm: None,
        background: info.background,
        show_jumps: false,
        thread_mm: 0.38,
    };
    let png = render_png(d, &opt, 900)?;
    let img = base64::engine::general_purpose::STANDARD.encode(png);
    let b = d.bounds();
    let stitches = d.stitch_count();
    let spm = if info.spm > 0.0 { info.spm } else { 400.0 };
    let minutes = stitches as f64 / spm + d.blocks.len().saturating_sub(1) as f64 * 0.5;
    let mut rows = String::new();
    for (k, blk) in d.blocks.iter().enumerate() {
        let n: usize = blk.runs.iter().map(|r| r.pts.len()).sum();
        let hex = format!(
            "#{:02x}{:02x}{:02x}",
            blk.color[0], blk.color[1], blk.color[2]
        );
        let (jn, jk) = janome_thread(blk.color);
        let objects: Vec<String> = d
            .block_shapes
            .get(k)
            .map(|v| {
                v.iter()
                    .filter_map(|&i| info.names.get(i))
                    .map(|s| esc(s))
                    .collect()
            })
            .unwrap_or_default();
        let stop = k > 0 && d.blocks[k - 1].color == blk.color;
        let _ = write!(
            rows,
            "<tr><td class=n>{}</td><td><span class=sw style=\"background:{hex}\"></span>{hex}</td><td>{}</td><td>{} {}</td><td class=r>{}</td><td>{}{}</td></tr>",
            k + 1,
            esc(&brother_name(blk.color)),
            esc(&jk),
            esc(&jn),
            n,
            if stop {
                "<b>Stop</b> (same thread) · "
            } else {
                ""
            },
            objects.join(", ")
        );
    }
    let hoop = info
        .hoop
        .as_ref()
        .map(|(l, _, _)| esc(l))
        .unwrap_or_else(|| "—".into());
    let title = if info.title.is_empty() {
        "Design".to_string()
    } else {
        esc(&info.title)
    };
    Ok(format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} · worksheet</title>
<style>
:root {{ --ink:#1d1d1f; --muted:#6e6e73; --line:#e3e1dc; --bg:#ffffff; }}
body {{ font: 14px/1.45 -apple-system, "Segoe UI", Inter, Roboto, sans-serif; color: var(--ink); background: var(--bg); margin: 24px auto; max-width: 860px; padding: 0 16px; }}
h1 {{ font-size: 22px; margin: 0 0 4px; }}
.sub {{ color: var(--muted); margin-bottom: 18px; }}
.top {{ display: grid; grid-template-columns: minmax(0, 1fr) 260px; gap: 20px; align-items: start; }}
img {{ width: 100%; border: 1px solid var(--line); border-radius: 8px; }}
dl {{ margin: 0; display: grid; grid-template-columns: auto 1fr; gap: 6px 14px; }}
dt {{ color: var(--muted); }} dd {{ margin: 0; font-variant-numeric: tabular-nums; }}
table {{ width: 100%; border-collapse: collapse; margin-top: 22px; }}
th, td {{ text-align: left; padding: 7px 8px; border-bottom: 1px solid var(--line); vertical-align: top; }}
th {{ font-size: 12px; color: var(--muted); font-weight: 600; text-transform: uppercase; letter-spacing: .04em; }}
td.n, td.r {{ font-variant-numeric: tabular-nums; }} td.r {{ text-align: right; }}
.sw {{ display: inline-block; width: 14px; height: 14px; border-radius: 3px; border: 1px solid rgba(0,0,0,.25); vertical-align: -2px; margin-right: 6px; }}
.note {{ color: var(--muted); font-size: 12px; margin-top: 14px; }}
@media (max-width: 640px) {{ .top {{ grid-template-columns: 1fr; }} }}
@media print {{ body {{ margin: 0; }} }}
</style></head><body>
<h1>{title}</h1>
<div class="sub">Production worksheet · ZigZag</div>
<div class="top">
<img alt="Stitch preview" src="data:image/png;base64,{img}">
<dl>
<dt>Size</dt><dd>{w:.1} × {h:.1} mm</dd>
<dt>Stitches</dt><dd>{stitches}</dd>
<dt>Colors</dt><dd>{colors}</dd>
<dt>Trims</dt><dd>{trims}</dd>
<dt>Sewing time</dt><dd>about {minutes:.0} min at {spm:.0} spm</dd>
<dt>Fabric</dt><dd>{fabric}</dd>
<dt>Hoop</dt><dd>{hoop}</dd>
</dl>
</div>
<table>
<thead><tr><th>#</th><th>Color</th><th>Brother</th><th>Janome</th><th class=r>Stitches</th><th>Sews</th></tr></thead>
<tbody>{rows}</tbody>
</table>
<p class="note">Thread names are the nearest match in each brand's standard palette; check against your spools. Center the hoop on the design's center mark.</p>
</body></html>
"#,
        w = b.width() / 10.0,
        h = b.height() / 10.0,
        colors = d.blocks.len(),
        trims = d.trim_count(),
        fabric = esc(&info.fabric),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::{Block, Run};
    use crate::geom::P;

    #[test]
    fn worksheet_lists_every_color_stop_with_thread_names() {
        let run = Run {
            pts: vec![P::new(0.0, 0.0), P::new(100.0, 0.0), P::new(100.0, 50.0)],
            trim_before: false,
        };
        let d = Design {
            name: "t".into(),
            blocks: vec![
                Block {
                    color: [0, 0, 0],
                    runs: vec![run.clone()],
                },
                Block {
                    color: [237, 23, 31],
                    runs: vec![run],
                },
            ],
            block_shapes: vec![vec![0], vec![1]],
            ..Design::default()
        };
        let info = WorksheetInfo {
            title: "Cap <1>".into(),
            names: vec!["outline".into(), "heart".into()],
            fabric: "Thin cap".into(),
            ..Default::default()
        };
        let h = html(&d, &info).unwrap();
        assert!(h.contains("Cap &lt;1&gt;"));
        assert!(h.contains("data:image/png;base64,"));
        assert!(
            h.contains(">Black<") && h.contains(">Red<"),
            "Brother names"
        );
        assert!(h.contains("heart"));
        assert_eq!(brother_name([14, 31, 124]), "Prussian Blue");
        assert_eq!(janome_thread([0, 0, 0]).1, "002");
    }
}
