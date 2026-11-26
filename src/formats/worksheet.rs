//! Production worksheet: a self-contained HTML page to print and keep next
//! to the machine, and a proof to show the client.
//!
//! Like Ink/Stitch's Print (an HTML page the browser prints, or saves as a
//! PDF), in one file: the operator sheet (picture, size, stitches, time, and
//! a row for every color stop with a picture of what that step sews, its
//! thread, stitches, trims and time) and, when ticked on the page, a client
//! proof (picture, threads, size and an approval line). Speed and the time a
//! thread change or a trim takes are set on the page, and the page
//! remembers them; so does the paper size. Client, order, date and notes are
//! typed on the page before printing.

use super::charts::nearest_threads;
use super::preview::{PreviewOptions, render_png};
use crate::design::Design;
use crate::geom::{Bounds, P};
use anyhow::Result;
use std::fmt::Write;

/// Whose thread chart names the colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Chart {
    #[default]
    Brother,
    Janome,
}

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
    /// Machine speed for the time estimate, stitches per minute (the page
    /// can change it).
    pub spm: f64,
    /// The chart thread names come from.
    pub chart: Chart,
}

/// Seconds a thread change (or a stop) takes, and a trim, until the page
/// is told otherwise: a home machine waits while the thread is changed by
/// hand.
const CHANGE_S: f64 = 30.0;
const TRIM_S: f64 = 8.0;

/// The nearest thread in a chart: (brand, name, number).
pub fn thread(chart: Chart, c: [u8; 3]) -> (&'static str, &'static str, Option<&'static str>) {
    let ((bn, bk), (jn, jk)) = nearest_threads(c);
    match chart {
        Chart::Brother => ("Brother", bn, bk),
        Chart::Janome => ("Janome", jn, Some(jk)),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// A time as the page shows it (the page's script writes it the same way).
fn fmt_time(s: f64) -> String {
    let s = s.max(0.0).round() as u64;
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        // (Whole minutes first: 1:59:30 is 2 h 00 min, not 1 h 60 min.)
        let m = s.div_ceil(60);
        format!("{} h {:02} min", m / 60, m % 60)
    }
}

fn total_time(s: f64) -> String {
    let m = (s / 60.0).ceil().max(1.0) as u64;
    if m < 60 {
        format!("about {m} min")
    } else {
        format!("about {} h {:02} min", m / 60, m % 60)
    }
}

/// One color stop, as the worksheet counts it.
struct Step {
    stitches: usize,
    trims: usize,
    /// Before it: 0 none (the first), 1 a thread change, 2 a stop in the
    /// same thread.
    change: u8,
}

fn steps(d: &Design) -> Vec<Step> {
    d.blocks
        .iter()
        .enumerate()
        .map(|(k, blk)| Step {
            stitches: blk.runs.iter().map(|r| r.pts.len()).sum(),
            trims: blk.runs.iter().filter(|r| r.trim_before).count(),
            change: match k {
                0 => 0,
                _ if d.blocks[k - 1].color == blk.color => 2,
                _ => 1,
            },
        })
        .collect()
}

fn seconds(s: &Step, spm: f64) -> f64 {
    s.stitches as f64 * 60.0 / spm
        + s.trims as f64 * TRIM_S
        + if s.change > 0 { CHANGE_S } else { 0.0 }
}

/// A picture of step `k`: what's sewn before it faded into the fabric,
/// this step's thread on top, framed like the whole design.
fn step_png(d: &Design, k: usize, frame: Bounds, background: [u8; 3]) -> Result<Vec<u8>> {
    let fade = |c: [u8; 3]| {
        let mut o = [0u8; 3];
        for i in 0..3 {
            o[i] = (c[i] as f64 * 0.2 + background[i] as f64 * 0.8).round() as u8;
        }
        o
    };
    let mut blocks: Vec<_> = d.blocks[..=k].to_vec();
    let last = blocks.len() - 1;
    for b in &mut blocks[..last] {
        b.color = fade(b.color);
    }
    let part = Design {
        blocks,
        ..Design::default()
    };
    // A thread a pixel or more wide at the size it's shown.
    let extent_mm = (frame.max.x - frame.min.x).max(frame.max.y - frame.min.y) / 10.0;
    let opt = PreviewOptions {
        hoop_mm: None,
        background,
        show_jumps: false,
        thread_mm: (extent_mm / 60.0).max(0.38),
        marks: Vec::new(),
        region: Some(frame),
        ..Default::default()
    };
    render_png(&part, &opt, 240)
}

/// The worksheet as a self-contained HTML page (pictures embedded).
pub fn html(d: &Design, info: &WorksheetInfo) -> Result<String> {
    use base64::Engine;
    let b64 = |png: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(png);
    let opt = PreviewOptions {
        hoop_mm: None,
        background: info.background,
        show_jumps: false,
        thread_mm: 0.38,
        marks: Vec::new(),
        ..Default::default()
    };
    let img = b64(render_png(d, &opt, 900)?);
    let b = d.bounds();
    let frame = if b.is_empty() {
        b
    } else {
        Bounds {
            min: P::new(b.min.x - 30.0, b.min.y - 30.0),
            max: P::new(b.max.x + 30.0, b.max.y + 30.0),
        }
    };
    let spm = if info.spm > 0.0 { info.spm } else { 400.0 };
    let steps = steps(d);
    let total_s: f64 = steps.iter().map(|s| seconds(s, spm)).sum();
    let changes = steps.iter().filter(|s| s.change == 1).count();
    let stops = steps.iter().filter(|s| s.change == 2).count();

    let mut rows = String::new();
    for (k, (blk, st)) in d.blocks.iter().zip(&steps).enumerate() {
        let (brand, name, number) = thread(info.chart, blk.color);
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
        let before = match st.change {
            2 => "<b>Stop</b>, same thread · ",
            _ => "",
        };
        let pic = b64(step_png(d, k, frame, info.background)?);
        let _ = write!(
            rows,
            "<tr data-stitches={} data-trims={} data-change={}>\
<td class=n>{}</td>\
<td class=pic><img alt=\"Step {}\" src=\"data:image/png;base64,{pic}\"></td>\
<td><span class=sw style=\"background:{h}\"></span><b>{}</b><div class=dim>{brand} {}<br>{h}</div></td>\
<td class=r>{}</td><td class=r>{}</td><td class=\"r t\">{}</td>\
<td>{before}{}<div class=note contenteditable data-ph=\"Notes…\"></div></td></tr>",
            st.stitches,
            st.trims,
            st.change,
            k + 1,
            k + 1,
            esc(name),
            esc(number.unwrap_or("")),
            st.stitches,
            st.trims,
            fmt_time(seconds(st, spm)),
            objects.join(", "),
            h = hex(blk.color),
        );
    }

    // The client sees each thread once.
    let mut swatches = String::new();
    let mut seen: Vec<[u8; 3]> = Vec::new();
    for blk in &d.blocks {
        if seen.contains(&blk.color) {
            continue;
        }
        seen.push(blk.color);
        let (brand, name, number) = thread(info.chart, blk.color);
        let _ = write!(
            swatches,
            "<li><span class=sw style=\"background:{}\"></span>{} <span class=dim>{brand} {}</span></li>",
            hex(blk.color),
            esc(name),
            esc(number.unwrap_or("")),
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
    let size = format!("{:.1} × {:.1} mm", b.width() / 10.0, b.height() / 10.0);
    Ok(format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} · worksheet</title>
<style id="paper">@page {{ size: letter; margin: 12mm; }}</style>
<style>
:root {{ --ink:#1d1d1f; --muted:#6e6e73; --line:#e3e1dc; --bg:#ffffff; }}
* {{ box-sizing: border-box; }}
body {{ font: 13px/1.45 -apple-system, "Segoe UI", Inter, Roboto, sans-serif; color: var(--ink); background: var(--bg); margin: 0 auto; max-width: 900px; padding: 0 16px 40px; }}
h1 {{ font-size: 22px; margin: 0; }}
.dim {{ color: var(--muted); font-size: 12px; }}
.bar {{ position: sticky; top: 0; z-index: 1; background: #f5f5f7; border-bottom: 1px solid var(--line); margin: 0 -16px 20px; padding: 10px 16px; display: flex; flex-wrap: wrap; gap: 8px 18px; align-items: center; }}
.bar label {{ display: flex; gap: 6px; align-items: center; color: var(--muted); }}
.bar input[type=number] {{ width: 64px; font: inherit; padding: 2px 4px; }}
.bar select {{ font: inherit; }}
.bar button {{ margin-left: auto; font: inherit; font-weight: 600; background: #1d1d1f; color: #fff; border: 0; border-radius: 6px; padding: 6px 16px; cursor: pointer; }}
.page {{ padding-top: 4px; }}
.proof .operator {{ border-top: 1px dashed var(--line); margin-top: 32px; padding-top: 28px; }}
.head {{ display: flex; justify-content: space-between; gap: 20px; align-items: baseline; margin-bottom: 14px; }}
.fields {{ display: grid; grid-template-columns: auto minmax(150px, auto); gap: 2px 10px; }}
.fields dt {{ color: var(--muted); }}
.fields dd {{ margin: 0; min-width: 150px; border-bottom: 1px solid var(--line); }}
.top {{ display: grid; grid-template-columns: minmax(0, 1fr) 250px; gap: 20px; align-items: start; }}
.top > img {{ width: 100%; border: 1px solid var(--line); border-radius: 8px; }}
dl.facts {{ margin: 0; display: grid; grid-template-columns: auto 1fr; gap: 5px 14px; }}
dl.facts dt {{ color: var(--muted); }} dl.facts dd {{ margin: 0; font-variant-numeric: tabular-nums; }}
table {{ width: 100%; border-collapse: collapse; margin-top: 20px; }}
th, td {{ text-align: left; padding: 6px 8px; border-bottom: 1px solid var(--line); vertical-align: top; }}
th {{ font-size: 11px; color: var(--muted); font-weight: 600; text-transform: uppercase; letter-spacing: .04em; }}
tr {{ break-inside: avoid; }}
td.n, td.r {{ font-variant-numeric: tabular-nums; }} .r {{ text-align: right; }}
td.pic img {{ width: 72px; height: 72px; object-fit: contain; border: 1px solid var(--line); border-radius: 4px; display: block; }}
.sw {{ display: inline-block; width: 13px; height: 13px; border-radius: 3px; border: 1px solid rgba(0,0,0,.25); vertical-align: -2px; margin-right: 6px; }}
.note {{ margin-top: 4px; min-height: 1.4em; }}
.notes {{ margin-top: 18px; }} .notes > [contenteditable] {{ min-height: 4em; border: 1px solid var(--line); border-radius: 6px; padding: 6px 8px; }}
[contenteditable] {{ outline: none; }}
[contenteditable]:focus {{ background: #fffbe6; }}
.threads {{ list-style: none; padding: 0; margin: 16px 0 0; columns: 2; }}
.threads li {{ padding: 3px 0; break-inside: avoid; }}
.sign {{ display: grid; grid-template-columns: 2fr 1fr; gap: 24px; margin-top: 40px; }}
.sign div {{ border-top: 1px solid var(--ink); padding-top: 4px; color: var(--muted); }}
.foot {{ color: var(--muted); font-size: 11px; margin-top: 14px; }}
body:not(.proof) .client {{ display: none; }}
@media screen {{ [data-ph]:empty::before {{ content: attr(data-ph); color: #b0b0b5; }} }}
@media (max-width: 640px) {{ .top {{ grid-template-columns: 1fr; }} }}
@media print {{
  body {{ max-width: none; padding: 0; font-size: 11px; }}
  .bar {{ display: none; }}
  .proof .operator {{ border: 0; margin: 0; padding: 0; break-before: page; }}
  .top > img {{ max-height: 90mm; object-fit: contain; }}
  td.pic img {{ width: 16mm; height: 16mm; }}
  th, td {{ padding: 4px 6px; }}
  .notes {{ break-inside: avoid; margin-top: 10px; }}
  .notes > [contenteditable] {{ min-height: 2.5em; }}
  [contenteditable]:focus {{ background: none; }}
}}
</style></head><body>
<div class="bar">
<label>Paper <select id="paper-size"><option value="letter">Letter</option><option value="A4">A4</option></select></label>
<label><input type="number" id="spm" min="50" step="50" value="{spm:.0}"> stitches/min</label>
<label><input type="number" id="change" min="0" value="{change_s:.0}"> s per thread change</label>
<label><input type="number" id="trim" min="0" value="{trim_s:.0}"> s per trim</label>
<label><input type="checkbox" id="proof"> Client proof page</label>
<button onclick="window.print()">Print…</button>
</div>

<section class="page client">
<div class="head"><h1>{title}</h1>
<dl class="fields"><dt>Client</dt><dd contenteditable data-ph="Name"></dd><dt>Date</dt><dd class="date" contenteditable></dd></dl></div>
<div class="top">
<img id="proof-img" alt="Design">
<dl class="facts">
<dt>Size</dt><dd>{size}</dd>
<dt>Stitches</dt><dd>{stitches}</dd>
<dt>Threads</dt><dd>{threads}</dd>
<dt>Fabric</dt><dd>{fabric}</dd>
</dl>
</div>
<ul class="threads">{swatches}</ul>
<div class="notes"><div class="dim">Notes</div><div contenteditable data-ph="Placement, quantity, price…"></div></div>
<div class="sign"><div>Approved by</div><div>Date</div></div>
<p class="foot">Thread colors are shown as closely as a screen or printer allows; the stitched piece uses the threads named.</p>
</section>

<section class="page operator">
<div class="head"><div><h1>{title}</h1><div class="dim">Production worksheet · ZigZag</div></div>
<dl class="fields"><dt>Client</dt><dd contenteditable></dd><dt>Order</dt><dd contenteditable></dd><dt>Date</dt><dd class="date" contenteditable></dd></dl></div>
<div class="top">
<img id="design-img" alt="Stitch preview" src="data:image/png;base64,{img}">
<dl class="facts">
<dt>Size</dt><dd>{size}</dd>
<dt>Stitches</dt><dd>{stitches}</dd>
<dt>Thread changes</dt><dd>{changes}</dd>
<dt>Stops</dt><dd>{stops}</dd>
<dt>Trims</dt><dd>{trims}</dd>
<dt>Sewing time</dt><dd><span id="total">{total}</span><div class="dim" id="time-basis">at {spm:.0} stitches/min</div></dd>
<dt>Fabric</dt><dd>{fabric}</dd>
<dt>Hoop</dt><dd>{hoop}</dd>
</dl>
</div>
<table>
<thead><tr><th>#</th><th>Sews</th><th>Thread</th><th class=r>Stitches</th><th class=r>Trims</th><th class=r>Time</th><th>Objects · notes</th></tr></thead>
<tbody>{rows}</tbody>
</table>
<div class="notes"><div class="dim">Notes</div><div contenteditable data-ph="Stabilizer, needle, bobbin…"></div></div>
<p class="foot">Thread names are the nearest match in the {brand} chart; check against your spools. Center the hoop on the design's center mark. Times allow <span class="change-s">{change_s:.0}</span> s for each thread change or stop and <span class="trim-s">{trim_s:.0}</span> s for each trim.</p>
</section>

<script>
(function () {{
  var KEY = 'zigzag-worksheet';
  var saved = {{}};
  try {{ saved = JSON.parse(localStorage.getItem(KEY) || '{{}}'); }} catch (e) {{}}
  var $ = function (id) {{ return document.getElementById(id); }};
  if (!saved.paper) saved.paper = /^en-(US|CA)$|^(es-MX|fr-CA)$/.test(navigator.language) ? 'letter' : 'A4';
  ['spm', 'change', 'trim'].forEach(function (k) {{ if (saved[k] > 0 || (saved[k] === 0 && k !== 'spm')) $(k).value = saved[k]; }});
  $('paper-size').value = saved.paper;
  $('proof').checked = !!saved.proof;
  $('proof-img').src = $('design-img').src;
  var today = new Date().toLocaleDateString();
  Array.prototype.forEach.call(document.querySelectorAll('.date'), function (e) {{ if (!e.textContent) e.textContent = today; }});
  function fmt(s) {{
    s = Math.max(0, Math.round(s));
    if (s < 60) return s + ' s';
    if (s < 3600) return Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0');
    var m = Math.ceil(s / 60);
    return Math.floor(m / 60) + ' h ' + String(m % 60).padStart(2, '0') + ' min';
  }}
  function total(s) {{
    var m = Math.max(1, Math.ceil(s / 60));
    return m < 60 ? 'about ' + m + ' min' : 'about ' + Math.floor(m / 60) + ' h ' + String(m % 60).padStart(2, '0') + ' min';
  }}
  function update() {{
    var spm = +$('spm').value || {spm:.0}, change = +$('change').value || 0, trim = +$('trim').value || 0;
    var sum = 0;
    Array.prototype.forEach.call(document.querySelectorAll('tr[data-stitches]'), function (tr) {{
      var s = tr.dataset.stitches * 60 / spm + tr.dataset.trims * trim + (tr.dataset.change > 0 ? change : 0);
      sum += s;
      tr.querySelector('.t').textContent = fmt(s);
    }});
    $('total').textContent = total(sum);
    $('time-basis').textContent = 'at ' + spm + ' stitches/min';
    document.querySelector('.change-s').textContent = change;
    document.querySelector('.trim-s').textContent = trim;
    $('paper').textContent = '@page {{ size: ' + $('paper-size').value + '; margin: 12mm; }}';
    document.body.classList.toggle('proof', $('proof').checked);
    try {{
      localStorage.setItem(KEY, JSON.stringify({{ spm: spm, change: change, trim: trim, paper: $('paper-size').value, proof: $('proof').checked }}));
    }} catch (e) {{}}
  }}
  ['spm', 'change', 'trim', 'paper-size', 'proof'].forEach(function (id) {{ $(id).addEventListener('input', update); $(id).addEventListener('change', update); }});
  update();
}})();
</script>
</body></html>
"#,
        stitches = d.stitch_count(),
        threads = seen.len(),
        trims = d.trim_count(),
        total = total_time(total_s),
        fabric = esc(&info.fabric),
        change_s = CHANGE_S,
        trim_s = TRIM_S,
        brand = match info.chart {
            Chart::Brother => "Brother",
            Chart::Janome => "Janome",
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::{Block, Run};
    use crate::geom::P;

    fn run(trim: bool) -> Run {
        Run {
            pts: vec![P::new(0.0, 0.0), P::new(100.0, 0.0), P::new(100.0, 50.0)],
            trim_before: trim,
        }
    }

    fn design() -> Design {
        Design {
            name: "t".into(),
            blocks: vec![
                Block {
                    color: [0, 0, 0],
                    runs: vec![run(false)],
                },
                Block {
                    color: [237, 23, 31],
                    runs: vec![run(false), run(true)],
                },
                Block {
                    color: [237, 23, 31],
                    runs: vec![run(false)],
                },
            ],
            block_shapes: vec![vec![0], vec![1], vec![1]],
            ..Design::default()
        }
    }

    #[test]
    fn worksheet_lists_every_color_stop_with_its_thread_picture_and_time() {
        let info = WorksheetInfo {
            title: "Cap <1>".into(),
            names: vec!["outline".into(), "heart".into()],
            fabric: "Thin cap".into(),
            spm: 600.0,
            ..Default::default()
        };
        let h = html(&design(), &info).unwrap();
        assert!(h.contains("Cap &lt;1&gt;"));
        assert!(!h.contains("Cap <1>"));
        // The design, and a picture for each of the three steps.
        assert_eq!(h.matches("data:image/png;base64,").count(), 4);
        assert!(h.contains("<b>Black</b>") && h.contains("<b>Red</b>"), "Brother names");
        assert!(h.contains("Brother 900"), "Brother numbers");
        assert!(h.contains("heart"));
        assert!(h.contains("<b>Stop</b>, same thread"), "the third step stops");
        assert!(h.contains("data-stitches=6 data-trims=1 data-change=1"));
        // 9 stitches at 600/min, a thread change, a stop, one trim.
        let s = 9.0 * 60.0 / 600.0 + 2.0 * CHANGE_S + TRIM_S;
        assert!(h.contains(&format!("<span id=\"total\">{}</span>", total_time(s))));
        assert!(h.contains("value=\"600\""));
        // One swatch a thread on the client's page.
        assert_eq!(h.matches("<li><span class=sw").count(), 2);
    }

    #[test]
    fn janome_names_its_threads_by_code() {
        let info = WorksheetInfo {
            chart: Chart::Janome,
            ..Default::default()
        };
        let h = html(&design(), &info).unwrap();
        assert!(h.contains("Janome 002"), "black is Janome 002");
        assert!(!h.contains("Brother 900"));
    }

    #[test]
    fn times_read_like_a_clock() {
        assert_eq!(fmt_time(42.4), "42 s");
        assert_eq!(fmt_time(83.0), "1:23");
        assert_eq!(fmt_time(3700.0), "1 h 02 min");
        // (The minutes carry into the hour.)
        assert_eq!(fmt_time(7199.0), "2 h 00 min");
        assert_eq!(fmt_time(3600.0), "1 h 00 min");
        assert_eq!(total_time(30.0), "about 1 min");
        assert_eq!(total_time(3900.0), "about 1 h 05 min");
    }
}
