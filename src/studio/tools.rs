//! MCP tools: what agents can do with the live studio document.

use crate::app::{HOOPS, SPM, Studio, shape_bounds};
use crate::mcp::{Content, ToolResult};
use crate::widgets::{allowed_kinds, kind_name};
use serde_json::{Value, json};
use std::path::PathBuf;
use zigzag::checks::{Issue, Level};
use zigzag::document::{Document, Placement};
use zigzag::formats::preview::{Art, PreviewOptions, render_png};
use zigzag::geom::{Bounds, P};
use zigzag::params::{FABRICS, Kind, Params, parse_hex, underlay};
use zigzag::svg::{Geometry, Part};

pub const INSTRUCTIONS: &str = "ZigZag is a machine-embroidery editor showing live documents on a hoop, one per tab; the tools work on the one in front (list_documents, switch_document, new_document). \
Coordinates are millimetres from the hoop center (+x right, +y down). The person watching sees every change, and each tool call is one undo step. \
Typical flow: get_design → add_svg (any SVG artwork) and add_text (lettering) → transform_objects to size and place → set_object to choose stitch types → render_preview to look at the stitches (artwork: true shows the drawing itself; region or object zooms in on details) → review_design (cost to sew and clarity, with what to fix) → check_design. Changes that are the person's to decide (how it looks against what it costs to sew) go to them as offer_choices, a question they answer with a click → save, or export (machine file or html worksheet). \
Where a tool takes `objects` as one name or a list, \"all\" is every object; set_object takes `objects` too, to change several the same way at once (delete_object takes a list). combine_shapes merges or cuts filled shapes (unite, minus_front, intersect, exclude), so overlapping shapes in one thread aren't sewn twice. get_design's `selected` is what the person has selected on the canvas: what they mean by \"this\". \
Stitch types: fill (tatami rows; areas wider than ~7 mm), satin (glossy zigzag columns 1–7 mm wide; works on strokes, and on narrow filled shapes like letters, bars and rings), run (thin running stitch for details), \
contour (rows following the outline inward), blanket (E-stitch edge), applique (placement line, stop, tack-down, stop, satin cover) and manual (every path point is a needle point). \
Settings can also be preset in SVG element ids, e.g. id=\"logo_fill_a=45\" or id=\"border_satin_w=2\". \
Groups are how the person selects and moves a part of the design as one thing (a click takes the whole group), and how the Layers list reads: put together only the objects that make one thing (a flower's petals, a bridge's towers and cables, the two waves of a wave mark), each in a <g> with an id that names it (id=\"bridge\"); leave everything else out of groups, and never wrap a whole drawing, or a background with what's drawn on it, in one. get_design gives each object's `group` (its name, or its id where it has none); group_objects and ungroup_objects change them afterwards. \
An object is named for its id without the settings (id=\"Face_mask_fill\" is \"Face mask\"); name it to a tool by that name, by the id, or by its index. \
list_machines and set_document's machine set a document up for a machine: its hoop, the file it reads, its shortest stitch and its lock stitches. \
Hat designs are small (about 50 × 50 mm on the front of a cap): keep satin columns under 7 mm wide, letters at least 5 mm tall, and details at least 1.5 mm. \
open_file also opens machine files (pes, pec, dst, exp, jef, vp3) as manual stitches. \
trace_image turns a logo (PNG, JPEG, WebP, GIF, BMP or TIFF) into shapes (snap colors to Brother or Janome threads with `threads`). simplify_design makes the whole design quicker to sew (fewer threads, no details too small to sew, fewer trims; try it with dry_run first to see what it comes to). add_outline gives a filled shape an outline (a running stitch, or satin) that is an object of its own, named \"<shape> (outline)\", with its own thread and stitch type. round_corners softens sharp corners, which stitch more cleanly; simplify takes extra points out of outlines (traced ones especially) and clean_up removes stray points, objects too small to sew and extra points across the design. \
To make satin stitching cleaner: measure_stitching counts what shows (stitches outside the shape, bare cloth, crowded needle points, seams, piled satin, travel in sight, trims); change one thing (a letter's corners with set_object cut or mitred, its zigzag with even, its lean, underlay or pull; or break_into_columns and then move_object to reorder the columns and edit_path to move a rail's points or add, move or delete a rung, reading them first with get_path); measure again passing the first result's measures as `before`, keep the change if the verdict is better and undo it if it's worse; and look at render_preview, which sees what the numbers don't. Text is made shapes with create_outlines before its columns or points can be changed. Before breaking a shape into columns, duplicate it and set the copy's kind to skip, to pass as measure_stitching's `within` (the outline the columns are held to); delete the copy when done. \
Use batch to make several changes in one call: it is all or nothing and one undo step, each step is {\"tool\": name, \"arguments\": {…}}, and later steps can use earlier results (\"$0.added.0.name\"). \
The person can pin numbered notes on the canvas to show where they mean: get_annotations reads them (render_preview draws them too), and resolve_note marks one done with a word on what you did. \
To save trims where it won't be seen, set_document's blend_mm sews across gaps where the thread is close in color to what it crosses (its own thread, a lighter or darker one, or cloth of its color) and cuts where it would stand out; it needs preview_color to be the cloth's real color. review_design tries it and says what it saves. \
To save a particular trim by hand, get_cuts lists each cut with its two ends, what a straight line between them would lie on, and the way round that shows least (least_seen_way); set_object's route_mm on the object the thread begins again on sews it that way instead (pass the way's way_mm, or your own points): keep it where it hides (on or beside something in a like color), leave a cut that has no such way, and look at what you routed close up with render_preview after. \
To offer the person a choice (when they ask for options, alternatives or a few versions), make versions to compare side by side with the original: start_variants, then for each version make its changes and call save_variant with a short label (it keeps the version and puts the original back for the next); show_comparison shows them again or puts them away. \
Tools that change the design return the whole object or document; pass terse: true (on a tool, or on a batch for all its steps) for a short answer instead: ok, the names of objects added, and new_problems, the design problems that weren't there before.";

/// The thread a setting may add on a color not its own (mm, a line that
/// shows) and not count as showing: about none, less the drawing's own
/// rounding. (Thread on its own color is texture, said but allowed.)
const SHOWS_MM: f64 = 1.0;
/// How wide the stitches are drawn to see where a setting's thread lies.
const LOOK_PX: u32 = 1200;
/// Thread as drawn, mm.
const THREAD_MM: f64 = 0.38;

/// How many pixels change color clearly between two drawings of the same
/// frame (premultiplied RGBA): thread that lies on another color.
fn changed_px(now: &(u32, u32, Vec<u8>), then: &(u32, u32, Vec<u8>)) -> usize {
    if (now.0, now.1) != (then.0, then.1) {
        return usize::MAX / 2;
    }
    now.2
        .chunks_exact(4)
        .zip(then.2.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|k| (a[k] as i32 - b[k] as i32).abs() > 40))
        .count()
}

/// The way out every card of choices ends with.
const KEEP: &str = "Keep it as it is";
/// The choice of all of a card's changes at once.
const ALL: &str = "Do all of these";

/// Whether a card's changes can all be made together: none sets a
/// document setting, or an object's setting, that another sets otherwise
/// (and none changes an object another reshapes or removes).
fn combinable(choices: &[crate::agent::Choice]) -> bool {
    let mut set: Vec<(String, Value, usize)> = Vec::new();
    // The objects each choice touches at all, and the ones it reshapes,
    // removes or otherwise changes as a whole.
    let mut touched: Vec<(String, usize)> = Vec::new();
    let mut whole: Vec<(String, usize)> = Vec::new();
    for (k, c) in choices.iter().enumerate() {
        for st in c.steps.as_array().into_iter().flatten() {
            let tool = st["tool"].as_str().unwrap_or("");
            let args = st["arguments"].as_object().cloned().unwrap_or_default();
            // (A step that uses an earlier step's result counts its steps
            // from its own choice's first: put after another's, it would
            // take that one's.)
            if args
                .values()
                .any(|v| v.to_string().contains("\"$") && !v.to_string().contains("\"$$"))
            {
                return false;
            }
            let mut objects: Vec<String> = args
                .get("object")
                .map(|o| o.to_string())
                .into_iter()
                .collect();
            match args.get("objects") {
                Some(Value::Array(a)) => objects.extend(a.iter().map(|o| o.to_string())),
                Some(o) => objects.push(o.to_string()),
                None => {}
            }
            let all = tool != "set_object" && tool != "set_document";
            for o in &objects {
                let other = |list: &[(String, usize)]| list.iter().any(|(x, n)| x == o && *n != k);
                if other(&whole) || (all && other(&touched)) {
                    return false;
                }
                touched.push((o.clone(), k));
                if all {
                    whole.push((o.clone(), k));
                }
            }
            let object = args
                .get("object")
                .map(|o| o.to_string())
                .unwrap_or_default();
            let targets: Vec<(String, Value)> = match tool {
                "set_document" | "set_object" => args
                    .iter()
                    .filter(|(key, _)| !matches!(key.as_str(), "object" | "terse"))
                    .map(|(key, v)| (format!("{tool}/{object}/{key}"), v.clone()))
                    .collect(),
                // Anything else on an object: that object, all of it.
                _ => vec![(format!("{tool}/{object}"), Value::Null)],
            };
            for (t, v) in targets {
                match set.iter().find(|x| x.0 == t) {
                    Some(x) if x.2 != k && (x.1 != v || v.is_null()) => return false,
                    Some(_) => {}
                    None => set.push((t, v, k)),
                }
            }
        }
    }
    true
}

/// The direction a filled shape's outline runs along most (its principal
/// axis, from the outline's segments weighted by length), as a fill angle
/// (degrees, y down), and how much longer it is that way than across.
fn principal_axis(rings: &[zigzag::geom::Polyline]) -> Option<(f64, f64)> {
    let segs = || {
        rings.iter().flat_map(|r| {
            (0..r.len()).map(move |k| {
                let (a, b) = (r[k], r[(k + 1) % r.len()]);
                ((a.x + b.x) / 2.0, (a.y + b.y) / 2.0, a.dist(b))
            })
        })
    };
    let total: f64 = segs().map(|s| s.2).sum();
    if total <= 0.0 {
        return None;
    }
    let (mx, my) = segs().fold((0.0, 0.0), |(x, y), s| (x + s.0 * s.2, y + s.1 * s.2));
    let (mx, my) = (mx / total, my / total);
    let (mut xx, mut yy, mut xy) = (0.0, 0.0, 0.0);
    for (x, y, l) in segs() {
        xx += l * (x - mx) * (x - mx);
        yy += l * (y - my) * (y - my);
        xy += l * (x - mx) * (y - my);
    }
    let half = (xx + yy) / 2.0;
    let r = (((xx - yy) / 2.0).powi(2) + xy * xy).sqrt();
    let (big, small) = (half + r, (half - r).max(1e-9));
    let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
    Some((
        crate::widgets::normalize_angle(angle.to_degrees()),
        (big / small).sqrt(),
    ))
}

/// A batch step failed for want of an object (`why`, its error) that was
/// there when the batch began: which earlier step took it away, to say so.
/// `names_before[j]`: the objects before step j (the failed one last).
fn gone_in_batch(why: &str, names_before: &[Vec<String>], steps: &[Value]) -> Option<String> {
    let name = why.strip_prefix("no object named '")?.split("'.").next()?;
    let first = names_before.first()?;
    if !first.iter().any(|n| n == name) {
        return None;
    }
    let j = (0..names_before.len() - 1).find(|&j| {
        names_before[j].iter().any(|n| n == name) && !names_before[j + 1].iter().any(|n| n == name)
    })?;
    let tool = steps
        .get(j)?
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let how = match tool {
        "delete_object" => " (deleting a shape takes its outline with it)",
        "trace_image" | "simplify_design" | "clean_up" | "break_into_columns"
        | "create_outlines" => " (it replaces objects with new ones)",
        _ => "",
    };
    Some(format!(
        "'{name}' was there when the batch began: step {j} ({tool}) took it away{how}."
    ))
}

/// The error for an object name that isn't there. It names a dozen of the
/// objects, not all of them: a traced design has hundreds, and the whole list
/// filled the chat. `get_design` has the rest.
fn no_object_named(asked: &str, names: &[String]) -> String {
    const SHOWN: usize = 12;
    let some = names
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match names.len() {
        0 => format!("no object named '{asked}': the design has no objects"),
        n if n <= SHOWN => format!("no object named '{asked}'. Objects: {some}"),
        n => format!(
            "no object named '{asked}'. Objects: {some}, and {} more (get_design lists them all)",
            n - SHOWN
        ),
    }
}

fn obj_schema() -> Value {
    json!({ "description": "Object: its index in sewing order (0-based), its name from get_design, or the id you gave it in an SVG (Face_mask for \"Face mask\").", "oneOf": [{ "type": "integer" }, { "type": "string" }] })
}

fn text_props(new: bool) -> Value {
    let mut p = json!({
        "text": { "type": "string" },
        "font": { "type": "string", "description": "Font family (see list_fonts). Default: a clean sans." },
        "size_mm": { "type": "number", "description": "Capital letter height, 2 to 80. Default 8." },
        "spacing_mm": { "type": "number", "description": "Extra letter spacing. Default 0.4." },
        "arc_mm": { "type": "number", "description": "Arc radius: 0 straight, >0 over the top, <0 along the bottom." },
        "bold": { "type": "boolean", "description": "Default true: bold letters make better satin columns." },
        "italic": { "type": "boolean" },
        "color": { "type": "string", "description": "#rrggbb" },
        "kind": { "type": "string", "enum": ["satin", "fill", "run"], "description": "Default satin." },
        "leading": { "type": "number", "description": "Line spacing for multi-line text (newlines in `text`), as a multiple of the letter height. Default 1.5." },
        "align": { "type": "string", "enum": ["left", "center", "right"], "description": "Line alignment. Default center." },
        "h_scale": { "type": "number", "description": "Horizontal scale, percent (20 to 400). Default 100." },
        "v_scale": { "type": "number", "description": "Vertical scale, percent. Default 100." },
        "case": { "type": "string", "enum": ["as", "upper", "lower", "title"], "description": "Letter case applied to the text. Default as typed." },
        "on_path": { "description": "Put the text on an existing path object's outline (the object is then not sewn, and stays that way until its kind is set again); null takes it off.", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "null" }] },
        "path_offset": { "type": "number", "description": "On a path: where the text starts, 0–1 along it." },
        "path_flip": { "type": "boolean", "description": "On a path: letters on the other side." },
        "kern": { "type": "array", "description": "Pair kerning: for each, the 0-based character index and how far (mm) to move it and the rest of the line; replaces any kerning set before.", "items": { "type": "object", "properties": { "at": { "type": "integer" }, "mm": { "type": "number" } }, "required": ["at", "mm"] } }
    });
    if new {
        p["x_mm"] = json!({ "type": "number", "description": "Center x (mm from hoop center). Default 0." });
        p["y_mm"] = json!({ "type": "number", "description": "Center y. Default 0." });
    } else {
        p["object"] = obj_schema();
    }
    p
}

/// Apply text tool arguments onto a spec.
fn apply_text_args(spec: &mut zigzag::document::TextSpec, args: &Value) -> Result<(), String> {
    if let Some(v) = args.get("text") {
        spec.text = v.as_str().ok_or("text must be a string")?.to_string();
    }
    if let Some(v) = args.get("font") {
        let want = v.as_str().ok_or("font must be a string")?;
        let fonts = zigzag::document::font_families();
        spec.font = fonts
            .iter()
            .find(|f| f.eq_ignore_ascii_case(want))
            .cloned()
            .ok_or_else(|| format!("font '{want}' isn't installed; call list_fonts"))?;
    }
    // (A number that isn't one, or a yes that isn't true or false, is
    // said so: left out, the text would come back unchanged and look done.)
    let num = |k: &str| -> Result<Option<f64>, String> {
        match args.get(k) {
            None => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|v| v.is_finite())
                .map(Some)
                .ok_or(format!("`{k}` must be a number")),
        }
    };
    let flag = |k: &str| -> Result<Option<bool>, String> {
        match args.get(k) {
            None => Ok(None),
            Some(v) => v
                .as_bool()
                .map(Some)
                .ok_or(format!("`{k}` must be true or false")),
        }
    };
    if let Some(v) = num("size_mm")? {
        spec.size_mm = v.clamp(2.0, 80.0);
    }
    if let Some(v) = num("spacing_mm")? {
        spec.spacing_mm = v.clamp(-2.0, 10.0);
    }
    if let Some(v) = num("arc_mm")? {
        spec.arc_mm = if v.abs() < 0.5 {
            0.0
        } else {
            v.signum() * v.abs().clamp(5.0, 200.0)
        };
    }
    if let Some(v) = flag("bold")? {
        spec.bold = v;
    }
    if let Some(v) = flag("italic")? {
        spec.italic = v;
    }
    if let Some(v) = args.get("color") {
        spec.color = v
            .as_str()
            .and_then(parse_hex)
            .ok_or("color must look like #rrggbb")?;
    }
    if let Some(v) = args.get("kind") {
        spec.kind = match v.as_str() {
            Some("satin") => Kind::Satin,
            Some("fill") => Kind::Fill,
            Some("run") => Kind::Run,
            _ => return Err("kind must be satin, fill or run".into()),
        };
    }
    if let Some(v) = num("leading")? {
        spec.leading = v.clamp(0.5, 5.0);
    }
    if let Some(v) = args.get("align") {
        spec.align = v
            .as_str()
            .and_then(zigzag::document::Align::parse)
            .ok_or("align must be left, center or right")?;
    }
    if let Some(v) = num("h_scale")? {
        spec.h_scale = v.clamp(20.0, 400.0);
    }
    if let Some(v) = num("v_scale")? {
        spec.v_scale = v.clamp(20.0, 400.0);
    }
    if let Some(v) = args.get("case") {
        spec.case = v
            .as_str()
            .and_then(zigzag::document::Case::parse)
            .ok_or("case must be as, upper, lower or title")?;
    }
    if let Some(v) = num("path_offset")? {
        spec.path_offset = v.clamp(0.0, 1.0);
    }
    if let Some(v) = flag("path_flip")? {
        spec.path_flip = v;
    }
    if let Some(list) = args.get("kern") {
        let list = list.as_array().ok_or("kern must be a list of {at, mm}")?;
        spec.kern.clear();
        for item in list {
            let at = item
                .get("at")
                .and_then(Value::as_u64)
                .ok_or("kern.at must be an index")? as usize;
            let mm = item
                .get("mm")
                .and_then(Value::as_f64)
                .ok_or("kern.mm must be a number")?;
            spec.set_kern(at, mm.clamp(-5.0, 5.0));
        }
    }
    Ok(())
}

/// The `tools/list` result.
pub fn tool_specs() -> Value {
    let fabrics: Vec<&str> = FABRICS.iter().map(|f| f.id).collect();
    let hoops: Vec<&str> = HOOPS.iter().map(|h| h.0).collect();
    let mut tools = vec![
        json!({
            "name": "get_design",
            "description": "Describe the current document: every object (index, name, the `group` it's in if any (its name, or its id where it has none), stitch type, color, position and size in mm, effective settings, stitch count, problems; `prompt`, where it has one, is the place in `prompts` of the request it was made for), what's selected, design stats, fabric, hoop and options.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "add_svg",
            "description": "Add SVG artwork to the canvas (or replace the document). Every filled shape and stroke becomes an object; fills default to fill stitch, strokes to satin (≥1 mm) or run. Settings in element ids apply (e.g. id=\"leaf_satin\"). Group with <g id=\"name\"> only shapes that are parts of one thing, and name each group for it; don't put unrelated shapes (a background and what's on it, the whole drawing) in one <g>. A satin can be given as its two edges: one stroked path (any stroke width, id with satin) whose two longest subpaths are the column's rails, with any shorter subpaths crossing both as rungs that keep the rails in step and set which way the stitches run, e.g. d=\"M4 10 C12 2 28 2 36 10 M4 10 C12 18 28 18 36 10 M14 5 L18 15\". By default the artwork keeps its own size (viewBox units scaled by width/height attributes, else 1 unit = 1 px) and is centered on the hoop.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "svg": { "type": "string", "description": "A complete <svg> document." },
                    "x_mm": { "type": "number", "description": "Center of the artwork, mm right of the hoop center. Default 0." },
                    "y_mm": { "type": "number", "description": "Center of the artwork, mm below the hoop center. Default 0." },
                    "width_mm": { "type": "number", "description": "Scale the artwork to this width." },
                    "height_mm": { "type": "number", "description": "Scale the artwork to this height (with width_mm: fit inside both)." },
                    "replace": { "type": "boolean", "description": "Start a new document with only this artwork, keeping the document settings (fabric, hoop, machine, preview color). Default false." }
                },
                "required": ["svg"]
            }
        }),
        json!({
            "name": "set_object",
            "description": "Change how an object is stitched. Omitted fields stay as they are; pass null to reset a field to the fabric's default.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "object": obj_schema(),
                    "objects": { "description": "In place of `object`: several objects to change the same way, in one undo step (a list of indexes or names, or \"all\").", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } }] },
                    "cell_mm": { "type": ["number", "null"], "description": "Cross stitch: the size of each X's cell (1 to 10)." },
                    "kind": { "type": "string", "enum": ["fill", "satin", "run", "contour", "blanket", "applique", "meander", "ripple", "cross", "tartan", "manual", "skip"] },
                    "angle": { "type": ["number", "null"], "description": "Fill row direction in degrees (0 = horizontal, 90 = vertical)." },
                    "spacing_mm": { "type": ["number", "null"], "description": "Fill/contour row spacing, satin zigzag spacing, or blanket prong spacing." },
                    "length_mm": { "type": ["number", "null"], "description": "Max stitch length for fill and run." },
                    "pull_mm": { "type": ["number", "null"], "description": "Pull compensation per side." },
                    "width_mm": { "type": ["number", "null"], "description": "Satin width (strokes and borders), appliqué cover width, or blanket prong length." },
                    "split_mm": { "type": ["number", "null"], "description": "Satin auto-split: split stitches longer than this (0 = off; default 7)." },
                    "lean": { "type": ["number", "null"], "description": "Satin: how far the stitches lean from square across the column, degrees (−60 to 60; default 0). A few degrees keeps stitches from sinking between a knit's rows; more shows a slant." },
                    "even": { "type": ["boolean", "null"], "description": "Satin zigzag: true (the default) shares each step along the column between the stitch over and the stitch back, so both slant the same; false sews one straight across and the next the whole step." },
                    "rails": { "type": ["array", "null"], "items": { "type": "integer" }, "description": "Two-rail satin (a stroke path of two rails and any rungs): which two subpaths are the rails, counted from 1 in the path's order; the rest are rungs. Null: worked out from what crosses what (a rung crosses both rails), which can't tell a short column with exactly two rungs from its rungs." },
                    "border": { "type": ["boolean", "null"], "description": "Satin on a filled shape: true = along its outline, false = across it as a column." },
                    "cut": { "type": ["boolean", "null"], "description": "Satin on a filled shape (a letter): true (the default) cuts it into columns where its strokes part, corners squared, one stroke running on and the other ending against it. False: read as strokes, each its own column, corners mitred, unless a corner is too sharp for a mitre to sew cleanly or a join can't be read clearly, when it's cut (or always mitred, with mitred true)." },
                    "mitred": { "type": ["boolean", "null"], "description": "Satin on a filled shape (a letter) with cut false: true mitres every corner however sharp. False: as cut says." },
                    "spiral": { "type": ["boolean", "null"], "description": "Contour fill as one spiral out from the center (or the start point) instead of rings." },
                    "underlay": { "type": ["boolean", "null"] },
                    "satin_underlay": { "description": "Satin underlay passes, or \"auto\".", "oneOf": [{ "type": "array", "items": { "type": "string", "enum": ["center", "contour", "zigzag"] } }, { "type": "string", "enum": ["auto"] }] },
                    "short_stitches": { "type": ["boolean", "null"] },
                    "repeats": { "type": ["integer", "null"], "description": "Running stitch passes over the line, 1 to 9: 1 is a single run, 3 a triple run (bolder). An even number ends back where it started." },
                    "color": { "type": ["string", "null"], "description": "Thread color #rrggbb, or null for the SVG's color." },
                    "start": { "type": ["number", "null"], "description": "Where sewing starts: 0–1 along the object's outline (fills: the outer ring from its first point; lines: from their start). Null: the planner chooses." },
                    "end": { "type": ["number", "null"], "description": "Where sewing ends, 0–1 along the outline (the thread goes on from there to the next object)." },
                    "trim_before": { "type": ["boolean", "null"], "description": "Cut before this object if a thread is running, overriding route_mm. False overrides an inherited request and allows automatic planning; null clears the explicit setting." },
                    "trim": { "type": ["boolean", "null"], "description": "Cut the thread after this object even if the next one is close." },
                    "route_mm": { "type": ["array", "null"], "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 }, "maxItems": 40, "description": "Route the thread to this object by hand, from the object sewn before it in the same thread: the points [[x, y], …] (mm from the hoop center) it's sewn through with running stitches, instead of being cut (or going the planner's way). Lay it where it won't be seen: along the edge of something in a like color, under what's sewn later, down a seam. [] sews straight there. Null: the planner decides again. get_cuts says where the thread is cut and where each cut's ends are; set start (and the object before's end) to choose where the way begins and ends. A trim on the object before still cuts." },
                    "stop": { "type": ["boolean", "null"], "description": "Pause the machine after this object (to lay foam, or trim appliqué fabric)." },
                    "hidden": { "type": ["boolean", "null"], "description": "Hide the object: it isn't shown or sewn, and keeps its stitch settings for when it's shown again." },
                    "locked": { "type": ["boolean", "null"], "description": "Lock the object: the person can't select or move it on the canvas (a background, so dragging over it selects what's on top). It sews as usual, and these tools still change it." }
                }
            }
        }),
        json!({
            "name": "delete_object",
            "description": "Delete an object, or several (objects). A filled shape's outline (the object named \"<shape> (outline)\") goes with it; deleting the outline alone leaves the shape. Naming an outline that already went with its shape is fine: it's said so.",
            "inputSchema": { "type": "object", "properties": {
                "object": obj_schema(),
                "objects": { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] }, "description": "Several objects to delete at once, instead of object." }
            } },
            "annotations": { "destructiveHint": true }
        }),
        json!({
            "name": "rename_object",
            "description": "Give an object a name (letters, digits and spaces). A filled shape and its outline share the name; its stitch settings stay.",
            "inputSchema": { "type": "object", "properties": { "object": obj_schema(), "name": { "type": "string" } }, "required": ["object", "name"] }
        }),
        json!({
            "name": "group_objects",
            "description": "Make one group of objects that are parts of one thing (the person then selects and moves them with one click, and Layers lists them together), or with `into` put objects into a group there already. They stay where they are and as they look, and are sewn together: a new group's objects where the last of them was, objects put `into` a group after the last of it (check the order with get_design where one must sew over another). A group needs a `name` that says what it is.",
            "inputSchema": { "type": "object", "properties": {
                "objects": { "description": "The objects: a list of names or indices.", "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } },
                "name": { "type": "string", "description": "What the new group is (\"bridge\", \"left wing\"): letters, digits and spaces." },
                "into": { "type": "string", "description": "An existing group (its name, as get_design's `group` gives it) to put the objects into, instead of making one." }
            }, "required": ["objects"] }
        }),
        json!({
            "name": "ungroup_objects",
            "description": "Take objects out of the group each is in (\"all\": every object out of its group). They stay where they are, as they look and where they're sewn; a group left with nothing in it is gone.",
            "inputSchema": { "type": "object", "properties": {
                "objects": { "description": "One object, a list of them, or \"all\".", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } }] }
            }, "required": ["objects"] }
        }),
        json!({
            "name": "align_objects",
            "description": "Line objects up, or space them evenly, each moved as a whole (a shape with its outline). align: their left, right, top or bottom edges, or their centers (center: side to side; middle: up and down), to the box round them all, or to the hoop (to: \"hoop\"; center and middle together put something in the middle of the hoop). distribute: their centers evenly spaced between the first and the last (three or more). One undo step.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "objects": { "description": "One object (index or name), a list of them, or \"all\".", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } }] },
                    "align": { "type": "string", "enum": ["left", "center", "right", "top", "middle", "bottom"] },
                    "distribute": { "type": "string", "enum": ["horizontal", "vertical"] },
                    "to": { "type": "string", "enum": ["objects", "hoop"], "description": "What align lines them up with. Default objects." }
                },
                "required": ["objects"]
            }
        }),
        json!({
            "name": "combine_shapes",
            "description": "Make one shape of several filled shapes (Pathfinder): unite (all of them, merged, so nothing is sewn twice where they overlap), minus_front (the one sewn first, less everything sewn after it: a hole or a notch), intersect (only where they all overlap) or exclude (where an odd number overlap). The result takes the first one's thread and stitch settings. Text must be made outlines first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "objects": { "description": "Two or more filled shapes (indexes or names).", "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } },
                    "op": { "type": "string", "enum": ["unite", "minus_front", "intersect", "exclude"], "description": "Default unite." }
                },
                "required": ["objects"]
            }
        }),
        json!({
            "name": "move_object",
            "description": "Move an object in the sewing order. Later objects sew on top of earlier ones.",
            "inputSchema": { "type": "object", "properties": { "object": obj_schema(), "to": { "type": "integer", "description": "New 0-based position." } }, "required": ["object", "to"] }
        }),
        json!({
            "name": "transform_objects",
            "description": "Move, resize, rotate or mirror objects (together, around their combined center). Applied in order: size/scale, mirror, rotate, then move. Fill angles turn with the objects. Changes are written into the SVG as transforms.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "objects": { "description": "One object (index or name) or a list of them.", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } }] },
                    "width_mm": { "type": "number", "description": "Resize to this width (keeps proportions unless height_mm is also given)." },
                    "height_mm": { "type": "number", "description": "Resize to this height." },
                    "scale": { "type": "number", "description": "Scale factor, e.g. 1.5." },
                    "mirror": { "type": "string", "enum": ["horizontal", "vertical"] },
                    "rotate_deg": { "type": "number", "description": "Clockwise degrees." },
                    "move_to_mm": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "Put the objects' center here [x, y] (mm from hoop center)." },
                    "move_by_mm": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "Shift by [dx, dy] mm." }
                },
                "required": ["objects"]
            }
        }),
        json!({
            "name": "duplicate_objects",
            "description": "Copy objects with all their settings. The copies sew last and are selected.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "objects": { "description": "One object (index or name) or a list of them.", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } }] },
                    "offset_mm": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "Where the copies go relative to the originals. Default [3, 3]." }
                },
                "required": ["objects"]
            }
        }),
        json!({
            "name": "add_text",
            "description": "Add lettering in a system font: straight, or on an arc (positive arc_mm curves over the top like a badge's top text, negative along the bottom). The text stays editable with edit_text. Satin letters should be at least 5 mm tall.",
            "inputSchema": {
                "type": "object",
                "properties": text_props(true),
                "required": ["text"]
            }
        }),
        json!({
            "name": "edit_text",
            "description": "Change a lettering object's text or settings. It keeps its position, rotation and stitch settings.",
            "inputSchema": {
                "type": "object",
                "properties": text_props(false),
                "required": ["object"]
            }
        }),
        json!({
            "name": "list_fonts",
            "description": "Font families installed on this computer, for add_text.",
            "inputSchema": { "type": "object", "properties": { "search": { "type": "string" } } },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "open_file",
            "description": "Open a file in a new tab (or the empty tab in front), which becomes the document the other tools work on: an SVG, or a machine file (pes, pec, dst, exp, jef, vp3) which opens as manual stitches you can move, scale and recolor. A file that's already open comes to the front instead, keeping its unsaved changes.",
            "inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "Absolute path." } }, "required": ["path"] }
        }),
        json!({
            "name": "list_documents",
            "description": "The documents open in the studio's tabs, in order: index, name, file, unsaved changes, and which one is in front (the one every other tool works on).",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "switch_document",
            "description": "Bring another open document to the front, so the other tools work on it. By its index from list_documents, or its name.",
            "inputSchema": { "type": "object", "properties": {
                "index": { "type": "integer" },
                "name": { "type": "string" }
            } }
        }),
        json!({
            "name": "new_document",
            "description": "Open a new, empty document in a new tab and bring it to the front (the document you were on stays open in its tab).",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "group_colors",
            "description": "Reorder objects to cut thread changes, without moving anything above an object it should sit under.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "set_document",
            "description": "Document-wide settings.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "fabric": { "type": "string", "enum": fabrics, "description": "Fabric preset; sets default densities and compensation." },
                    "machine": { "type": "string", "description": "The machine the design is for: an id or name from list_machines, or \"none\". Sets the hoop to the machine's default (a design already in one of its hoops stays there) unless hoop is given too, sets min_stitch_mm and lock to the machine's (the usual ones if it has none) unless given too, and makes the machine's file format the default for export." },
                    "min_stitch_mm": { "type": "number", "description": "No stitch shorter than this (0.1 to 1 mm; 0.1 is the usual), for machines that drop short stitches. Fill rows closer together than this are sewn in alternating passes and other short stitches merged, in the plan and in exported files. Choosing a machine sets it." },
                    "narrow_satin_mm": { "type": "number", "description": "Satins narrower than this on average (mm) get a too-narrow warning from the checks; 1 is the usual, 0 never warns. Set it to what sews cleanly on the fabric (0 to 3). Stitches don't depend on it." },
                    "lock": { "type": "object", "description": "The lock (tie-in and tie-off) stitches in exported files: style back (back and forth), triangle or none; each stitch's length (0.2 to 2 mm, never shorter than min_stitch_mm); how many (twos for back, threes for triangle). The usual is back, 0.4 mm, 4. Choosing a machine sets it.", "properties": { "style": { "type": "string", "enum": ["back", "triangle", "none"] }, "length_mm": { "type": "number" }, "count": { "type": "integer" } } },
                    "hoop": { "type": "string", "description": format!("The hoop's sewing field, width by height in mm. Standard hoops: {}. Any other size is a custom hoop, e.g. 150x220 (each side 10 to 1000 mm).", hoops.join(", ")) },
                    "remove_overlaps": { "type": "boolean", "description": "Skip fill hidden under later objects." },
                    "hat_order": { "type": "boolean", "description": "Sew center-out and bottom-up (for caps)." },
                    "over_sewn": { "type": "boolean", "description": "Where the thread can't get from one part of an object to the next out of sight (under stitches still to come, or under what's sewn later), sew it over the fill already sewn instead of cutting it: fewer trims, but the running stitch lies on top of the fill. Default false: cut." },
                    "connect_mm": { "type": "number", "description": "Sew across gaps up to this long (mm), between objects or the separate parts of one, with running stitches instead of cutting the thread, where there is no way round out of sight (under a fill or what is sewn on top: that travel is made anyway); 0 cuts those." },
                    "blend_mm": { "type": "number", "description": "Sew across gaps up to this long (0 to 15 mm; 0 is off) where the thread is close in color to what it would lie on, instead of cutting it: the same thread, a lighter or darker one, or bare cloth of its own color (preview_color is the cloth, so set that right first). The plainer it would show, the less far it goes: all of blend_mm on a matching thread, a bit over half for light red on red or navy on royal blue, nothing for white on black, never less than connect_mm. Saves trims where a line of thread won't be seen; get_design's stats say how many (blended_gaps)." },
                    "preview_color": { "type": "string", "description": "Fabric color for previews, #rrggbb (e.g. the hat's color). With blend_mm, also what a thread crossing bare cloth is judged against." }
                }
            }
        }),
        json!({
            "name": "list_machines",
            "description": "The machines a document can be set up for (set_document's machine): ZigZag's and the user's own. Each has its hoops (width by height in mm), the hoop a new design starts in, the file format it reads if known, the shortest stitch and locks it asks for if any, its link if ZigZag sends designs straight to it (skitch: send_to_skitch; the rest take an exported file), and whether it has been sewn on from ZigZag. Also the document's machine and the one new documents start with.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "render_preview",
            "description": "Render the stitches as they'll be sewn (PNG), on the preview fabric color with the hoop outline. With artwork true, render the drawing itself (the shapes as drawn, in their thread colors) instead, to tell a crude drawing from detail the stitching loses. Zoom in with region or object to check small details closely.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "width_px": { "type": "integer", "description": "Image width, 200–2000. Default 900." },
                    "show_jumps": { "type": "boolean", "description": "Draw jump stitches. Default true." },
                    "fit": { "type": "string", "enum": ["design", "hoop"], "description": "Frame the design closely or show the whole hoop. Default design." },
                    "artwork": { "type": "boolean", "description": "Draw the artwork as drawn instead of the stitches. Default false." },
                    "region": {
                        "type": "object",
                        "description": "Close-up: frame just this area (mm from the hoop center, like object positions).",
                        "properties": {
                            "x_mm": { "type": "number", "description": "Center x." },
                            "y_mm": { "type": "number", "description": "Center y." },
                            "width_mm": { "type": "number" },
                            "height_mm": { "type": "number" }
                        },
                        "required": ["x_mm", "y_mm", "width_mm", "height_mm"]
                    },
                    "object": { "description": "Close-up on one object (with a little room around it), named as in other tools.", "oneOf": [{ "type": "integer" }, { "type": "string" }] }
                }
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "get_annotations",
            "description": "The notes the person pinned on the canvas, still to be dealt with: each one's number, what it says, where (a point, or a box, in mm from the hoop center), the objects it's on, and whether it's new or was sent with an earlier request.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "resolve_note",
            "description": "Mark one of the person's canvas notes done (it leaves the canvas), with a few words on what you did about it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "number": { "type": "integer", "description": "The note's number (get_annotations)." },
                    "reply": { "type": "string", "description": "What you did, in a few words." }
                },
                "required": ["number"]
            }
        }),
        json!({
            "name": "start_variants",
            "description": "Start making versions for the person to compare: the design as it is now becomes the original they're shown beside. Then make each version and call save_variant.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "save_variant",
            "description": "Keep the design as it is now as a version (the person sees it beside the original and the others, and chooses), and put the original back to start the next version from.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "label": { "type": "string", "description": "How this version differs, in a few words (\"Bolder satin border\")." }
                },
                "required": ["label"]
            }
        }),
        json!({
            "name": "show_comparison",
            "description": "Show the person the latest versions side by side with the original (or, if there are none, the design before and after the last request), filling the canvas; or put that away (show: false) so the canvas shows the design again. start_variants shows it by itself.",
            "inputSchema": {
                "type": "object",
                "properties": { "show": { "type": "boolean", "description": "Default true." } }
            }
        }),
        json!({
            "name": "review_design",
            "description": "Review the whole design as an embroiderer would before it's sewn, and say what to fix: the sewing basics first (objects with no underlay where they need it, rows too far apart to cover the cloth, fills running the same way as what they sit on or touch, thread sewn across finished fill, satin underlay that doesn't suit its width, shapes sewn only as an empty outline, a patch border not sewn last or too narrow, lettering too small to read, lines sewn across a plain ground, specks the trace left sewn as loops), then what it costs to sew (threads, thread changes, trims and the objects behind most of them, threads sewn in more than one block, time), what may not read clearly (pieces under 1.5 mm, gaps sewn across in plain view, threads too close to the fabric's color, touching objects in threads too alike to tell apart), stitch types that may suit a shape better (a narrow band filled that satin would sew cleaner, a satin too wide) and fill angles for flow (rows along a piece's length) and dimension (touching pieces set apart), and the problems check_design finds. What document settings would save trims (what_ifs) is tried on copies. With a picture of the stitches at about their real size on a screen, to judge whether it reads. Use it after generating or changing artwork, and fix what costs or reads badly without taking out shading, highlights or features.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "offer_choices",
            "description": "Only when there are really different ways to go, or a trade-off that's the person's to weigh (not for a change that's clearly better, not by habit, and not more than one in a reply): ask them to pick one of a few changes, as a card in the chat: your question, then each option with what it gains and what it costs (how it changes the look), and the exact steps that make it. They click the one they want and the studio makes it at once (one undo step); you don't make it yourself. An option to do them all (when they go together) and a last one, keeping it as it is, are added for them (don't add those); a single choice shows as your question with a button for it and one for Not now. An optional change is offered this way, never only mentioned in your summary. Write for a beginner: each label in everyday words, and each detail saying what they'd see on the fabric and what it means for sewing, with no setting names or embroidery jargon left unexplained. Use it for changes that are theirs to decide: a document setting whose thread shows (review_design's what_ifs), a stitch type or fill angle you're unsure reads better, a bigger change to the look. Steps are tool calls as in a batch ({tool, arguments}); they can't open, save, export or switch documents, undo, or offer more choices.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "What the choices are about, short and clear: \"Fewer trims\"." },
                    "question": { "type": "string", "description": "What you ask the person, one sentence in plain words: \"Which of these should I do?\"" },
                    "combine": { "type": "boolean", "description": "Whether the changes can be made together, as an option to do them all (added when they don't set the same thing differently). Default true." },
                    "choices": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 4,
                        "items": {
                            "type": "object",
                            "properties": {
                                "label": { "type": "string", "description": "The change in a few words (one line), in everyday words a beginner knows: \"Let the thread run over the trunk\", not a setting's name." },
                                "detail": { "type": "string", "description": "One short sentence (about 90 characters; it's cut at two lines): what it does, in plain words: what they'd see on the fabric, and what it means for sewing. E.g. \"The machine stops to cut the thread 27 times instead of 89, so it sews faster, but a few short lines of thread will show on the trunk.\" No jargon (trims, travel, fill, satin) without saying what it means." },
                                "steps": { "type": "array", "minItems": 1, "items": { "type": "object", "properties": { "tool": { "type": "string" }, "arguments": { "type": "object" } }, "required": ["tool"] } }
                            },
                            "required": ["label", "steps"]
                        }
                    }
                },
                "required": ["title", "question", "choices"]
            }
        }),
        json!({
            "name": "check_design",
            "description": "List problems an experienced digitizer would flag (sizes, satin widths, hoop fit, lettering size).",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "get_cuts",
            "description": "Where the thread is cut as the design stands, in sewing order: for each cut, the object it's cut after and the one it starts again on (same thread), where the thread leaves and where it begins again (mm from the hoop center), and how far apart those are. To save one by hand, give the object it begins on a route_mm (set_object): a way for the thread to be sewn there out of sight. Also the ways already routed by hand and the gaps blended across (set_document's blend_mm).",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "get_svg",
            "description": "The document as SVG, with every object's stitch settings written into its id (what Save writes).",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "select",
            "description": "Select an object in the editor so the person sees it highlighted (null to clear).",
            "inputSchema": { "type": "object", "properties": { "object": { "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "null" }] } } }
        }),
        json!({
            "name": "undo",
            "description": "Undo the last change (yours or the person's).",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "redo",
            "description": "Redo the change last undone.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "save",
            "description": "Save the document as SVG (settings stored in object ids). Without a path, saves over the open file.",
            "inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "Absolute path ending in .svg." } } }
        }),
        json!({
            "name": "export",
            "description": "Write a machine file: pes (Brother, Baby Lock), dst (Tajima, most commercial machines), exp (Melco, Bernina), jef (Janome), vp3 (Husqvarna Viking, Pfaff), pec, or pen (raw Skitch stream). Or html: a printable production worksheet (picture, size, time, thread list with Brother/Janome names). Without a format, the one the path ends in (.dst), or else the one the document's machine reads.",
            "inputSchema": { "type": "object", "properties": { "format": { "type": "string", "enum": ["pes", "dst", "exp", "jef", "vp3", "pec", "pen", "html"] }, "path": { "type": "string", "description": "Absolute output path." }, "objects": { "type": "boolean", "description": "Also write PATH.objects.json beside a machine file: which object each stitch, jump and trim in it is for (the objects, and the runs of commands for each, numbered as the file has them), so a checker can name the shape behind a stitch. Not for pen or html. Default false." } }, "required": ["path"] }
        }),
        json!({
            "name": "trace_image",
            "description": "Trace a raster logo (png, jpg, webp, gif, bmp, tiff) into stitchable shapes and add them (or start a new document with them). Flat logos trace exactly (edges, corners, circles); photos are posterized. Use `palette` to keep to the design's threads, and `threads` to snap the rest to a thread chart. The result says which colors are new_colors (each one more thread change) and how many trims the design had before and has after.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path of the image." },
                    "colors": { "type": "integer", "description": "Thread colors, 2–10. Default: found automatically." },
                    "width_mm": { "type": "number", "description": "Width of the whole image, mm (the traced shapes are narrower when a white margin is left out). Default: 80% of the hoop." },
                    "x_mm": { "type": "number", "description": "Center x (mm from hoop center). Default 0." },
                    "y_mm": { "type": "number", "description": "Center y. Default 0." },
                    "remove_background": { "type": "boolean", "description": "Leave out white areas. Default true." },
                    "threads": { "type": "string", "enum": ["none", "brother", "janome"], "description": "Snap colors to this thread chart. Default none." },
                    "palette": { "description": "Threads to keep to: \"design\" (the threads the design already uses) or a list of #rrggbb. A traced color near one (within palette_reach) becomes it, so the trace adds no thread change for it; the rest are new colors (snapped to `threads` if given).", "oneOf": [{ "type": "string", "enum": ["design"] }, { "type": "array", "items": { "type": "string" } }] },
                    "palette_reach": { "type": "number", "description": "How different a color may look from a palette thread and still become it (CIEDE2000: 2 barely visible, 10 a shade off, 20 clearly different). Default 10." },
                    "lines": { "type": "boolean", "description": "Trace thin lines as single lines (run/satin) instead of thin fills. Default false." },
                    "line_width_mm": { "type": "number", "description": "With lines: the widest line traced as a line. Default 1.5." },
                    "detail": { "type": "number", "description": "0–100, how closely outlines follow the pixels. Default 60." },
                    "speckle_mm": { "type": "number", "description": "Drop areas smaller than this across. Default 1." },
                    "replace": { "type": "boolean", "description": "Start a new document with only the trace, keeping the document settings (fabric, hoop, machine, preview color). Default false." }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "add_outline",
            "description": "Give filled shapes an outline: a stroke along each shape's edge, as an object of its own (named \"<shape> (outline)\") with its own thread, stitch type and place in the sewing order, right after its shape to start with. It stays on the shape's edge when the shape is moved or reshaped. A running stitch unless kind says satin; change its stitch type, thread and width with set_object. Lines, text and shapes that already have an outline are left as they are.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "objects": { "description": "The shapes to outline: one (index or name) or a list of them.", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } }] },
                    "kind": { "type": "string", "enum": ["run", "satin"], "description": "How the outline is sewn. Default run." },
                    "width_mm": { "type": "number", "description": "How wide a satin outline is, centered on the edge. Default 2." },
                    "color": { "type": "string", "description": "Thread color #rrggbb. Default: a darker shade of each shape's thread." }
                },
                "required": ["objects"]
            }
        }),
        json!({
            "name": "break_into_columns",
            "description": "Break filled shapes sewn in satin into the columns they're sewn as, to see and adjust the route: each column becomes a two-rail satin object (its rails and rungs as paths), all in a group in the order ZigZag sews them. Satin columns one after another in one group, in the same thread and with the same settings, are sewn as one satin: in the order and direction they're in, the thread travelling on under the columns still to come and each column lapped under the one it ends against, so breaking a shape doesn't change how it's sewn. Afterwards reorder them with move_object, reshape their rails, or give one a trim with set_object to cut after it (the columns up to it are still one satin; a column with other settings of its own is sewn on its own). Text must be turned into shapes first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "objects": { "description": "The satin shapes to break up: one (index or name) or a list of them.", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } }] }
                },
                "required": ["objects"]
            }
        }),
        json!({
            "name": "measure_stitching",
            "description": "Measure how cleanly satin stitching comes out, to tell whether a change made it better. Call it before a change and again after, passing the first call's `measures` as `before`: it says which numbers moved and gives a verdict (better, worse, mixed or same). It plans just the objects given, on their own, with the document's settings, and counts what shows in the thread; less of each is better: trims (and trims_needed, the cuts the shape can't be sewn without, one fewer than its separate parts), travel_showing (travel stitches lying across finished stitching), stitches_outside (needle points more than 0.6 mm past the outline), stitches_leaving_shape (stitches that cross out of the shape and back), bare_mm2 (cloth inside the outline no thread covers), crowded_needle_points (four or more within 0.25 mm: the narrow end of a fan), seams (places a stroke is sewn in pieces and the stitch direction jumps), piled_mm2 (satin three columns deep), stitches_off_square (stitches more than 30° from square across their column), stitches_off_edge (the same against the shape's own edge), worst_lean_deg, pointed_ends (column ends that come to a point) and filled_parts (parts that couldn't be made columns and are filled instead). `faults` names the ones that aren't zero. A filled shape sewn in satin is held to its own outline. Columns broken out of a shape (break_into_columns) have no outline left to be held to: before breaking it, duplicate the shape and set the copy's kind to skip (kept, not sewn), then pass the copy as `within`; without one, the measures that need an outline are null. These are measures, not eyes: stitches outside the shape, bare cloth, piled satin and seams are counted well, a fan of stitches into a corner poorly, so look at render_preview too. check_design lists the design-wide problems (hoop fit, sizes, stitch lengths).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "objects": { "description": "The satin objects to measure together: one (index or name) or a list. Left out: every satin in the design.", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } }] },
                    "within": { "description": "Filled shapes whose outlines the stitching should cover and stay inside, besides the measured shapes' own: the unsewn copy of a shape that was broken into columns.", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } }] },
                    "before": { "type": "object", "description": "The `measures` of an earlier call, to compare with." }
                }
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "create_outlines",
            "description": "Turn text into plain shapes (Type › Create Outlines): its letters' outlines as one object that's no longer text, so it can be broken into columns (break_into_columns) or edited point by point (edit_path). It keeps its stitch settings and thread; the wording, font and size can't be changed afterwards.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "objects": { "description": "The text objects: one (index or name) or a list of them.", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } }] }
                },
                "required": ["objects"]
            }
        }),
        json!({
            "name": "get_path",
            "description": "An object's outline as its points, to read before edit_path: its subpaths in the path's order, each open or closed, each point with its place in mm (the design's coordinates, as center_mm in get_design) and its curve handles where it has them. For a two-rail satin (what break_into_columns makes) each subpath says whether it's a rail or a rung: the stitches run from rail to rail, and each rung fixes where on the two rails one stitch lands. Subpaths and points are counted from 1.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "object": { "description": "Index or name.", "oneOf": [{ "type": "integer" }, { "type": "string" }] }
                },
                "required": ["object"]
            }
        }),
        json!({
            "name": "edit_path",
            "description": "Change an object's outline point by point (the Direct Selection and Pen tools' edits), all in one undo step: move a point, add one, delete one, add a subpath or delete one. On a two-rail satin this is how a column is adjusted: move a rail's points to widen, narrow or straighten it, move a rung's two ends to turn the stitches there, add a rung (a two-point subpath crossing both rails) where the stitches should be held to an angle, or delete one that twists them. Read the points with get_path first; numbers are as it gives them (from 1) and refer to the path as it is before this call's edits when there's one edit; with several, each edit sees the ones before it. Text must be made outlines first (create_outlines). Check the result with check_design or render_preview.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "object": { "description": "Index or name.", "oneOf": [{ "type": "integer" }, { "type": "string" }] },
                    "edits": {
                        "type": "array",
                        "description": "The edits, applied in order.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "op": { "type": "string", "enum": ["move", "insert", "delete", "add_subpath", "delete_subpath"], "description": "move: a point (its handles go with it) to to_mm, or by by_mm. insert: a new point on the segment after point `after` (0: before the first point of an open subpath is not allowed; use the segment after 1), at the middle of the segment keeping its shape, or at at_mm as a corner. delete: a point. add_subpath: a new subpath through points_mm (a rung is two points, one just outside each rail). delete_subpath: a whole subpath (not a satin's rail)." },
                                "subpath": { "type": "integer", "description": "Which subpath, from 1 (all but add_subpath). May be left out when the object has only one." },
                                "point": { "type": "integer", "description": "Which point of it, from 1 (move, delete)." },
                                "after": { "type": "integer", "description": "insert: the point the new one comes after (the last point of a closed subpath: between it and the first)." },
                                "to_mm": { "type": "array", "items": { "type": "number" }, "description": "move: where to, [x, y]." },
                                "by_mm": { "type": "array", "items": { "type": "number" }, "description": "move: how far, [dx, dy]." },
                                "at_mm": { "type": "array", "items": { "type": "number" }, "description": "insert: where, [x, y]. Left out: the middle of the segment." },
                                "points_mm": { "type": "array", "items": { "type": "array", "items": { "type": "number" } }, "description": "add_subpath: its points, [[x, y], …], two or more." },
                                "closed": { "type": "boolean", "description": "add_subpath: closed. Default false." }
                            },
                            "required": ["op"]
                        }
                    }
                },
                "required": ["object", "edits"]
            }
        }),
        json!({
            "name": "round_corners",
            "description": "Round (or chamfer) the sharp corners of objects' outlines. Each corner rounds as far as its sides allow, so neighbouring cuts never overlap. Satin and fill stitch more cleanly round sharp corners.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "objects": { "description": "The objects to round: one (index or name) or a list of them.", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } }] },
                    "radius_mm": { "type": "number", "description": "Corner radius. Default 1.5." },
                    "kind": { "type": "string", "enum": ["round", "inverted", "chamfer"], "description": "Default round." }
                },
                "required": ["objects"]
            }
        }),
        json!({
            "name": "simplify",
            "description": "Simplify objects' outlines: fewer points, smooth curves (Illustrator's and VectorCraft's Simplify). Fewer points stitch more smoothly, especially traced artwork. Corners stay corners.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "objects": { "description": "The objects to simplify: one (index or name) or a list of them.", "oneOf": [{ "type": "integer" }, { "type": "string" }, { "type": "array", "items": { "oneOf": [{ "type": "integer" }, { "type": "string" }] } }] },
                    "level": { "type": "number", "description": "0–100: 0 for the fewest points, 100 to stay closest to the original (each 10 halves how far the outline may move; 50 is 1 pt). Default: Auto, the level for the objects' size." }
                },
                "required": ["objects"]
            }
        }),
        json!({
            "name": "clean_up",
            "description": "Clean up the whole document (Object > Path > Clean Up): remove stray points (objects with one point or no size), objects too small to sew, and extra points (points on top of each other, points in the middle of straight lines). Lettering is left alone. Says what it removed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "stray_points": { "type": "boolean", "description": "Default true." },
                    "tiny_objects": { "type": "boolean", "description": "Default true." },
                    "tiny_mm": { "type": "number", "description": "With tiny_objects: remove objects smaller than this across. Default 1." },
                    "extra_points": { "type": "boolean", "description": "Default true." },
                    "dry_run": { "type": "boolean", "description": "Only count what it would remove. Default false." }
                }
            }
        }),
        json!({
            "name": "simplify_design",
            "description": "Simplify the whole design so it is quicker and easier to sew (Object > Simplify Design): merge the threads most alike down to a number, take out details too small to sew well, sew across short gaps instead of trimming, and sew each thread together. Give only what you want changed. Returns what the design costs to sew before and after (threads, thread changes, trims, stitches, minutes) and what was merged or removed. One undo step.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "threads": { "type": "integer", "minimum": 1, "description": "Merge the threads most alike until this many are left. Each pair becomes the one that sews more (a real thread, not a blend). Default: keep them all." },
                    "min_size_mm": { "type": "number", "description": "Remove objects smaller than this across, 0–10 mm (lettering is left alone). Default 0: none." },
                    "connect_mm": { "type": "number", "description": "Sew across gaps up to this long instead of cutting the thread, 0–10 mm (the document's connect_mm; never lowered). Default: leave it." },
                    "group_colors": { "type": "boolean", "description": "Reorder so each thread sews in as few goes as the layering allows. Default true." },
                    "unite": { "type": "string", "enum": ["off", "similar", "all"], "description": "Make one shape of objects that touch, sew one after another and share a thread (after any merging and regrouping), so nothing is sewn under or between them. similar: only objects with the same stitch type (fill with fill, satin with satin), so a satin detail on a fill stays. all: whatever they are sewn as; fewest stitches, but detail in the same thread is lost. Lettering and locked objects are left alone. Default off." },
                    "dry_run": { "type": "boolean", "description": "Only report what it would come to. Default false." }
                }
            }
        }),
        json!({
            "name": "batch",
            "description": "Run several tool calls in one go, in order. Atomic by default: if any step fails nothing changes, and the whole batch is one undo step. When a step fails, its error says why (and which earlier step removed or renamed an object it names) and gives the step as it was sent: fix that step, or leave it out, and send the batch again. A string argument \"$N.field\" (e.g. \"$0.added.0.name\") is replaced by that field of step N's JSON result; write \"$$\" for a literal \"$\". A step is {\"tool\": name, \"arguments\": {…}}. open_file, new_document, switch_document and undo can't run in an atomic batch, nor a batch in a batch. The stitches are planned once, at the end (and before any step that reads them: get_design, render_preview, check_design, export), so stitch counts in other steps' results are from before the batch.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "maxItems": 64,
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool": { "type": "string" },
                                "arguments": { "type": "object" }
                            },
                            "required": ["tool"]
                        }
                    },
                    "atomic": { "type": "boolean", "description": "Roll everything back if a step fails, and undo as one step. False: stop at the step that fails and keep the ones before it. Default true." },
                    "label": { "type": "string", "description": "What the batch did, shown to the person." }
                },
                "required": ["steps"]
            }
        }),
    ];
    // Hints for clients: which tools only read, and which can lose work.
    for t in &mut tools {
        let name = t["name"].as_str().unwrap_or("").to_string();
        let read_only = only_reads(&name);
        // Every tool that changes something can answer in a line.
        if !read_only {
            t["inputSchema"]["properties"]["terse"] = json!({
                "type": "boolean",
                "description": if name == "batch" {
                    "Answer in a few lines instead of every step's full result: ok, how many steps ran, the names of objects added, the results of steps that only read (get_design, check_design, …), and new_problems (design problems that weren't there before the batch). Later steps can still use earlier results. Default false."
                } else {
                    "Answer in a line instead of the full result: ok, the names of any objects added, and new_problems (design problems that weren't there before this call). Default false."
                }
            });
        }
        let destructive = matches!(
            name.as_str(),
            "delete_object" | "open_file" | "export" | "save"
        );
        if t.get("annotations").is_none() {
            t["annotations"] = if read_only {
                json!({ "readOnlyHint": true })
            } else {
                json!({ "readOnlyHint": false, "destructiveHint": destructive })
            };
        }
    }
    tools.push(json!({
        "name": "send_to_skitch",
        "description": "Send the design to the connected Brother Skitch, for a document whose machine is a Skitch or that has none (the studio's Skitch window connects; ask the person to connect there first if machine_status says it isn't). The window then walks the person through tracing the outline and sewing. Check the design and get the person's go-ahead first.",
        "inputSchema": { "type": "object", "properties": {} },
        "annotations": { "destructiveHint": false, "openWorldHint": true }
    }));
    tools.push(json!({
        "name": "machine_status",
        "description": "How the Skitch is doing: connected or not, what it's doing, any error and what to do about it, which thread is on and which comes next, and how far through the design it is.",
        "inputSchema": { "type": "object", "properties": {} },
        "annotations": { "readOnlyHint": true }
    }));
    Value::Array(tools)
}

/// Tools that only look: they change nothing, and their results are what
/// was asked for.
/// A tool that reads the stitches (so they're planned before it runs).
pub(crate) fn reads_stitches(name: &str) -> bool {
    matches!(
        name,
        "get_design"
            | "get_cuts"
            | "render_preview"
            | "check_design"
            | "review_design"
            | "export"
            | "send_to_skitch"
    )
}

fn only_reads(name: &str) -> bool {
    matches!(
        name,
        "get_design"
            | "list_fonts"
            | "render_preview"
            | "check_design"
            | "review_design"
            | "get_svg"
            | "list_documents"
            | "list_machines"
            | "get_annotations"
            | "measure_stitching"
            | "get_cuts"
            | "get_path"
            | "machine_status"
    )
}

/// A problem's message without its measurements (`Satin only # mm wide…`):
/// the same problem a little bigger or smaller isn't a new one.
fn problem_kind(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut chars = message.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            if !out.ends_with('#') {
                out.push('#');
            }
        } else if c == '.' && out.ends_with('#') && chars.peek().is_some_and(char::is_ascii_digit) {
            // (A decimal point inside a number.)
        } else {
            out.push(c);
        }
    }
    out
}

/// A place 0–1 along an outline. (1 is the far end, not the start again.)
fn along(x: f64) -> f64 {
    if x == 1.0 {
        1.0 - 1e-9
    } else {
        x.rem_euclid(1.0)
    }
}

/// The color a thread route's line is drawn in: no thread's.
pub(crate) const ROUTE_RGB: [u8; 3] = [255, 204, 0];

/// A line with the points that don't change where it goes left out.
pub(crate) fn simplified(pts: &[P], tol: f64) -> Vec<P> {
    zigzag::design::simplify_line(pts, tol)
}

/// A batch step's argument that stands for an earlier step's result.
fn is_ref(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| s.starts_with('$') && !s.starts_with("$$"))
}

/// The names of the objects a result says were added, if it says.
fn added_names(result: &Value) -> Vec<Value> {
    // (`made`: what break_into_columns and create_outlines turn objects into.)
    ["added", "made"]
        .iter()
        .filter_map(|k| result[*k].as_array())
        .flatten()
        .map(|o| o.get("name").cloned().unwrap_or_else(|| o.clone()))
        .collect()
}

/// A tool result's text.
fn text_of(r: &ToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.as_str()),
            Content::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Refuse arguments a tool doesn't take (a misspelt `widht_mm` would
/// otherwise be ignored without a word).
fn check_args(name: &str, args: &Value) -> Result<(), String> {
    static SPECS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let specs = SPECS.get_or_init(tool_specs);
    let Some(spec) = specs
        .as_array()
        .and_then(|a| a.iter().find(|t| t["name"] == name))
    else {
        return Ok(()); // unknown tools are reported by run_tool
    };
    let Some(obj) = args.as_object() else {
        return if args.is_null() {
            Ok(())
        } else {
            Err(format!("{name}: arguments must be an object"))
        };
    };
    let props = spec["inputSchema"]["properties"].as_object();
    let known = |k: &str| props.is_some_and(|p| p.contains_key(k));
    let unknown: Vec<&String> = obj.keys().filter(|k| !known(k)).collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let mut expected: Vec<&String> = props.map(|p| p.keys().collect()).unwrap_or_default();
    expected.sort();
    Err(format!(
        "{name} doesn't take {}. It takes: {}.",
        unknown
            .iter()
            .map(|k| format!("`{k}`"))
            .collect::<Vec<_>>()
            .join(", "),
        if expected.is_empty() {
            "no arguments".to_string()
        } else {
            expected
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    ))
}

/// Field `key` of `v`: an element, by number, of an array.
fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match key.parse::<usize>() {
        Ok(i) if v.is_array() => v.get(i),
        _ => v.get(key),
    }
}

/// Replace "$N.path" strings in `v` by that field of step N's result.
fn resolve_refs(v: &Value, results: &[Value]) -> Result<Value, String> {
    Ok(match v {
        // "$$…" is a literal "$…".
        Value::String(s) if s.starts_with("$$") => Value::String(s[1..].to_string()),
        Value::String(s) if s.starts_with('$') => {
            let body = &s[1..];
            let (step, path) = body.split_once('.').unwrap_or((body, ""));
            let Ok(n) = step.parse::<usize>() else {
                return Ok(v.clone());
            };
            let mut cur = results
                .get(n)
                .ok_or(format!("{s}: there is no earlier step {n}"))?;
            for (k, key) in path.split('.').filter(|k| !k.is_empty()).enumerate() {
                // (A step that added one object: "$N.name" is its name, as
                // for a tool that answers with the object itself.)
                // (Whether the step answered in full, each object with
                // its name, or tersely, with the names alone; and `made`
                // as well as `added`, as `added_names` reads them.)
                let only_added = || {
                    let all: Vec<&Value> = ["added", "made"]
                        .iter()
                        .filter_map(|a| cur.get(*a).and_then(Value::as_array))
                        .flatten()
                        .collect();
                    match all.as_slice() {
                        [one] if k == 0 => field(one, key)
                            .or_else(|| (key == "name" && one.is_string()).then_some(*one)),
                        _ => None,
                    }
                };
                cur = match field(cur, key).or_else(only_added) {
                    Some(v) => v,
                    None => {
                        let has: Vec<String> = cur
                            .as_object()
                            .map(|o| o.keys().map(|k| format!("`{k}`")).collect())
                            .unwrap_or_default();
                        // (A terse step lists what it added by name alone.)
                        let named = cur
                            .get("added")
                            .and_then(Value::as_array)
                            .and_then(|a| a.first())
                            .is_some_and(Value::is_string);
                        let eg = if named { "added.0" } else { "added.0.name" };
                        return Err(if has.is_empty() {
                            format!("{s}: step {n}'s result has no `{key}`")
                        } else {
                            format!(
                                "{s}: step {n}'s result has no `{key}`; it has {} (an object it added is \"${n}.{eg}\")",
                                has.join(", ")
                            )
                        });
                    }
                };
            }
            cur.clone()
        }
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|x| resolve_refs(x, results))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| Ok((k.clone(), resolve_refs(x, results)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// How many subpaths object `s` is drawn as.
fn rails_count(s: &zigzag::svg::Shape) -> usize {
    match &s.geometry {
        Geometry::Stroke { lines, .. } => lines.len(),
        Geometry::Fill { rings, .. } => rings.len(),
    }
}

fn mm(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn issue_json(i: &Issue, names: &[String]) -> Value {
    json!({
        "level": match i.level { Level::Error => "error", Level::Warning => "warning", Level::Info => "note" },
        "object": i.shape.and_then(|s| names.get(s).cloned()),
        "index": i.shape,
        "message": i.message,
    })
}

impl Studio {
    /// Resolve an object reference: index, exact name, or a unique partial name.
    pub(crate) fn resolve(&self, v: Option<&Value>) -> Result<usize, String> {
        let n = self.doc.shapes.len();
        match v {
            Some(Value::Number(x)) => {
                let i = x
                    .as_u64()
                    .ok_or("object index must be a non-negative integer")?
                    as usize;
                if i < n {
                    Ok(i)
                } else {
                    Err(format!(
                        "no object {i}; there are {n} (0..{})",
                        n.saturating_sub(1)
                    ))
                }
            }
            Some(Value::String(s)) => {
                let lower = s.to_lowercase();
                if let Some(i) = self.names.iter().position(|x| x.to_lowercase() == lower) {
                    return Ok(i);
                }
                if let Some(i) = self.doc.shapes.iter().position(|x| x.id == *s) {
                    return Ok(i);
                }
                // The name as it's written in an id: `Face_mask`, or the whole
                // id with its settings (`Face_mask_fill_a=45`), for "Face mask".
                // An agent that wrote the SVG knows its objects by those.
                let words = zigzag::svg::name_words(s).join(" ").to_lowercase();
                let lower = if words.is_empty() { lower } else { words };
                if let Some(i) = self.names.iter().position(|x| x.to_lowercase() == lower) {
                    return Ok(i);
                }
                let hits: Vec<usize> = self
                    .names
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| x.to_lowercase().contains(&lower))
                    .map(|(i, _)| i)
                    .collect();
                match hits.as_slice() {
                    [i] => Ok(*i),
                    [] => Err(no_object_named(s, &self.names)),
                    _ => Err(format!(
                        "'{s}' matches several objects: {}",
                        hits.iter()
                            .map(|&i| self.names[i].clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                }
            }
            _ => Err("`object` must be an index or a name".into()),
        }
    }

    fn object_json(&self, i: usize) -> Value {
        let s = &self.doc.shapes[i];
        let f = self.doc.settings.fabric();
        let p = &s.params;
        let kind = p.kind.unwrap_or(Kind::Fill);
        let b = shape_bounds(s);
        let mut settings = serde_json::Map::new();
        if let Some(via) = &p.via {
            let via: Vec<Value> = via.iter().map(|(x, y)| json!([x, y])).collect();
            settings.insert("route_mm".into(), Value::Array(via));
        }
        match kind {
            Kind::Fill => {
                settings.insert("angle".into(), json!(p.fill_angle()));
                settings.insert("spacing_mm".into(), json!(p.fill_spacing(&f)));
                settings.insert("length_mm".into(), json!(p.fill_len(&f)));
                settings.insert("pull_mm".into(), json!(p.fill_pull(&f)));
                settings.insert("underlay".into(), json!(p.underlay_on()));
            }
            Kind::Satin => {
                let w = crate::app::mean_satin_width(s);
                let ul = p.satin_underlay_for(&f, w);
                let names: Vec<&str> = [
                    (underlay::CENTER, "center"),
                    (underlay::CONTOUR, "contour"),
                    (underlay::ZIGZAG, "zigzag"),
                ]
                .iter()
                .filter(|(b, _)| ul & b != 0)
                .map(|(_, n)| *n)
                .collect();
                settings.insert("mean_width_mm".into(), json!(mm(w)));
                if matches!(s.geometry, Geometry::Fill { .. }) {
                    settings.insert("border".into(), json!(p.is_border()));
                    settings.insert("cut".into(), json!(p.satin_cut()));
                    settings.insert("mitred".into(), json!(p.satin_mitred()));
                    // (Overlapping strokes aren't offered; said only of an
                    // object a file has them on.)
                    if p.satin_overlap() {
                        settings.insert("overlap".into(), json!(true));
                    }
                }
                settings.insert("spacing_mm".into(), json!(p.satin_spacing(&f)));
                settings.insert("pull_mm".into(), json!(p.satin_pull(&f)));
                settings.insert("satin_underlay".into(), json!(names));
                settings.insert(
                    "satin_underlay_auto".into(),
                    json!(p.satin_underlay.is_none()),
                );
                settings.insert("short_stitches".into(), json!(p.short_stitches_on()));
                settings.insert("split_mm".into(), json!(p.satin_split().unwrap_or(0.0)));
                settings.insert("lean".into(), json!(p.satin_lean()));
                settings.insert("even".into(), json!(p.satin_even()));
                if let Some((a, b)) = p.rails {
                    settings.insert("rails".into(), json!([a, b]));
                }
            }
            Kind::Run => {
                settings.insert("length_mm".into(), json!(p.run_len(&f)));
                settings.insert("repeats".into(), json!(p.run_repeats()));
            }
            Kind::Contour => {
                settings.insert("spacing_mm".into(), json!(p.contour_spacing()));
                settings.insert("length_mm".into(), json!(p.run_len(&f)));
                settings.insert("spiral".into(), json!(p.spiral == Some(true)));
            }
            Kind::Blanket => {
                settings.insert(
                    "spacing_mm".into(),
                    json!(
                        p.spacing
                            .unwrap_or(zigzag::params::defaults::BLANKET_SPACING)
                    ),
                );
                settings.insert(
                    "width_mm".into(),
                    json!(p.width.unwrap_or(zigzag::params::defaults::BLANKET_WIDTH)),
                );
            }
            Kind::Applique => {
                settings.insert(
                    "width_mm".into(),
                    json!(p.width.unwrap_or(zigzag::params::defaults::APPLIQUE_WIDTH)),
                );
                settings.insert("spacing_mm".into(), json!(p.satin_spacing(&f)));
                settings.insert(
                    "steps".into(),
                    json!([
                        "placement line",
                        "stop: lay fabric",
                        "tack-down",
                        "stop: trim fabric",
                        "satin cover"
                    ]),
                );
            }
            Kind::Meander => {
                settings.insert("spacing_mm".into(), json!(p.meander_spacing()));
            }
            Kind::Ripple => {
                settings.insert("spacing_mm".into(), json!(p.ripple_spacing()));
                settings.insert("spiral".into(), json!(p.spiral == Some(true)));
            }
            Kind::Cross => {
                settings.insert("cell_mm".into(), json!(p.cross_cell()));
            }
            Kind::Tartan => {
                settings.insert(
                    "sett".into(),
                    json!(
                        p.tartan_sett(s.color)
                            .iter()
                            .map(|(c, w)| json!({ "color": hex(*c), "width_mm": w }))
                            .collect::<Vec<_>>()
                    ),
                );
            }
            Kind::Skip | Kind::Manual => {}
        }
        if kind != Kind::Skip {
            for (k, v) in [("start", p.start), ("end", p.end)] {
                if let Some(t) = v {
                    settings.insert(k.into(), json!(t));
                }
            }
            if let Some(on) = p.trim_before {
                settings.insert("trim_before".into(), json!(on));
            }
            for (k, v) in [("trim", p.trim), ("stop", p.stop)] {
                if v == Some(true) {
                    settings.insert(k.into(), json!(true));
                }
            }
        }
        // (Said when they're so: hidden isn't sewn, locked can't be
        // selected on the canvas.)
        for (k, on) in [("hidden", p.hidden()), ("locked", p.locked())] {
            if on {
                settings.insert(k.into(), json!(true));
            }
        }
        json!({
            "index": i,
            "name": self.names.get(i),
            // (By its name; one nobody named, by its id, which tells it
            // from the others and names it to group_objects.)
            "group": self.shown_group(i).map(|g| {
                if zigzag::svg::name_words(&g).is_empty() {
                    g
                } else {
                    Studio::group_label(&g)
                }
            }),
            "kind": kind.name(),
            "allowed_kinds": allowed_kinds(s).iter().map(|k| k.name()).collect::<Vec<_>>(),
            "lettering": self.doc.lettering(i).map(|t| json!({ "text": t.text, "font": t.font, "size_mm": t.size_mm, "spacing_mm": t.spacing_mm, "arc_mm": t.arc_mm, "bold": t.bold, "italic": t.italic, "leading": t.leading, "align": t.align.name(), "h_scale": t.h_scale, "v_scale": t.v_scale, "case": t.case.name(), "on_path": t.path.is_some(), "path_offset": t.path_offset, "path_flip": t.path_flip, "kern": t.kern.iter().map(|(k, v)| json!({ "at": k, "mm": v })).collect::<Vec<_>>() })),
            "prompt": self.doc.prompt(i),
            "source": match (s.part, &s.geometry) { (Part::Stroke, _) => "stroke", (_, Geometry::Fill { .. }) => "filled shape", _ => "stroke" },
            "color": hex(s.color),
            "center_mm": [mm(b.center().x / 10.0), mm(b.center().y / 10.0)],
            "size_mm": [mm(b.width() / 10.0), mm(b.height() / 10.0)],
            "stitches": self.design.per_shape.get(i).copied().unwrap_or(0),
            "settings": settings,
            "problems": self.issues.iter().filter(|x| x.shape == Some(i)).map(|x| x.message.clone()).collect::<Vec<_>>(),
        })
    }

    /// The open documents, for list_documents.
    fn documents_json(&self) -> Value {
        let front = self.active_tab();
        json!({
            "documents": self.tab_infos().iter().enumerate().map(|(i, t)| json!({
                "index": i,
                "name": t.name,
                "file": t.path.as_ref().map(|p| p.display().to_string()),
                "unsaved_changes": t.modified,
                "in_front": i == front,
            })).collect::<Vec<_>>(),
        })
    }

    /// `list_machines`: every machine, the document's and the one new
    /// documents start with.
    fn machines_json(&self) -> Value {
        let hoop = zigzag::document::format_hoop;
        json!({
            "machines": self.machines.all().map(|p| json!({
                "id": p.id,
                "name": p.name,
                "hoops": p.hoops.iter().map(|&h| hoop(h)).collect::<Vec<_>>(),
                "default_hoop": hoop(p.default_hoop),
                "format": p.format,
                "min_stitch_mm": p.min_stitch_mm,
                "lock": p.lock.map(zigzag::machine::profile::lock_json),
                "link": p.link,
                "sewn_from_zigzag": p.tested,
                "source": p.source,
                "yours": self.machines.is_mine(&p.id),
            })).collect::<Vec<_>>(),
            "document": self.doc.settings.machine,
            "new_documents": self.machines.current,
        })
    }

    /// The path and the object for Route Thread Along Path: an open line
    /// (the way, drawn with the pen or any line tool) selected with the
    /// object the thread is to be sewn to along it; or just the line, if
    /// it's the one Edit Thread Route made of an object's route.
    pub(crate) fn route_pair(&self) -> Option<(usize, usize)> {
        let sel = self.sel_indices();
        let line = |i: usize| {
            self.doc.lettering(i).is_none()
                && matches!(&self.doc.shapes[i].geometry,
                    Geometry::Stroke { lines, .. } if lines.len() == 1 && !lines[0].1 && lines[0].0.len() >= 2)
        };
        let at = |uid: usize| self.doc.shapes.iter().position(|s| s.uid == uid);
        match sel.as_slice() {
            &[a, b] => match (line(a), line(b)) {
                (true, false) => Some((a, b)),
                (false, true) => Some((b, a)),
                // (Two lines: the way is the one that isn't sewn, or else
                // the one drawn last.)
                (true, true) => {
                    let skip = |i: usize| self.doc.shapes[i].params.kind == Some(Kind::Skip);
                    Some(if skip(a) && !skip(b) { (a, b) } else { (b, a) })
                }
                _ => None,
            },
            &[a] if line(a) => {
                let (guide, target) = self.route_edit?;
                (self.doc.shapes[a].uid == guide)
                    .then(|| at(target))
                    .flatten()
                    .map(|t| (a, t))
            }
            _ => None,
        }
    }

    /// Object › Route Thread Along Path: the selected line becomes the way
    /// the thread is sewn to the selected object (its `route_mm`), and
    /// goes. One undo step.
    pub(crate) fn route_along_selected(&mut self) {
        let Some((guide, target)) = self.route_pair() else {
            self.status =
                "Select the line to route along and the object the thread goes to.".into();
            return;
        };
        let Geometry::Stroke { lines, .. } = &self.doc.shapes[guide].geometry else {
            return;
        };
        let mut way = lines[0].0.clone();
        // Towards the object: the end nearer it is where it arrives.
        let b = shape_bounds(&self.doc.shapes[target]);
        if !b.is_empty() && way[0].dist(b.center()) < way[way.len() - 1].dist(b.center()) {
            way.reverse();
        }
        // No more points than say where it goes (a curve comes flattened
        // into many), and no more than a name holds.
        let mut tol = 1.5;
        let mut pts = simplified(&way, tol);
        while pts.len() > 40 {
            tol *= 1.6;
            pts = simplified(&way, tol);
        }
        let uid = self.doc.shapes[target].uid;
        let name = self.names[target].clone();
        let route: Vec<Value> = pts
            .iter()
            .map(|q| json!([q.x / 10.0, q.y / 10.0]))
            .collect();
        let r = self.run_tool(
            "batch",
            &json!({ "terse": true, "steps": [
                { "tool": "set_object", "arguments": { "object": target, "route_mm": route } },
                { "tool": "delete_object", "arguments": { "object": guide } },
            ] }),
        );
        if r.is_error {
            self.status = format!("Couldn't route it: {}", text_of(&r));
            return;
        }
        self.route_edit = None;
        if let Some(i) = self.doc.shapes.iter().position(|s| s.uid == uid) {
            self.set_selection(&[i]);
        }
        self.status = format!(
            "The thread is sewn to {name} along that line ({} points).",
            pts.len()
        );
    }

    /// Object › Edit Thread Route: the selected object's route as a line
    /// (yellow, not sewn) to reshape with the path tools; Route Thread
    /// Along Path puts it back.
    pub(crate) fn edit_route_as_path(&mut self) {
        let Some(target) = self
            .sel_indices()
            .into_iter()
            .find(|&i| self.doc.shapes[i].params.via.is_some())
        else {
            self.status = "Select an object the thread is routed to.".into();
            return;
        };
        // As it's sewn (from where the thread was), or else as it's kept.
        let way: Vec<P> = self
            .design
            .routes
            .iter()
            .find(|(i, _)| *i == target)
            .map(|(_, w)| w.clone())
            .or_else(|| {
                let via = self.doc.shapes[target].params.via.as_ref()?;
                Some(
                    via.iter()
                        .map(|&(x, y)| P::new(x * 10.0, y * 10.0))
                        .collect(),
                )
            })
            .unwrap_or_default();
        if way.len() < 2 {
            self.status =
                "That route is straight: draw a line for it and use Route Thread Along Path."
                    .into();
            return;
        }
        let path = vec![zigzag::bezier::SubPath {
            anchors: way
                .iter()
                .map(|&q| zigzag::bezier::Anchor::corner(q))
                .collect(),
            closed: false,
        }];
        let uid = self.doc.shapes[target].uid;
        let mut idx = 0;
        match self.structural(|d| {
            idx = d.add_drawing_path(&path, ROUTE_RGB, Kind::Skip)?;
            Ok(())
        }) {
            Ok(()) => {
                self.replan_soon();
                self.route_edit = Some((self.doc.shapes[idx].uid, uid));
                self.set_selection(&[idx]);
                self.status =
                    "Reshape the yellow line, then Object › Route Thread Along Path.".into();
            }
            Err(e) => self.status = format!("Couldn't make its line: {e:#}"),
        }
    }

    /// Object › Clear Thread Route: the planner decides again.
    pub(crate) fn clear_routes(&mut self) {
        let idx: Vec<usize> = self
            .sel_indices()
            .into_iter()
            .filter(|&i| self.doc.shapes[i].params.via.is_some())
            .collect();
        if idx.is_empty() {
            return;
        }
        let r = self.run_tool("set_object", &json!({ "objects": idx, "route_mm": null }));
        self.status = if r.is_error {
            text_of(&r)
        } else {
            "Thread route cleared.".into()
        };
    }

    /// For a cut from `from` to `to` in `thread`, before object `target`:
    /// what a straight line between them lies on, stretch by stretch, and
    /// (`route`: between two objects, where a route can be given) the way
    /// that shows least.
    fn cut_ground(
        &self,
        target: usize,
        thread: [u8; 3],
        from: P,
        to: P,
        route: bool,
    ) -> (Value, Option<Value>) {
        use zigzag::design::{Backdrop, contrast, coverage};
        let d = &self.design;
        // What's sewn before it, the last first.
        let place = d.sew_order.iter().position(|&i| i == target).unwrap_or(0);
        let before: Vec<usize> = d.sew_order[..place]
            .iter()
            .rev()
            .copied()
            .filter(|&i| {
                self.doc
                    .shapes
                    .get(i)
                    .is_some_and(|s| !s.params.hidden() && s.params.kind != Some(Kind::Skip))
            })
            .collect();
        let mut names: Vec<usize> = Vec::new();
        let under: Vec<_> = before
            .iter()
            .filter_map(|&i| {
                let (rings, rule) = coverage(&self.doc.shapes[i])?;
                let mut b = zigzag::geom::Bounds::empty();
                rings.iter().flatten().for_each(|&q| b.add(q));
                names.push(i);
                Some((b, rings, rule, self.doc.shapes[i].color))
            })
            .collect();
        let ground = Backdrop {
            under: &under,
            // (Between two objects: none of the one it's coming to is sewn yet.)
            own: None,
            cloth: self.doc.settings.cloth,
            thread,
        };
        // Straight across: runs of the same thing under it.
        let over = |q: P| -> (Option<usize>, [u8; 3]) {
            under
                .iter()
                .position(|(b, rings, rule, _)| {
                    q.x >= b.min.x
                        && q.x <= b.max.x
                        && q.y >= b.min.y
                        && q.y <= b.max.y
                        && zigzag::geom::point_inside(rings, *rule, q)
                })
                .map_or((None, ground.cloth), |k| (Some(names[k]), under[k].3))
        };
        let gap = from.dist(to);
        let n = (gap / 5.0).ceil().max(1.0) as usize;
        let mut runs: Vec<(Option<usize>, [u8; 3], f64)> = Vec::new();
        for k in 0..n {
            let (on, color) = over(from.lerp(to, (k as f64 + 0.5) / n as f64));
            match runs.last_mut() {
                Some(r) if r.0 == on => r.2 += gap / n as f64,
                _ => runs.push((on, color, gap / n as f64)),
            }
        }
        let crosses: Vec<Value> = runs
            .iter()
            .map(|&(on, color, len)| {
                json!({
                    "over": on.map_or("the cloth".to_string(), |i| self.names.get(i).cloned().unwrap_or_default()),
                    "color": hex(color),
                    "mm": mm(len / 10.0),
                    "shows": mm(contrast(thread, color)),
                })
            })
            .collect();
        if !route {
            return (Value::Array(crosses), None);
        }
        // The way that shows least: each stretch counted for more the
        // plainer it would be (and not at all where it's as plain as can be).
        let reach = |w: f64| if w >= 0.98 { 0.0 } else { 1.0 - w };
        // (Straight across, unless some way round costs less than that.)
        let most = (gap * 1.6 + 60.0).min(600.0);
        let straight = ground
            .spends(&[from, to], &reach)
            .map(|c| (vec![from, to], c));
        let round = ground.least_seen(
            from,
            to,
            &reach,
            most,
            straight.as_ref().map_or(1e7, |s| s.1 * 0.97),
        );
        let way = round
            .or(straight)
            .map(|(line, spent)| {
                let len: f64 = line.windows(2).map(|w| w[0].dist(w[1])).sum();
                // (Its points between: where it leaves and arrives are the cut's own.)
                let inner = &line[1..line.len() - 1];
                json!({
                    "way_mm": inner.iter().map(|q| json!([mm(q.x / 10.0), mm(q.y / 10.0)])).collect::<Vec<_>>(),
                    "length_mm": mm(len / 10.0),
                    "needs_blend_mm": mm(spent / 10.0),
                    "straight": inner.is_empty(),
                })
            });
        (Value::Array(crosses), way)
    }

    /// `get_cuts`: where the thread is cut, and between what.
    fn cuts_json(&self) -> Value {
        let d = &self.design;
        let at = |q: P| json!([mm(q.x / 10.0), mm(q.y / 10.0)]);
        let name = |i: usize| self.names.get(i).cloned().unwrap_or_default();
        let mut cuts = Vec::new();
        for (b, block) in d.blocks.iter().enumerate() {
            for (r, run) in block.runs.iter().enumerate() {
                if !run.trim_before || r == 0 {
                    continue;
                }
                let (Some(&from), Some(&to)) = (block.runs[r - 1].pts.last(), run.pts.first())
                else {
                    continue;
                };
                // (Whose stitches end the run before, and begin this one.)
                let spans = |r: usize| d.stitch_shapes.get(b).and_then(|x| x.get(r));
                let after = spans(r - 1).and_then(|s| s.last()).map(|s| s.1);
                let before = spans(r).and_then(|s| s.first()).map(|s| s.1);
                let mut cut = json!({
                    "after": after.map(name),
                    "begins_on": before.map(name),
                    "same_object": after.is_some() && after == before,
                    "thread": hex(block.color),
                    "from_mm": at(from),
                    "to_mm": at(to),
                    "gap_mm": mm(from.dist(to) / 10.0),
                });
                // What the thread would lie on, sewn across: straight, and
                // by the way that shows least (for gaps worth the look).
                if let Some(target) = before.filter(|_| from.dist(to) <= 400.0) {
                    let (crosses, way) =
                        self.cut_ground(target, block.color, from, to, after != before);
                    cut["straight_crosses"] = crosses;
                    if let Some(way) = way {
                        cut["least_seen_way"] = way;
                    }
                }
                cuts.push(cut);
            }
        }
        json!({
            "cuts": cuts,
            "routed_by_hand": d.routes.iter().map(|(i, r)| json!({
                "to": name(*i),
                "way_mm": r.iter().map(|&q| at(q)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "blended_gaps": d.blends.iter().map(|w| json!({
                "way_mm": w.iter().map(|&q| at(q)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "note": "A cut between two objects is saved by giving the one it begins on a route_mm; one inside an object (same_object) is the document's connect_mm, blend_mm or over_sewn to save. straight_crosses is what the thread would lie on sewn straight across (shows: 0 can't be seen, 1 as plain as it gets). least_seen_way is the way round that shows least: pass its way_mm as route_mm to take it, or set blend_mm to at least its needs_blend_mm for the planner to take it (and others like it) by itself. No least_seen_way: every way crosses something the thread stands out on, and the cut is better kept.",
        })
    }

    fn design_json(&self) -> Value {
        // The prompt each object was made for, once each: an object has
        // its place in `prompts`, not the words over again.
        let mut prompts: Vec<Value> = Vec::new();
        let objects: Vec<Value> = (0..self.doc.shapes.len())
            .map(|i| {
                let mut o = self.object_json(i);
                if let Some(p) = o.get("prompt").filter(|p| p.is_string()).cloned() {
                    let k = prompts.iter().position(|x| *x == p).unwrap_or_else(|| {
                        prompts.push(p);
                        prompts.len() - 1
                    });
                    o["prompt"] = json!(k);
                }
                o
            })
            .collect();
        let b = self.design.bounds();
        let total = self.design.stitch_count();
        let (hw, hh) = self.hoop_dims();
        let fabric = self.doc.settings.fabric();
        json!({
            "coordinates": "millimetres; origin = hoop center; +x right, +y down",
            "document": {
                "name": self.doc.name,
                "file": self.doc.path.as_ref().map(|p| p.display().to_string()),
                "unsaved_changes": self.modified,
                "selected": self.selected_names(),
                "fabric": fabric.id,
                "fabric_name": fabric.name,
                "hoop": { "id": zigzag::document::format_hoop((hw, hh)), "name": self.hoop_label(), "width_mm": hw, "height_mm": hh },
                "machine": self.doc.settings.machine.as_ref().map(|id| json!({
                    "id": id,
                    "name": self.machine_label(),
                    "format": self.machine().and_then(|p| p.format),
                })),
                "min_stitch_mm": self.doc.settings.min_stitch_mm,
                "narrow_satin_mm": self.doc.settings.narrow_satin_mm,
                "lock": zigzag::machine::profile::lock_json(self.doc.settings.lock),
                "remove_overlaps": self.doc.settings.remove_overlaps,
                "connect_mm": self.doc.settings.connect_mm,
                "blend_mm": self.doc.settings.blend_mm,
                "over_sewn": self.doc.settings.over_sewn,
                "hat_order": self.doc.settings.hat_order,
                "preview_color": hex(self.doc.settings.cloth),
            },
            "stats": {
                "width_mm": mm(b.width() / 10.0),
                "height_mm": mm(b.height() / 10.0),
                "stitches": total,
                "colors": self.design.blocks.len(),
                "trims": self.design.trim_count(),
                "blended_gaps": self.design.blends.len(),
                "minutes": (total as f64 / SPM).ceil(),
            },
            "objects": objects,
            "prompts": prompts,
            "design_problems": self.issues.iter().filter(|x| x.shape.is_none()).map(|x| issue_json(x, &self.names)).collect::<Vec<_>>(),
        })
    }

    /// `run_tool`, with a panic inside it answered as a tool error rather
    /// than taking the app (or the headless server) down with it — the
    /// same guard the stitch planner has. What the tool changed before
    /// it panicked is put back, like an atomic batch's rollback, and
    /// `batching` goes back to what it was so a panicking batch can't
    /// leave it set.
    pub fn run_tool_caught(&mut self, name: &str, args: &Value) -> ToolResult {
        let was_batching = self.batching;
        let (undo0, doc0, redo0, sel0, modified0) = (
            self.undo.len(),
            self.doc.clone(),
            self.redo.clone(),
            self.selected,
            self.modified,
        );
        // A tool that switched tabs swapped the live document out from
        // under the snapshot: putting it back would give two tabs the
        // same file, so only `batching` is restored then.
        let tabs0 = (self.tabs.active, self.tab_count());
        zigzag::design::caught(|| self.run_tool(name, args)).unwrap_or_else(|e| {
            if (self.tabs.active, self.tab_count()) == tabs0 {
                self.doc = doc0;
                self.undo.truncate(undo0);
                self.redo = redo0;
                self.stable = self.doc.clone();
                self.selected = sel0;
                self.modified = modified0;
                self.dirty = true;
                self.replan();
            }
            self.batching = was_batching;
            ToolResult::error(format!(
                "`{name}` panicked: {}",
                crate::planner::panic_text(&e)
            ))
        })
    }

    /// Run one MCP tool against the live document.
    pub fn run_tool(&mut self, name: &str, args: &Value) -> ToolResult {
        if let Err(e) = check_args(name, args) {
            return ToolResult::error(e);
        }
        // Inside a batch, only a step that reads the stitches waits for them.
        let reads = reads_stitches(name);
        if reads {
            self.plan_now();
        }
        if (reads || !self.batching) && self.dirty_needs_plan() {
            self.replan();
        }
        // A short answer was asked for: what's wrong now, to tell what's new.
        let terse = args.get("terse").and_then(Value::as_bool) == Some(true);
        let before = (terse && name != "batch").then(|| self.problems());
        if name == "set_object" && args.get("objects").is_some() {
            return self.set_objects(args);
        }
        let result = match name {
            "batch" => return self.tool_batch(args),
            "trace_image" => self.tool_trace_image(args),
            "round_corners" => self.tool_round_corners(args),
            "break_into_columns" => self.tool_break_into_columns(args),
            "create_outlines" => self.tool_create_outlines(args),
            "measure_stitching" => self.tool_measure_stitching(args),
            "get_path" => self.tool_get_path(args),
            "edit_path" => self.tool_edit_path(args),
            "add_outline" => self.tool_add_outline(args),
            "simplify" => self.tool_simplify(args),
            "clean_up" => self.tool_clean_up(args),
            "simplify_design" => self.tool_simplify_design(args),
            "get_design" => Ok(ToolResult::json(&self.design_json())),
            "get_cuts" => Ok(ToolResult::json(&self.cuts_json())),
            "add_svg" => self.tool_add_svg(args),
            "set_object" => self.tool_set_object(args),
            "delete_object" => self.tool_delete_object(args),
            "group_objects" => self.tool_group(args),
            "ungroup_objects" => self.resolve_many(args.get("objects")).and_then(|idx| {
                let mut out = 0;
                self.structural(|d| {
                    out = d.ungroup(&idx)?;
                    if out == 0 {
                        anyhow::bail!("none of those is in a group");
                    }
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
                self.replan_soon();
                Ok(ToolResult::text(format!(
                    "{out} {} out of {} group.",
                    if out == 1 { "object is" } else { "objects are" },
                    if out == 1 { "its" } else { "their" }
                )))
            }),
            "move_object" => self.resolve(args.get("object")).and_then(|i| {
                let to =
                    args.get("to")
                        .and_then(Value::as_u64)
                        .ok_or("`to` must be a non-negative integer")? as usize;
                let to = to.min(self.doc.shapes.len() - 1);
                self.structural(|d| {
                    let s = d.shapes.remove(i);
                    d.shapes.insert(to, s);
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
                self.replan_soon();
                Ok(ToolResult::text(format!(
                    "Sewing order: {}",
                    self.names.join(" → ")
                )))
            }),
            "set_document" => self.tool_set_document(args),
            "transform_objects" => self.tool_transform(args),
            "add_text" => {
                let mut spec = zigzag::document::TextSpec::default();
                apply_text_args(&mut spec, args).and_then(|_| {
                    let mut x = args.get("x_mm").and_then(Value::as_f64).unwrap_or(0.0);
                    let mut y = args.get("y_mm").and_then(Value::as_f64).unwrap_or(0.0);
                    let on = self.path_for_text(args.get("on_path"))?;
                    let mut baseline = None;
                    if let Some((src, rel, c)) = on {
                        spec.path = Some(rel);
                        (x, y) = (c.x / 10.0, c.y / 10.0);
                        baseline = Some(self.doc.shapes[src].uid);
                    }
                    let i = self
                        .add_lettering(&spec, (x, y))
                        .map_err(|e| format!("{e:#}"))?;
                    self.unsew_baseline(baseline);
                    self.replan_soon();
                    self.fit_view();
                    Ok(ToolResult::json(&self.object_json(i)))
                })
            }
            "edit_text" => self.resolve(args.get("object")).and_then(|i| {
                let mut spec = self.doc.lettering(i).ok_or(format!(
                    "{} isn't lettering (only text added with add_text can be edited)",
                    self.names[i]
                ))?;
                apply_text_args(&mut spec, args)?;
                let mut baseline = None;
                if args.get("on_path").is_some_and(Value::is_null) {
                    spec.path = None;
                } else if let Some((src, rel, _)) = self.path_for_text(args.get("on_path"))? {
                    spec.path = Some(rel);
                    baseline = Some(self.doc.shapes[src].uid);
                }
                let uid = self.doc.shapes[i].uid;
                self.update_lettering(i, &spec)
                    .map_err(|e| format!("{e:#}"))?;
                self.unsew_baseline(baseline);
                self.replan_soon();
                let i = self
                    .doc
                    .shapes
                    .iter()
                    .position(|s| s.uid == uid)
                    .unwrap_or(i);
                Ok(ToolResult::json(&self.object_json(i)))
            }),
            "open_file" => match args.get("path").and_then(Value::as_str).map(PathBuf::from) {
                Some(p) if p.is_absolute() => {
                    let doc = Document::open(&p).map_err(|e| format!("{e:#}"));
                    doc.map(|d| {
                        self.open_document(d);
                        self.replan_soon();
                        ToolResult::json(&self.design_json())
                    })
                }
                _ => Err("path must be absolute".into()),
            },
            "list_documents" => Ok(ToolResult::json(&self.documents_json())),
            "list_machines" => Ok(ToolResult::json(&self.machines_json())),
            "switch_document" => {
                let infos = self.tab_infos();
                let i = match (
                    args.get("index").and_then(Value::as_u64),
                    args.get("name").and_then(Value::as_str),
                ) {
                    (Some(i), _) => Some(i as usize).filter(|&i| i < infos.len()),
                    (None, Some(n)) => infos.iter().position(|t| t.name == n),
                    _ => return ToolResult::error("give index or name"),
                };
                match i {
                    Some(i) => {
                        self.switch_tab(i);
                        self.replan_soon();
                        Ok(ToolResult::json(&self.documents_json()))
                    }
                    None => Err(format!(
                        "no such document; open: {}",
                        infos
                            .iter()
                            .map(|t| t.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                }
            }
            "new_document" => {
                self.new_tab();
                self.replan_soon();
                Ok(ToolResult::json(&self.documents_json()))
            }
            "group_colors" => {
                self.group_colors();
                Ok(ToolResult::text(format!(
                    "{} Sewing order: {}",
                    self.status,
                    self.names.join(" → ")
                )))
            }
            "list_fonts" => {
                let q = args
                    .get("search")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                let fonts: Vec<String> = zigzag::document::font_families()
                    .into_iter()
                    // (Names that begin with a dot are the system's own
                    // faces for its interface, not ones to set type in.)
                    .filter(|f| !f.starts_with('.') && f.to_lowercase().contains(&q))
                    .collect();
                Ok(ToolResult::json(
                    &json!({ "default": zigzag::document::default_font(), "fonts": fonts }),
                ))
            }
            "duplicate_objects" => self.tool_duplicate(args),
            "render_preview" => self.tool_render(args),
            "get_annotations" => self.tool_get_annotations(),
            "resolve_note" => self.tool_resolve_note(args),
            "start_variants" => self.tool_start_variants(),
            "save_variant" => self.tool_save_variant(args),
            "show_comparison" => self.tool_show_comparison(args),
            "review_design" => self.tool_review_design(),
            "offer_choices" => self.tool_offer_choices(args),
            "check_design" => {
                let v: Vec<Value> = self
                    .issues
                    .iter()
                    .map(|x| issue_json(x, &self.names))
                    .collect();
                Ok(if v.is_empty() {
                    ToolResult::text("No problems found.")
                } else {
                    ToolResult::json(&json!(v))
                })
            }
            "get_svg" => self
                .doc
                .to_svg()
                .map(ToolResult::text)
                .map_err(|e| format!("{e:#}")),
            "select" => match args.get("object") {
                None | Some(Value::Null) => {
                    self.set_selection(&[]);
                    Ok(ToolResult::text("Selection cleared."))
                }
                v => self.resolve(v).map(|i| {
                    self.set_selection(&[i]);
                    ToolResult::text(format!("Selected {}.", self.names[i]))
                }),
            },
            "redo" => {
                if self.apply_history(crate::app::History::Redo) {
                    self.replan_soon();
                    Ok(ToolResult::text("Redone."))
                } else {
                    Ok(ToolResult::text("Nothing to redo."))
                }
            }
            "rename_object" => self.resolve(args.get("object")).and_then(|i| {
                let name = args
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("`name` must be a string")?;
                let uid = self.doc.shapes[i].uid;
                self.structural(|d| d.rename(i, name))
                    .map_err(|e| format!("{e:#}"))?;
                self.replan_soon();
                let i = self
                    .doc
                    .shapes
                    .iter()
                    .position(|s| s.uid == uid)
                    .unwrap_or(i);
                Ok(ToolResult::json(&self.object_json(i)))
            }),
            "align_objects" => self.tool_align(args),
            "combine_shapes" => self.tool_combine(args),
            "undo" => {
                if self.apply_history(crate::app::History::Undo) {
                    // (The names too, for the next call to go by.)
                    self.replan_soon();
                    Ok(ToolResult::text("Undone."))
                } else {
                    Ok(ToolResult::text("Nothing to undo."))
                }
            }
            "save" => {
                let path = args.get("path").and_then(Value::as_str).map(PathBuf::from);
                if let Some(p) = &path
                    && !p.is_absolute()
                {
                    return ToolResult::error("path must be absolute");
                }
                if let Some(p) = &path
                    && !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg"))
                {
                    return ToolResult::error(
                        "save writes an SVG: the path must end in .svg (for a machine file, use export)",
                    );
                }
                if path.is_none() && self.doc.path.is_none() {
                    return ToolResult::error(
                        "this document has never been saved; pass an absolute path ending in .svg",
                    );
                }
                if self.save(path.as_deref()) {
                    Ok(ToolResult::text(self.status.clone()))
                } else {
                    Err(self.status.clone())
                }
            }
            "export" => {
                // No format: the one the path ends in, or else the one the
                // document's machine reads.
                let path = args.get("path").and_then(Value::as_str).map(PathBuf::from);
                let named = path
                    .as_ref()
                    .and_then(|p| p.extension())
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .and_then(|e| {
                        zigzag::machine::profile::machine_format(&e)
                            .or((e == "html").then_some("html"))
                    });
                let fmt = match args.get("format").and_then(Value::as_str) {
                    Some(f) => Some(f),
                    None => named.or(self.machine().and_then(|p| p.format)),
                };
                let same = |f: &str| {
                    zigzag::machine::profile::machine_format(f).or((f == "html").then_some("html"))
                };
                match path {
                    // (A machine file under another's name, or over a file
                    // that isn't one, is a mistake either way.)
                    _ if args.get("format").is_some()
                        && fmt.is_some_and(|f| named.is_none() || named != same(f)) =>
                    {
                        Err(format!(
                            "the path must end in .{}, the format asked for",
                            fmt.unwrap_or("")
                        ))
                    }
                    _ if fmt.is_none() => Err(
                        "format is required: the path doesn't end in one (.pes, .dst, …) and the document has no machine that says which file it reads"
                            .into(),
                    ),
                    Some(p) if p.is_absolute() => self
                        .export_to(fmt.unwrap_or(""), &p)
                        .and_then(|_| {
                            let mut said = format!(
                                "Wrote {} ({} stitches).",
                                p.display(),
                                self.design.stitch_count()
                            );
                            // The map of its commands to the objects.
                            if args.get("objects").and_then(Value::as_bool) == Some(true) {
                                let file = p.file_name().unwrap_or_default().to_string_lossy();
                                let map = self
                                    .doc
                                    .object_map(&self.design, fmt.unwrap_or(""), &file)
                                    .ok_or_else(|| {
                                        anyhow::anyhow!(
                                            "the file was written, but only machine files other than pen have an object map"
                                        )
                                    })?;
                                let to = zigzag::document::Document::object_map_path(&p);
                                std::fs::write(&to, serde_json::to_string_pretty(&map)?)?;
                                said.push_str(&format!(" Wrote {}.", to.display()));
                            }
                            Ok(ToolResult::text(said))
                        })
                        .map_err(|e| format!("{e:#}")),
                    _ => Err("path must be absolute".into()),
                }
            }
            "send_to_skitch" => self.tool_send_to_skitch(),
            "machine_status" => Ok(ToolResult::json(&self.machine_json())),
            // Lets tests prove a panic in a tool is an error, not a crash —
            // and that what it changed first is put back.
            #[cfg(test)]
            "__panic__" => {
                if args.get("mutate").and_then(Value::as_bool) == Some(true) {
                    self.modified = true;
                    self.selected = Some(usize::MAX);
                    self.undo.push(self.doc.clone());
                    self.redo.push(self.doc.clone());
                    self.doc.name = "panicked".into();
                }
                if args.get("switch_tab").and_then(Value::as_bool) == Some(true) {
                    self.new_tab();
                }
                panic!("test-only panic");
            }
            other => Err(format!("unknown tool '{other}'")),
        };
        if !self.batching && self.dirty_needs_plan() {
            self.replan();
        } else if self.batching {
            // Later steps name objects as they are now (names are cheap).
            self.names = zigzag::svg::display_names(&self.doc.shapes);
        }
        let result = result.unwrap_or_else(ToolResult::error);
        match before {
            Some(before) if !result.is_error => self.terse_result(result, &before),
            _ => result,
        }
    }

    /// The design's problems, as (object, kind of problem): which object
    /// is which number can change, and so can a measurement in the message;
    /// its name and what's wrong with it don't.
    pub(crate) fn problems(&self) -> Vec<(Option<String>, String)> {
        self.issues
            .iter()
            .map(|i| {
                (
                    i.shape.and_then(|s| self.names.get(s).cloned()),
                    problem_kind(&i.message),
                )
            })
            .collect()
    }

    /// The problems there are now that weren't in `before`.
    pub(crate) fn new_problems(&self, before: &[(Option<String>, String)]) -> Vec<Value> {
        self.issues
            .iter()
            .filter(|i| {
                let object = i.shape.and_then(|s| self.names.get(s).cloned());
                let kind = problem_kind(&i.message);
                !before.iter().any(|(o, m)| *o == object && *m == kind)
            })
            .map(|i| issue_json(i, &self.names))
            .collect()
    }

    /// A tool's result in a line: that it worked, what it added, and what
    /// it broke. (A result that's already a sentence stays as it is; inside
    /// a batch the stitches aren't planned yet, so the batch says what's new.)
    fn terse_result(&self, full: ToolResult, before: &[(Option<String>, String)]) -> ToolResult {
        let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(&text_of(&full)) else {
            return full;
        };
        let mut out = json!({ "ok": true });
        let added = added_names(&value);
        if !added.is_empty() {
            out["added"] = Value::Array(added);
        }
        // What it costs to sew, when a tool says (trace_image): kept.
        for k in ["new_colors", "trims"] {
            if let Some(v) = value.get(k) {
                out[k] = v.clone();
            }
        }
        if !self.batching {
            out["new_problems"] = Value::Array(self.new_problems(before));
        }
        let mut r = ToolResult::json(&out);
        r.content.extend(
            full.content
                .into_iter()
                .filter(|c| matches!(c, Content::Image(_))),
        );
        r
    }

    /// Several tool calls in order; atomic: all or nothing, one undo step.
    fn tool_batch(&mut self, args: &Value) -> ToolResult {
        let terse = args.get("terse").and_then(Value::as_bool) == Some(true);
        let before = terse.then(|| self.problems());
        let was = std::mem::replace(&mut self.batching, true);
        let mut r = self.tool_batch_steps(args);
        self.batching = was;
        if !was && self.undo.len() > crate::app::UNDO_STEPS {
            let extra = self.undo.len() - crate::app::UNDO_STEPS;
            self.undo.drain(..extra);
        }
        if !was && (self.dirty || self.dirty_needs_plan()) {
            self.replan();
        }
        // The short answer: what ran, what it added, what the steps that
        // only read found, and what's wrong now that wasn't.
        if let Some(before) = before.filter(|_| !r.is_error) {
            let full: Value = serde_json::from_str(&text_of(&r)).unwrap_or(Value::Null);
            let steps = full["results"].as_array().cloned().unwrap_or_default();
            let mut out = json!({ "ok": true, "steps": steps.len() });
            let added: Vec<Value> = steps
                .iter()
                .flat_map(|s| added_names(&s["result"]))
                .collect();
            if !added.is_empty() {
                out["added"] = Value::Array(added);
            }
            let read: Vec<Value> = steps
                .iter()
                .filter(|s| s["tool"].as_str().is_some_and(only_reads))
                .cloned()
                .collect();
            if !read.is_empty() {
                out["results"] = Value::Array(read);
            }
            out["new_problems"] = Value::Array(self.new_problems(&before));
            let images: Vec<Content> = std::mem::take(&mut r.content)
                .into_iter()
                .filter(|c| matches!(c, Content::Image(_)))
                .collect();
            r = ToolResult::json(&out);
            r.content.extend(images);
        }
        r
    }

    fn tool_batch_steps(&mut self, args: &Value) -> ToolResult {
        let Some(steps) = args.get("steps").and_then(Value::as_array) else {
            return ToolResult::error("`steps` must be a list of {tool, arguments}");
        };
        if steps.len() > 64 {
            return ToolResult::error("at most 64 steps in a batch");
        }
        let atomic = args.get("atomic").and_then(Value::as_bool).unwrap_or(true);
        let (undo0, doc0, redo0, sel0, modified0) = (
            self.undo.len(),
            self.doc.clone(),
            std::mem::take(&mut self.redo),
            self.selected,
            self.modified,
        );
        // Steps that did something a rollback can't take back.
        let mut outside: Vec<&str> = Vec::new();
        let mut results: Vec<Value> = Vec::new();
        let mut report = Vec::new();
        let mut images = Vec::new();
        // The objects' names before each step, to say which step took away
        // one a later step names.
        let mut names_before: Vec<Vec<String>> = Vec::new();
        for (k, step) in steps.iter().enumerate() {
            names_before.push(self.names.clone());
            let tool = step.get("tool").and_then(Value::as_str).unwrap_or("");
            // (A step with `args` or `params` for `arguments` would run with
            // none, and fail for want of one of them: say what a step is.)
            let odd = step
                .as_object()
                .and_then(|o| {
                    o.keys()
                        .find(|k| !matches!(k.as_str(), "tool" | "arguments"))
                })
                .map(|k| {
                    format!(
                        "a step doesn't take `{k}`: it is {{\"tool\": name, \"arguments\": {{…}}}}."
                    )
                })
                .or((tool.is_empty()).then(|| {
                    "a step needs a `tool`: it is {\"tool\": name, \"arguments\": {…}}.".to_string()
                }));
            let failed = |msg: String| {
                if atomic {
                    format!(
                        "Step {k} ({tool}) failed: {msg} Nothing was changed: the batch is all or nothing."
                    )
                } else {
                    format!("Step {k} ({tool}) failed: {msg} Steps before it were kept.")
                }
            };
            // Opening a file clears the history, so it couldn't be rolled
            // back; nor could a step in another document (the batch's undo
            // step and its rollback belong to the one it began in).
            let outcome = if let Some(odd) = odd {
                Err(odd)
            } else if tool == "batch"
                || (atomic
                    && matches!(
                        tool,
                        "undo" | "redo" | "open_file" | "switch_document" | "new_document"
                    ))
            {
                Err(format!("`{tool}` can't run inside an atomic batch."))
            } else {
                resolve_refs(step.get("arguments").unwrap_or(&json!({})), &results)
                    .map(|a| self.run_tool_caught(tool, &a))
            };
            let r = outcome.unwrap_or_else(ToolResult::error);
            if r.is_error {
                let why = text_of(&r);
                let mut msg = failed(why.clone());
                if let Some(note) = gone_in_batch(&why, &names_before, steps) {
                    msg.push(' ');
                    msg.push_str(&note);
                }
                if atomic {
                    msg.push_str(&format!(
                        " Fix step {k} (or leave it out) and send the batch again."
                    ));
                }
                if atomic {
                    self.doc = doc0;
                    self.undo.truncate(undo0);
                    self.redo = redo0;
                    self.stable = self.doc.clone();
                    self.selected = sel0;
                    // (A save that went through wrote the design the
                    // rollback has just taken back: the file isn't this.)
                    self.modified = modified0 || outside.contains(&"save");
                    if !outside.is_empty() {
                        msg.push_str(&format!(
                            " (The design is as it was, but these steps had already run and aren't undone: {}.)",
                            outside.join(", ")
                        ));
                    }
                    self.dirty = true;
                    self.replan();
                } else if self.undo.len() == undo0 {
                    self.redo = redo0;
                }
                let mut r = ToolResult::json(&json!({
                    "error": msg,
                    "failed_step": { "step": k, "tool": tool, "arguments": step.get("arguments").cloned().unwrap_or(json!({})) },
                    "results": report,
                }));
                r.is_error = true;
                return r;
            }
            let text = text_of(&r);
            let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
            images.extend(
                r.content
                    .into_iter()
                    .filter(|c| matches!(c, Content::Image(_))),
            );
            report.push(json!({ "step": k, "tool": tool, "result": value.clone() }));
            results.push(value);
            if matches!(
                tool,
                "save"
                    | "export"
                    | "send_to_skitch"
                    | "resolve_note"
                    | "save_variant"
                    | "start_variants"
                    | "offer_choices"
            ) && let Some(name) = steps[k].get("tool").and_then(Value::as_str)
            {
                outside.push(name);
            }
        }
        // Nothing changed (steps that only read): redo is as it was.
        if self.undo.len() == undo0 {
            self.redo = redo0;
        }
        // One undo step for the lot.
        if atomic && self.undo.len() > undo0 + 1 {
            self.undo.truncate(undo0 + 1);
        }
        if let Some(label) = args.get("label").and_then(Value::as_str) {
            self.status = label.to_string();
        }
        let mut out = ToolResult::json(&json!({ "results": report }));
        out.content.extend(images);
        out
    }

    fn tool_trace_image(&mut self, args: &Value) -> Result<ToolResult, String> {
        let path = match args.get("path").and_then(Value::as_str).map(PathBuf::from) {
            Some(p) if p.is_absolute() => p,
            _ => return Err("path must be absolute".into()),
        };
        let bytes =
            std::fs::read(&path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
        let num = |k: &str| {
            args.get(k)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
        };
        let (hw, hh) = self.hoop_dims();
        let image = zigzag::trace::Image::decode(&bytes).map_err(|e| format!("{e:#}"))?;
        let (w, h) = image.size();
        let aspect = h as f64 / w.max(1) as f64;
        let mut o = zigzag::trace::TraceOptions {
            width_mm: num("width_mm").unwrap_or_else(|| (hw * 0.8).min(hh * 0.8 / aspect)),
            ..Default::default()
        };
        if let Some(n) = args.get("colors").and_then(Value::as_u64) {
            if !(2..=10).contains(&n) {
                return Err("colors must be 2–10 (or leave it out to find them)".into());
            }
            o.colors = n as u32;
        }
        if let Some(b) = args.get("remove_background").and_then(Value::as_bool) {
            o.remove_background = b;
        }
        if let Some(b) = args.get("lines").and_then(Value::as_bool) {
            o.lines = b;
        }
        if let Some(v) = num("line_width_mm") {
            o.line_width_mm = v.clamp(0.3, 5.0);
        }
        if let Some(v) = num("detail") {
            o.detail = v.clamp(0.0, 100.0);
        }
        if let Some(v) = num("speckle_mm") {
            o.speckle_mm = v.clamp(0.0, 10.0);
        }
        o.threads = match args.get("threads").and_then(Value::as_str) {
            None | Some("none") => Vec::new(),
            Some("brother") => crate::threads::chart(crate::threads::Brand::Brother)
                .into_iter()
                .map(|t| t.0)
                .collect(),
            Some("janome") => crate::threads::chart(crate::threads::Brand::Janome)
                .into_iter()
                .map(|t| t.0)
                .collect(),
            Some(other) => {
                return Err(format!(
                    "threads must be none, brother or janome, not {other}"
                ));
            }
        };
        o.palette = match args.get("palette") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::String(s)) if s == "design" => {
                self.doc.threads().into_iter().map(|t| t.0).collect()
            }
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| v.as_str().and_then(parse_hex))
                .collect::<Option<Vec<_>>>()
                .ok_or("palette colors must look like #rrggbb")?,
            Some(_) => return Err("palette must be \"design\" or a list of #rrggbb".into()),
        };
        if let Some(v) = num("palette_reach") {
            o.palette_reach = v.clamp(0.0, 100.0);
        }
        let palette = o.palette.clone();
        let threads_before: Vec<[u8; 3]> = self.doc.threads().into_iter().map(|t| t.0).collect();
        // (The plan is up to date when a call begins.)
        let trims_before = self.design.trim_count();
        let t = zigzag::trace::trace(&image, &o).map_err(|e| format!("{e:#}"))?;
        let replace = args
            .get("replace")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let place = Placement {
            center_mm: Some((num("x_mm").unwrap_or(0.0), num("y_mm").unwrap_or(0.0))),
            width_mm: None,
            height_mm: None,
        };
        let mut added = Vec::new();
        self.structural(|d| {
            if replace {
                // A new document for the same job: its fabric, hoop,
                // machine and the rest stay.
                let settings = std::mem::take(&mut d.settings);
                *d = Document::empty();
                d.settings = settings;
            }
            added = d.add_svg(&t.svg, place)?;
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))?;
        if replace {
            self.selected = None;
        }
        self.replan_soon();
        self.fit_view();
        // What it costs to sew: the threads it brought that weren't there,
        // and the trims (each separate piece of a color is a jump).
        // (Planned just above, unless a batch plans at its end: then now,
        // for the trims to be the trace's.)
        if self.batching {
            self.replan();
        }
        let new_colors: Vec<String> = t
            .colors
            .iter()
            .filter(|c| !threads_before.contains(c) && !palette.contains(c))
            .map(|&c| hex(c))
            .collect();
        let objs: Vec<Value> = added.iter().map(|&i| self.object_json(i)).collect();
        Ok(ToolResult::json(&json!({
            "traced_as": t.method,
            "colors": t.colors.iter().map(|&c| hex(c)).collect::<Vec<_>>(),
            "new_colors": new_colors,
            "trims": { "before": trims_before, "after": self.design.trim_count() },
            "added": objs,
            "total_objects": self.doc.shapes.len(),
        })))
    }

    fn tool_simplify(&mut self, args: &Value) -> Result<ToolResult, String> {
        use zigzag::pathtools;
        let idx = self.resolve_many(args.get("objects"))?;
        let level = match args.get("level") {
            None | Some(Value::Null) => {
                let mut b = zigzag::geom::Bounds::empty();
                for &i in &idx {
                    if let Ok(p) = self.doc.edit_path(i) {
                        let pb = zigzag::bezier::bounds(&p);
                        if !pb.is_empty() {
                            b.add(pb.min);
                            b.add(pb.max);
                        }
                    }
                }
                let size = if b.is_empty() {
                    0.0
                } else {
                    b.width().max(b.height())
                };
                pathtools::auto_simplify_level(size / pathtools::PT)
            }
            Some(v) => v
                .as_f64()
                .filter(|l| (0.0..=100.0).contains(l))
                .ok_or("level must be a number from 0 to 100")?,
        };
        let tol = pathtools::simplify_level_tolerance(level) * pathtools::PT;
        let mut removed = 0;
        let redo = self.structural_keeping_redo(|d| {
            removed = d.simplify_paths(&idx, tol)?;
            Ok(())
        })?;
        if removed == 0 {
            self.undo.pop();
            self.redo = redo;
            return Ok(ToolResult::text(format!(
                "Already as simple as level {level:.0} gets."
            )));
        }
        self.replan_soon();
        Ok(ToolResult::text(format!(
            "Simplified at level {level:.0}: {removed} fewer points."
        )))
    }

    fn tool_clean_up(&mut self, args: &Value) -> Result<ToolResult, String> {
        let flag = |k: &str| args.get(k).and_then(Value::as_bool).unwrap_or(true);
        let tiny = args.get("tiny_mm").and_then(Value::as_f64).unwrap_or(1.0);
        if !(tiny.is_finite() && tiny > 0.0) {
            return Err("tiny_mm must be a positive number".into());
        }
        let opt = zigzag::document::CleanUp {
            stray: flag("stray_points"),
            specks: flag("tiny_objects"),
            speck: tiny.min(100.0) * 10.0,
            points: flag("extra_points"),
        };
        let dry = args
            .get("dry_run")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if dry {
            let mut doc = self.doc.clone();
            let r = doc.clean_up(&opt, true).map_err(|e| format!("{e:#}"))?;
            return Ok(ToolResult::text(
                crate::path_tools::cleanup_summary(&r).replacen("Removed", "Would remove", 1),
            ));
        }
        let mut r = zigzag::document::CleanUpReport::default();
        let redo = self.structural_keeping_redo(|d| {
            r = d.clean_up(&opt, false)?;
            Ok(())
        })?;
        if r.total() == 0 {
            self.undo.pop();
            self.redo = redo;
        } else {
            self.replan_soon();
        }
        Ok(ToolResult::text(crate::path_tools::cleanup_summary(&r)))
    }

    fn tool_simplify_design(&mut self, args: &Value) -> Result<ToolResult, String> {
        use crate::simplify::Figures;
        use zigzag::document::SimplifyDesign;
        let threads = match args.get("threads") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .filter(|&n| n >= 1)
                    .ok_or("`threads` must be a whole number, 1 or more")? as usize,
            ),
        };
        let mm = |key: &str, most: f64| match args.get(key) {
            None | Some(Value::Null) => Ok(0.0),
            Some(v) => v
                .as_f64()
                .filter(|m| m.is_finite() && (0.0..=most).contains(m))
                .ok_or(format!("`{key}` must be a number from 0 to {most}")),
        };
        let flag = |key: &str, default: bool| match args.get(key) {
            None | Some(Value::Null) => Ok(default),
            Some(v) => v.as_bool().ok_or(format!("`{key}` must be true or false")),
        };
        let opt = SimplifyDesign {
            threads,
            min_size_mm: mm("min_size_mm", SimplifyDesign::MAX_SIZE_MM)?,
            connect_mm: mm("connect_mm", SimplifyDesign::MAX_CONNECT_MM)?,
            group: flag("group_colors", true)?,
            unite: match args.get("unite") {
                None | Some(Value::Null) => zigzag::document::Unite::Off,
                Some(v) => v
                    .as_str()
                    .and_then(zigzag::document::Unite::by_id)
                    .ok_or("`unite` must be \"off\", \"similar\" or \"all\"")?,
            },
        };
        let dry = flag("dry_run", false)?;
        if self.doc.shapes.is_empty() {
            return Err("there is nothing in the design to simplify".into());
        }
        if self.dirty_needs_plan() {
            self.replan();
        }
        let before = Figures::of(&self.design);
        let (report, after) = if dry {
            let mut doc = self.doc.clone();
            let r = doc.simplify_design(&opt).map_err(|e| format!("{e:#}"))?;
            (r, Figures::of(&doc.plan()))
        } else {
            let mut r = zigzag::document::SimplifyReport::default();
            let redo = self.structural_keeping_redo(|d| {
                r = d.simplify_design(&opt)?;
                Ok(())
            })?;
            if r.changed() {
                self.replan();
            } else {
                // Nothing to undo.
                self.undo.pop();
                self.redo = redo;
            }
            (r, Figures::of(&self.design))
        };
        let figures = |f: &Figures| {
            json!({
                "threads": f.threads,
                "thread_changes": f.changes,
                "trims": f.trims,
                "stitches": f.stitches,
                "minutes": f.minutes,
            })
        };
        let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
        Ok(ToolResult::json(&json!({
            "changed": report.changed() && !dry,
            "dry_run": dry,
            "before": figures(&before),
            "after": figures(&after),
            "merged_threads": report
                .merged
                .iter()
                .map(|(from, into)| json!({ "from": hex(*from), "into": hex(*into) }))
                .collect::<Vec<_>>(),
            "removed_objects": report.removed,
            "united_objects": report.united,
            "reordered": report.reordered,
            "connect_mm": report.connect_mm.1,
        })))
    }

    fn tool_add_outline(&mut self, args: &Value) -> Result<ToolResult, String> {
        let idx = self.resolve_many(args.get("objects"))?;
        let width = args
            .get("width_mm")
            .and_then(Value::as_f64)
            .unwrap_or(zigzag::params::defaults::OUTLINE_SATIN_WIDTH);
        if !(width.is_finite() && width > 0.0) {
            return Err("width_mm must be a positive number".into());
        }
        let color = match args.get("color") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_str()
                    .and_then(parse_hex)
                    .ok_or("color must look like #rrggbb")?,
            ),
        };
        let kind = match args.get("kind").and_then(Value::as_str) {
            None | Some("run") => Kind::Run,
            Some("satin") => Kind::Satin,
            Some(k) => return Err(format!("kind must be run or satin, not {k}")),
        };
        let mut new = Vec::new();
        self.structural(|d| {
            new = d.add_outline(&idx, kind, width, color)?;
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))?;
        self.replan_soon();
        self.set_selection(&new);
        Ok(ToolResult::json(
            &json!({ "added": new.iter().map(|&i| self.object_json(i)).collect::<Vec<_>>() }),
        ))
    }

    fn tool_break_into_columns(&mut self, args: &Value) -> Result<ToolResult, String> {
        let idx = self.resolve_many(args.get("objects"))?;
        let mut new = Vec::new();
        self.structural(|d| {
            new = d.break_into_columns(&idx)?;
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))?;
        self.replan_soon();
        self.set_selection(&new);
        Ok(ToolResult::json(
            &json!({ "made": new.iter().map(|&i| self.object_json(i)).collect::<Vec<_>>() }),
        ))
    }

    fn tool_measure_stitching(&mut self, args: &Value) -> Result<ToolResult, String> {
        use zigzag::design::measure;
        let satin = |s: &zigzag::svg::Shape| {
            s.params.kind == Some(Kind::Satin)
                && match &s.geometry {
                    Geometry::Fill { .. } => !s.params.is_border() && !s.outline,
                    Geometry::Stroke { .. } => measure::drawn_column(s).is_some(),
                }
        };
        let asked: Vec<usize> = match args.get("objects") {
            None | Some(Value::Null) => (0..self.doc.shapes.len())
                .filter(|&i| satin(&self.doc.shapes[i]))
                .collect(),
            some => self.resolve_many(some)?,
        };
        // What this can't measure (a border satin, a fill, a run) is said
        // and left out, not a failure of the rest.
        let (idx, skipped): (Vec<usize>, Vec<usize>) =
            asked.into_iter().partition(|&i| satin(&self.doc.shapes[i]));
        if idx.is_empty() {
            return Err(if skipped.is_empty() {
                "there's no satin to measure: a filled shape sewn in satin, or a two-rail satin"
                    .into()
            } else {
                "none of these is a satin column or a filled shape sewn in satin (a border satin, a fill or a run isn't measured this way: see review_design and check_design)".into()
            });
        }
        let within = match args.get("within") {
            None | Some(Value::Null) => Vec::new(),
            some => self.resolve_many(some)?,
        };
        if let Some(&i) = within
            .iter()
            .find(|&&i| !matches!(self.doc.shapes[i].geometry, Geometry::Fill { .. }))
        {
            return Err(format!(
                "`within`: object {i} isn't a filled shape, so it has no outline to hold stitching to"
            ));
        }
        let take = |idx: &[usize]| -> Vec<zigzag::svg::Shape> {
            idx.iter().map(|&i| self.doc.shapes[i].clone()).collect()
        };
        let (shapes, held) = (take(&idx), take(&within));
        let opt = self.doc.plan_options();
        let (outline, bare) = measure::counted(&shapes, &held);
        let l =
            zigzag::design::caught(|| measure::measured(&shapes, &held, &opt)).map_err(|_| {
                "those objects couldn't be measured (an internal error on their outlines)"
                    .to_string()
            })?;
        let mm2 = |cells: usize| mm(cells as f64 / 100.0);
        let measures = json!({
            "trims": l.trims,
            "trims_needed": l.owed,
            "travel_showing": l.exposed,
            "stitches_outside": outline.then_some(l.outside),
            "stitches_leaving_shape": l.astray,
            "bare_mm2": bare.then(|| mm2(l.bare)),
            "crowded_needle_points": l.crowded,
            "seams": l.seams,
            "piled_mm2": mm2(l.layers),
            "stitches_off_square": l.skewed,
            "stitches_off_edge": l.off_edge,
            "worst_lean_deg": l.lean,
            "pointed_ends": l.pointed,
            "filled_parts": l.fills,
        });
        // (What isn't a fault at nothing: the cuts it can't do without.)
        let amount = |key: &str, v: &Value| -> Option<f64> {
            let x = v.get(key)?.as_f64()?;
            Some(match key {
                "trims" => {
                    (x - v.get("trims_needed").and_then(Value::as_f64).unwrap_or(0.0)).max(0.0)
                }
                _ => x,
            })
        };
        let keys: Vec<&str> = measures
            .as_object()
            .map(|m| {
                m.keys()
                    .map(String::as_str)
                    .filter(|k| *k != "trims_needed")
                    .collect()
            })
            .unwrap_or_default();
        let faults: Vec<&str> = keys
            .iter()
            .copied()
            .filter(|k| amount(k, &measures).is_some_and(|x| x > 0.0))
            .collect();
        let mut out = json!({
            "objects": idx,
            "columns": l.columns,
            "measures": measures,
            "faults": faults,
        });
        if !skipped.is_empty() {
            out["not_measured"] = json!({
                "objects": skipped.iter().map(|&i| self.names.get(i).cloned().unwrap_or_default()).collect::<Vec<_>>(),
                "why": "not a satin column or a filled shape sewn in satin (a border satin, a fill or a run): review_design and check_design look over those",
            });
        }
        if !outline {
            out["note"] = json!(
                "No outline to hold these columns to: stitches_outside and bare_mm2 aren't measured. Pass the shape they came from (an unsewn copy) as `within`."
            );
        } else if !bare {
            out["note"] = json!(
                "Too big an area to look over for bare cloth: bare_mm2 isn't measured. Measure fewer objects at a time."
            );
        }
        if let Some(before) = args.get("before").filter(|b| b.is_object()) {
            let (mut better, mut worse) = (Vec::new(), Vec::new());
            let mut changes = serde_json::Map::new();
            for k in &keys {
                let (Some(was), Some(now)) = (amount(k, before), amount(k, &out["measures"]))
                else {
                    continue;
                };
                if (now - was).abs() > 1e-9 {
                    changes.insert(
                        (*k).into(),
                        json!({ "before": before[*k], "after": out["measures"][*k] }),
                    );
                    if now < was {
                        better.push(*k)
                    } else {
                        worse.push(*k)
                    }
                }
            }
            out["compared"] = json!({
                "verdict": match (better.is_empty(), worse.is_empty()) {
                    (true, true) => "same",
                    (false, true) => "better",
                    (true, false) => "worse",
                    (false, false) => "mixed",
                },
                "better": better,
                "worse": worse,
                "changes": changes,
            });
        }
        Ok(ToolResult::json(&out))
    }

    fn tool_create_outlines(&mut self, args: &Value) -> Result<ToolResult, String> {
        let idx = self.resolve_many(args.get("objects"))?;
        if let Some(&i) = idx.iter().find(|&&i| self.doc.lettering(i).is_none()) {
            return Err(format!(
                "object {i} isn't text: only text is made outlines (a shape already is its outline)"
            ));
        }
        let uids: Vec<usize> = idx.iter().map(|&i| self.doc.shapes[i].uid).collect();
        let mut new = Vec::new();
        self.structural(|d| {
            for uid in &uids {
                let Some(i) = d.shapes.iter().position(|s| s.uid == *uid) else {
                    continue;
                };
                let j = d.expand_lettering(i)?;
                new.push(d.shapes[j].uid);
            }
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))?;
        self.replan_soon();
        let made: Vec<usize> = (0..self.doc.shapes.len())
            .filter(|&i| new.contains(&self.doc.shapes[i].uid))
            .collect();
        self.set_selection(&made);
        Ok(ToolResult::json(
            &json!({ "made": made.iter().map(|&i| self.object_json(i)).collect::<Vec<_>>() }),
        ))
    }

    /// Object `i`'s outline as `get_path` gives it.
    fn path_json(&self, i: usize, path: &zigzag::bezier::BezPath) -> Value {
        let s = &self.doc.shapes[i];
        let at = |p: zigzag::geom::P| json!([mm(p.x / 10.0), mm(p.y / 10.0)]);
        // (Which subpaths are a two-rail satin's rails: as it says, or as
        // they're told apart when it doesn't.)
        let rails: Option<(usize, usize)> = match &s.geometry {
            Geometry::Stroke { lines, .. }
                if s.params.kind == Some(Kind::Satin) && !s.outline && lines.len() >= 2 =>
            {
                zigzag::stitch::satin::rails_of_subpaths(lines, s.params.satin_rails())
            }
            _ => None,
        };
        let subpaths: Vec<Value> = path
            .iter()
            .enumerate()
            .map(|(k, sp)| {
                let points: Vec<Value> = sp
                    .anchors
                    .iter()
                    .enumerate()
                    .map(|(n, a)| {
                        let mut point = serde_json::Map::new();
                        point.insert("point".into(), json!(n + 1));
                        point.insert("at_mm".into(), at(a.p));
                        if a.h_in != a.p {
                            point.insert("handle_in_mm".into(), at(a.h_in));
                        }
                        if a.h_out != a.p {
                            point.insert("handle_out_mm".into(), at(a.h_out));
                        }
                        Value::Object(point)
                    })
                    .collect();
                let mut sub = serde_json::Map::new();
                sub.insert("subpath".into(), json!(k + 1));
                sub.insert("closed".into(), json!(sp.closed));
                if let Some((a, b)) = rails
                    && path.len() == rails_count(s)
                {
                    sub.insert(
                        "role".into(),
                        json!(if k == a || k == b { "rail" } else { "rung" }),
                    );
                }
                sub.insert("points".into(), json!(points));
                Value::Object(sub)
            })
            .collect();
        json!({ "object": i, "name": self.names.get(i), "subpaths": subpaths })
    }

    fn tool_get_path(&mut self, args: &Value) -> Result<ToolResult, String> {
        let i = self.resolve(args.get("object"))?;
        if self.doc.lettering(i).is_some() {
            return Err(
                "that object is text: its letters are edited as text (edit_text), or made outlines first (create_outlines)"
                    .into(),
            );
        }
        let path = self.doc.edit_path(i).map_err(|e| format!("{e:#}"))?;
        Ok(ToolResult::json(&self.path_json(i, &path)))
    }

    fn tool_edit_path(&mut self, args: &Value) -> Result<ToolResult, String> {
        use zigzag::bezier::{Anchor, SubPath};
        use zigzag::geom::P;
        let i = self.resolve(args.get("object"))?;
        if self.doc.lettering(i).is_some() {
            return Err(
                "that object is text: make it outlines first (create_outlines), then edit its points"
                    .into(),
            );
        }
        let edits = args
            .get("edits")
            .and_then(Value::as_array)
            .filter(|e| !e.is_empty())
            .ok_or("`edits` must be a list of one or more edits")?;
        let mut path = self.doc.edit_path(i).map_err(|e| format!("{e:#}"))?;
        // (A satin's marked rails, kept right as subpaths come and go.)
        let mut rails = self.doc.shapes[i].params.rails;
        let point = |v: Option<&Value>, what: &str| -> Result<P, String> {
            let xy = v
                .and_then(Value::as_array)
                .filter(|a| a.len() == 2)
                .and_then(|a| Some((a[0].as_f64()?, a[1].as_f64()?)))
                .filter(|(x, y)| x.is_finite() && y.is_finite())
                .ok_or(format!("`{what}` must be [x, y] in mm"))?;
            Ok(P::new(xy.0 * 10.0, xy.1 * 10.0))
        };
        for (n, e) in edits.iter().enumerate() {
            let said = |why: String| format!("edit {}: {why}", n + 1);
            let op = e.get("op").and_then(Value::as_str).ok_or_else(|| {
                said("`op` must be move, insert, delete, add_subpath or delete_subpath".into())
            })?;
            // (Counted from 1; here from 0.)
            let number = |key: &str, most: usize, of: &str| -> Result<usize, String> {
                e.get(key)
                    .and_then(Value::as_u64)
                    .map(|k| k as usize)
                    .filter(|k| (1..=most).contains(k))
                    .map(|k| k - 1)
                    .ok_or_else(|| {
                        said(format!("`{key}` must be a number from 1 to {most} ({of})"))
                    })
            };
            if op == "add_subpath" {
                let pts = e
                    .get("points_mm")
                    .and_then(Value::as_array)
                    .filter(|a| a.len() >= 2)
                    .ok_or_else(|| said("`points_mm` must be two or more [x, y] points".into()))?;
                let anchors = pts
                    .iter()
                    .map(|v| point(Some(v), "points_mm").map(Anchor::corner))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(said)?;
                let closed = e.get("closed").and_then(Value::as_bool).unwrap_or(false);
                path.push(SubPath { anchors, closed });
                continue;
            }
            // (Left out, with only one subpath to mean: that one.)
            let s = match (e.get("subpath"), path.len()) {
                (None | Some(Value::Null), 1) => 0,
                (None | Some(Value::Null), n) => {
                    return Err(said(format!(
                        "this object has {n} subpaths: say which with `subpath` (from 1; get_path lists them)"
                    )));
                }
                _ => number("subpath", path.len(), "this object's subpaths")?,
            };
            match op {
                "delete_subpath" => {
                    if let Some((a, b)) = rails {
                        let k = s as u32 + 1;
                        if k == a || k == b {
                            return Err(said(format!(
                                "subpath {k} is one of the satin's rails: a column needs both (delete the object to remove the column)"
                            )));
                        }
                        let down = |r: u32| if r > k { r - 1 } else { r };
                        rails = Some((down(a), down(b)));
                    }
                    if path.len() == 1 {
                        return Err(said(
                            "that's the object's only subpath: delete the object instead (delete_object)".into(),
                        ));
                    }
                    path.remove(s);
                }
                "move" => {
                    let k = number("point", path[s].anchors.len(), "that subpath's points")?;
                    let a = &mut path[s].anchors[k];
                    let by = match (e.get("to_mm"), e.get("by_mm")) {
                        (Some(to), None) => point(Some(to), "to_mm").map_err(said)? - a.p,
                        (None, Some(by)) => point(Some(by), "by_mm").map_err(said)?,
                        _ => return Err(said("give `to_mm` or `by_mm` (one of them)".into())),
                    };
                    (a.p, a.h_in, a.h_out) = (a.p + by, a.h_in + by, a.h_out + by);
                }
                "delete" => {
                    let k = number("point", path[s].anchors.len(), "that subpath's points")?;
                    let least = if path[s].closed { 3 } else { 2 };
                    if path[s].anchors.len() <= least {
                        return Err(said(format!(
                            "subpath {} has only {} points: delete the subpath instead (delete_subpath)",
                            s + 1,
                            path[s].anchors.len()
                        )));
                    }
                    path[s].anchors.remove(k);
                }
                "insert" => {
                    let count = path[s].anchors.len();
                    let last = if path[s].closed { count } else { count - 1 };
                    let after = number(
                        "after",
                        last.max(1),
                        "the point the new one comes after; the last point only on a closed subpath",
                    )?;
                    let (_, k) = zigzag::bezier::split(&mut path, s, after, 0.5);
                    if let Some(at) = e.get("at_mm") {
                        // (Put there as a corner: the curve either side
                        // keeps its far handles.)
                        let at = point(Some(at), "at_mm").map_err(said)?;
                        path[s].anchors[k] = Anchor::corner(at);
                    }
                }
                other => {
                    return Err(said(format!(
                        "no op `{other}`: move, insert, delete, add_subpath or delete_subpath"
                    )));
                }
            }
        }
        let uid = self.doc.shapes[i].uid;
        self.structural(|d| {
            d.set_path(i, &path)?;
            if let Some(j) = d.shapes.iter().position(|s| s.uid == uid)
                && d.shapes[j].params.rails != rails
            {
                d.shapes[j].params.rails = rails;
            }
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))?;
        self.replan_soon();
        let i = self
            .doc
            .shapes
            .iter()
            .position(|s| s.uid == uid)
            .ok_or("the object has no outline left")?;
        let now = self.doc.edit_path(i).map_err(|e| format!("{e:#}"))?;
        let mut out = self.path_json(i, &now);
        out["settings"] = self.object_json(i)["settings"].clone();
        Ok(ToolResult::json(&out))
    }

    fn tool_round_corners(&mut self, args: &Value) -> Result<ToolResult, String> {
        let idx = self.resolve_many(args.get("objects"))?;
        let r = args.get("radius_mm").and_then(Value::as_f64).unwrap_or(1.5);
        if !(r.is_finite() && r > 0.0) {
            return Err("radius_mm must be a positive number".into());
        }
        let kind = match args.get("kind").and_then(Value::as_str) {
            None | Some("round") => zigzag::bezier::CornerKind::Round,
            Some("inverted") => zigzag::bezier::CornerKind::Inverted,
            Some("chamfer") => zigzag::bezier::CornerKind::Chamfer,
            Some(k) => return Err(format!("kind must be round, inverted or chamfer, not {k}")),
        };
        let mut n = 0;
        let redo = self.structural_keeping_redo(|d| {
            n = d.round_corners(&idx, r.min(50.0) * 10.0, kind, &[])?;
            Ok(())
        })?;
        if n == 0 {
            // Nothing changed: no undo step, and redo as it was.
            self.undo.pop();
            self.redo = redo;
            return Ok(ToolResult::text(
                "No sharp corners to round on those objects.",
            ));
        }
        self.replan_soon();
        Ok(ToolResult::text(format!("Rounded {n} corners.")))
    }

    /// A structural change that may turn out to change nothing: what could
    /// be redone before it is handed back, to put back if so (taken, not
    /// copied: it's whole documents).
    fn structural_keeping_redo(
        &mut self,
        f: impl FnOnce(&mut Document) -> anyhow::Result<()>,
    ) -> Result<Vec<Document>, String> {
        let redo = std::mem::take(&mut self.redo);
        match self.structural(f) {
            Ok(()) => Ok(redo),
            Err(e) => {
                self.redo = redo;
                Err(format!("{e:#}"))
            }
        }
    }

    /// Re-plan after an edit now, or (in a batch) once it's done.
    pub(crate) fn replan_soon(&mut self) {
        if self.batching {
            self.dirty = true;
            // Names are cheap, and later steps refer to objects by them.
            self.names = zigzag::svg::display_names(&self.doc.shapes);
        } else {
            self.replan();
        }
    }

    pub(crate) fn dirty_needs_plan(&self) -> bool {
        self.design.per_shape.len() != self.doc.shapes.len()
            || self.names.len() != self.doc.shapes.len()
    }

    fn tool_add_svg(&mut self, args: &Value) -> Result<ToolResult, String> {
        let svg = args
            .get("svg")
            .and_then(Value::as_str)
            .ok_or("`svg` is required")?;
        let num = |k: &str| args.get(k).and_then(Value::as_f64);
        let place = Placement {
            center_mm: Some((num("x_mm").unwrap_or(0.0), num("y_mm").unwrap_or(0.0))),
            width_mm: num("width_mm").filter(|v| *v > 0.0),
            height_mm: num("height_mm").filter(|v| *v > 0.0),
        };
        let replace = args
            .get("replace")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut added = Vec::new();
        self.structural(|d| {
            if replace {
                // A new document for the same job: its fabric, hoop,
                // machine and the rest stay.
                let settings = std::mem::take(&mut d.settings);
                *d = Document::empty();
                d.settings = settings;
            }
            added = d.add_svg(svg, place)?;
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))?;
        if replace {
            self.selected = None;
        }
        self.replan_soon();
        self.fit_view();
        let objs: Vec<Value> = added.iter().map(|&i| self.object_json(i)).collect();
        Ok(ToolResult::json(&json!({
            "added": objs,
            "total_objects": self.doc.shapes.len(),
            "design_problems": self.issues.iter().filter(|x| x.shape.is_none()).map(|x| issue_json(x, &self.names)).collect::<Vec<_>>(),
        })))
    }

    /// `on_path` for add_text / edit_text: the object, its outline relative
    /// to its center (the text's baseline), and that center (units).
    /// The path that text was just put on isn't sewn itself. Made part of
    /// the text's own undo step (the step is taken first, so undoing it
    /// brings the path back as it was, and text that couldn't be made
    /// leaves the path alone).
    fn unsew_baseline(&mut self, uid: Option<usize>) {
        if let Some(k) = uid.and_then(|u| self.doc.shapes.iter().position(|s| s.uid == u)) {
            self.doc.shapes[k].params.kind = Some(Kind::Skip);
            self.stable = self.doc.clone();
        }
    }

    fn path_for_text(
        &mut self,
        v: Option<&Value>,
    ) -> Result<Option<(usize, zigzag::bezier::BezPath, P)>, String> {
        let Some(v) = v.filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        let i = self.resolve(Some(v))?;
        let path = self
            .doc
            .edit_path(i)
            .map_err(|e| format!("{} can't be a baseline: {e:#}", self.names[i]))?;
        let b = zigzag::bezier::bounds(&path);
        let c = b.center();
        let rel = zigzag::bezier::transform(&path, zigzag::geom::translation(P::default() - c));
        Ok(Some((i, rel, c)))
    }

    fn tool_delete_object(&mut self, args: &Value) -> Result<ToolResult, String> {
        let list: Vec<Value> = match (args.get("objects"), args.get("object")) {
            (Some(Value::Array(a)), _) if !a.is_empty() => a.clone(),
            (Some(Value::Array(_)), _) => return Err("`objects` is empty".into()),
            (Some(v), _) | (None, Some(v)) => vec![v.clone()],
            (None, None) => return Err("`object` (or `objects`) is required".into()),
        };
        // An outline named after its shape went (with it, by an earlier
        // step): already done, not a failure that undoes a whole batch.
        let mut idx: Vec<usize> = Vec::new();
        let mut already: Vec<String> = Vec::new();
        for v in &list {
            match self.resolve(Some(v)) {
                Ok(i) => {
                    // (Not by part of a name: "sun", once deleted, would
                    // be "sunbeam" the second time it's asked.)
                    self.named_in_full(Some(v), i)?;
                    if !idx.contains(&i) {
                        idx.push(i);
                    }
                }
                Err(e) => match v.as_str() {
                    Some(n) if self.deleted_names.iter().any(|d| d == n) => {
                        already.push(n.to_string());
                    }
                    _ => return Err(e),
                },
            }
        }
        // (A shape's outline goes with it.)
        let gone = self.doc.with_outlines(&idx);
        let names: Vec<String> = gone.iter().map(|&i| self.names[i].clone()).collect();
        if !gone.is_empty() {
            self.structural(|d| d.delete_many(&gone))
                .map_err(|e| e.to_string())?;
            // (Remembered, a few hundred at most.)
            self.deleted_names.extend(names.iter().cloned());
            let over = self.deleted_names.len().saturating_sub(300);
            self.deleted_names.drain(..over);
            self.selected = self
                .selected
                .filter(|&u| self.doc.shapes.iter().any(|s| s.uid == u));
        }
        let mut said = Vec::new();
        if !names.is_empty() {
            said.push(format!("Deleted {}.", names.join(", ")));
        }
        if !already.is_empty() {
            said.push(format!(
                "{} already went with {}.",
                already.join(", "),
                if already.len() == 1 {
                    "its shape"
                } else {
                    "their shapes"
                }
            ));
        }
        Ok(ToolResult::text(said.join(" ")))
    }

    fn tool_set_object(&mut self, args: &Value) -> Result<ToolResult, String> {
        let i = self.resolve(args.get("object"))?;
        let shape = &self.doc.shapes[i];
        let mut p: Params = shape.params.clone();
        let mut color = shape.color;
        let paint = shape.paint;
        let allowed = allowed_kinds(shape);
        let num = |k: &str| -> Result<Option<Option<f64>>, String> {
            match args.get(k) {
                None => Ok(None),
                Some(Value::Null) => Ok(Some(None)),
                Some(v) => v
                    .as_f64()
                    .filter(|x| x.is_finite())
                    .map(|x| Some(Some(x)))
                    .ok_or(format!("`{k}` must be a number")),
            }
        };
        let flag = |k: &str| -> Result<Option<Option<bool>>, String> {
            match args.get(k) {
                None => Ok(None),
                Some(Value::Null) => Ok(Some(None)),
                Some(v) => v
                    .as_bool()
                    .map(|x| Some(Some(x)))
                    .ok_or(format!("`{k}` must be true, false or null")),
            }
        };
        if let Some(k) = args.get("kind") {
            let k = k.as_str().and_then(Kind::parse).ok_or(
                "`kind` must be fill, satin, run, contour, blanket, applique, meander, ripple, cross, tartan, manual or skip",
            )?;
            if !allowed.contains(&k) {
                return Err(format!(
                    "{} can't be {} (it's a stroke; use {})",
                    self.names[i],
                    k.name(),
                    allowed
                        .iter()
                        .map(|k| k.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            p.kind = Some(k);
        }
        if let Some(v) = num("angle")? {
            p.angle = v.map(crate::widgets::normalize_angle);
        }
        if let Some(v) = num("spacing_mm")? {
            // (The open stitches are spaced wider than rows of fill: as
            // far as their panels go.)
            let most = match p.kind {
                Some(Kind::Meander) => 8.0,
                Some(Kind::Blanket | Kind::Ripple) => 6.0,
                _ => 2.0,
            };
            p.spacing = v.map(|x| x.clamp(0.1, most));
        } else if args.get("kind").is_some() {
            // (A wide meander made a fill keeps rows a fill can have.)
            let most = match p.kind {
                Some(Kind::Meander) => 8.0,
                Some(Kind::Blanket | Kind::Ripple) => 6.0,
                _ => 2.0,
            };
            p.spacing = p.spacing.map(|x: f64| x.min(most));
        }
        if let Some(v) = num("length_mm")? {
            p.len = v.map(|x| x.clamp(0.5, 10.0));
        }
        if let Some(v) = num("pull_mm")? {
            p.pull = v.map(|x| x.clamp(0.0, 1.0));
        }
        if let Some(v) = num("width_mm")? {
            p.width = v.map(|x| x.clamp(0.3, 15.0));
        }
        if let Some(v) = flag("border")? {
            p.border = v;
        }
        if let Some(v) = flag("underlay")? {
            p.underlay = v;
        }
        if let Some(v) = flag("short_stitches")? {
            p.short_stitches = v;
        }
        if let Some(v) = num("cell_mm")? {
            p.cell = v.map(|x| x.clamp(1.0, 10.0));
        }
        if let Some(v) = num("split_mm")? {
            p.split = v.map(|x| x.max(0.0));
        }
        if let Some(v) = num("lean")? {
            p.lean = v
                .map(|x| x.clamp(-zigzag::params::MAX_LEAN, zigzag::params::MAX_LEAN))
                .filter(|x| *x != 0.0);
        }
        if let Some(v) = flag("even")? {
            p.even = v.filter(|&x| x != zigzag::params::defaults::SATIN_EVEN);
        }
        if let Some(v) = flag("cut")? {
            // (Cut is the default: only not cutting is recorded.)
            p.cut = v.filter(|&x| !x);
        }
        if let Some(v) = flag("mitred")? {
            p.mitred = v.filter(|&x| x);
        }
        match args.get("rails") {
            None => {}
            Some(Value::Null) => p.rails = None,
            Some(v) => {
                let pair: Option<(u32, u32)> =
                    v.as_array().filter(|a| a.len() == 2).and_then(|a| {
                        let n = |x: &Value| x.as_u64().and_then(|n| u32::try_from(n).ok());
                        Some((n(&a[0])?, n(&a[1])?))
                    });
                let subpaths = match &self.doc.shapes[i].geometry {
                    Geometry::Stroke { lines, .. } => lines.len() as u32,
                    Geometry::Fill { .. } => 0,
                };
                match pair {
                    Some((a, b))
                        if a != b && a >= 1 && b >= 1 && a <= subpaths && b <= subpaths =>
                    {
                        p.rails = Some((a, b))
                    }
                    _ if subpaths < 2 => {
                        return Err(
                            "`rails` picks two of an object's strokes as a satin's rails: this object has no strokes to pick from (it's a filled shape, or one line)"
                                .into(),
                        );
                    }
                    _ => {
                        return Err(format!(
                            "`rails` must be two different subpath numbers from 1 to {subpaths} (this object's subpaths), or null"
                        ));
                    }
                }
            }
        }
        if let Some(v) = num("repeats")? {
            p.repeats = v.map(|x| x.clamp(1.0, 9.0) as u32);
        }
        if let Some(v) = num("start")? {
            p.start = v.map(along);
        }
        if let Some(v) = num("end")? {
            p.end = v.map(along);
        }
        if let Some(v) = flag("trim_before")? {
            p.trim_before = v;
        }
        if let Some(v) = flag("trim")? {
            p.trim = v.filter(|&x| x);
        }
        match args.get("route_mm") {
            None => {}
            Some(Value::Null) => p.via = None,
            Some(Value::Array(a)) if a.len() <= 40 => {
                let pts: Option<Vec<(f64, f64)>> = a
                    .iter()
                    .map(|q| {
                        let q = q.as_array().filter(|q| q.len() == 2)?;
                        let n = |v: &Value| v.as_f64().filter(|x| x.abs() <= 1000.0);
                        // (To a tenth of a millimetre: it's kept in the name.)
                        Some((
                            (n(&q[0])? * 10.0).round() / 10.0,
                            (n(&q[1])? * 10.0).round() / 10.0,
                        ))
                    })
                    .collect();
                p.via = Some(pts.ok_or("route_mm must be points [[x, y], …] in mm")?);
            }
            Some(_) => {
                return Err("route_mm must be up to 40 points [[x, y], …], [] or null".into());
            }
        }
        if let Some(v) = flag("spiral")? {
            p.spiral = v.filter(|&x| x);
        }
        if let Some(v) = flag("stop")? {
            p.stop = v.filter(|&x| x);
        }
        if let Some(v) = flag("hidden")? {
            p.hide = v.filter(|&x| x);
        }
        if let Some(v) = flag("locked")? {
            p.lock = v.filter(|&x| x);
        }
        match args.get("satin_underlay") {
            None => {}
            Some(Value::String(s)) if s == "auto" => p.satin_underlay = None,
            Some(Value::Null) => p.satin_underlay = None,
            Some(Value::Array(a)) => {
                let mut m = 0;
                for x in a {
                    m |= x
                        .as_str()
                        .and_then(underlay::parse)
                        .ok_or("satin_underlay items must be center, contour or zigzag")?;
                }
                p.satin_underlay = Some(m);
                p.underlay = None;
            }
            Some(_) => return Err("satin_underlay must be a list or \"auto\"".into()),
        }
        match args.get("color") {
            None => {}
            Some(Value::Null) => color = paint,
            Some(Value::String(s)) => color = parse_hex(s).ok_or("color must look like #rrggbb")?,
            Some(_) => return Err("color must look like #rrggbb".into()),
        }
        self.structural(|d| {
            d.shapes[i].params = p;
            d.shapes[i].color = color;
            Ok(())
        })
        .map_err(|e| e.to_string())?;
        self.replan_soon();
        Ok(ToolResult::json(&self.object_json(i)))
    }

    /// That `v` names object `i` in full, not by a part of its name: for
    /// deleting ("sun", deleted, would be "sunbeam" the second time).
    fn named_in_full(&self, v: Option<&Value>, i: usize) -> Result<(), String> {
        let Some(Value::String(asked)) = v else {
            return Ok(());
        };
        let name = &self.names[i];
        let words = zigzag::svg::name_words(asked).join(" ");
        let same = |x: &str| x.eq_ignore_ascii_case(name);
        if same(asked) || same(&words) || self.doc.shapes[i].id == *asked {
            Ok(())
        } else {
            Err(format!(
                "no object named '{asked}' (the nearest is '{name}': name it in full to delete it)"
            ))
        }
    }

    /// `set_object` with `objects`: the same change to each, one undo step.
    fn set_objects(&mut self, args: &Value) -> ToolResult {
        let idx = match self.resolve_many(args.get("objects")) {
            Ok(idx) => idx,
            Err(e) => return ToolResult::error(e),
        };
        let mut each = args.clone();
        if let Some(o) = each.as_object_mut() {
            o.remove("objects");
            o.remove("object");
            o.remove("terse");
        }
        let step = |i: usize| {
            let mut a = each.clone();
            a["object"] = json!(i);
            a
        };
        if self.batching {
            // (In a batch already: its undo step, and its rollback.)
            for &i in &idx {
                let r = self.run_tool("set_object", &step(i));
                if r.is_error {
                    return r;
                }
            }
            return ToolResult::json(&json!({ "ok": true, "changed": idx.len() }));
        }
        let steps: Vec<Value> = idx
            .iter()
            .map(|&i| json!({ "tool": "set_object", "arguments": step(i) }))
            .collect();
        self.tool_batch(&json!({ "steps": steps, "terse": true }))
    }

    /// `align_objects`: line objects up, or space them evenly.
    fn tool_align(&mut self, args: &Value) -> Result<ToolResult, String> {
        let idx = self.resolve_many(args.get("objects"))?;
        let word = |k: &str| args.get(k).and_then(Value::as_str);
        let (align, distribute) = (word("align"), word("distribute"));
        if align.is_none() && distribute.is_none() {
            return Err("give `align` or `distribute`".into());
        }
        // What moves as one: an element's fill and its outline.
        let mut units: Vec<(Vec<usize>, zigzag::geom::Bounds)> = Vec::new();
        for &i in &idx {
            let s = &self.doc.shapes[i];
            let b = shape_bounds(s);
            if b.is_empty() {
                continue;
            }
            match units
                .iter_mut()
                .find(|(u, _)| !s.id.is_empty() && self.doc.shapes[u[0]].id == s.id)
            {
                Some((u, ub)) => {
                    u.push(i);
                    ub.add(b.min);
                    ub.add(b.max);
                }
                None => units.push((vec![i], b)),
            }
        }
        if units.is_empty() {
            return Err("those objects have no size".into());
        }
        let mut all = zigzag::geom::Bounds::empty();
        match word("to") {
            Some("hoop") => {
                let (hw, hh) = self.hoop_dims();
                all.add(P::new(-hw * 5.0, -hh * 5.0));
                all.add(P::new(hw * 5.0, hh * 5.0));
            }
            None | Some("objects") => units.iter().for_each(|(_, b)| {
                all.add(b.min);
                all.add(b.max);
            }),
            _ => return Err("to must be objects or hoop".into()),
        }
        let mut shift = vec![P::default(); units.len()];
        if let Some(a) = align {
            for (k, (_, b)) in units.iter().enumerate() {
                let c = b.center();
                shift[k] = match a {
                    "left" => P::new(all.min.x - b.min.x, 0.0),
                    "right" => P::new(all.max.x - b.max.x, 0.0),
                    "center" => P::new(all.center().x - c.x, 0.0),
                    "top" => P::new(0.0, all.min.y - b.min.y),
                    "bottom" => P::new(0.0, all.max.y - b.max.y),
                    "middle" => P::new(0.0, all.center().y - c.y),
                    _ => {
                        return Err(
                            "align must be left, center, right, top, middle or bottom".into()
                        );
                    }
                };
            }
        }
        if let Some(d) = distribute {
            let across = match d {
                "horizontal" => true,
                "vertical" => false,
                _ => return Err("distribute must be horizontal or vertical".into()),
            };
            if units.len() < 3 {
                return Err("distribute needs three or more objects".into());
            }
            let at = |b: &zigzag::geom::Bounds| if across { b.center().x } else { b.center().y };
            let mut order: Vec<usize> = (0..units.len()).collect();
            order.sort_by(|&a, &b| at(&units[a].1).total_cmp(&at(&units[b].1)));
            let (first, last) = (at(&units[order[0]].1), at(&units[order[order.len() - 1]].1));
            let step = (last - first) / (order.len() - 1) as f64;
            for (n, &k) in order.iter().enumerate() {
                let d = first + step * n as f64 - at(&units[k].1);
                if across {
                    shift[k].x += d;
                } else {
                    shift[k].y += d;
                }
            }
        }
        self.structural(|d| {
            for ((u, _), by) in units.iter().zip(&shift) {
                if by.x != 0.0 || by.y != 0.0 {
                    d.transform(u, zigzag::geom::translation(*by))?;
                }
            }
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))?;
        self.replan_soon();
        self.set_selection(&idx);
        Ok(ToolResult::json(&json!({
            "moved": idx.iter().map(|&i| json!({
                "name": self.names.get(i),
                "center_mm": self.object_json(i)["center_mm"],
            })).collect::<Vec<_>>()
        })))
    }

    /// `combine_shapes`: one shape of several (the Pathfinder).
    fn tool_combine(&mut self, args: &Value) -> Result<ToolResult, String> {
        let idx = self.resolve_many(args.get("objects"))?;
        if idx.len() < 2 {
            return Err("combine_shapes takes two or more shapes".into());
        }
        let op = match args.get("op").and_then(Value::as_str) {
            None | Some("unite") => zigzag::boolean::PathOp::Unite,
            Some("minus_front") => zigzag::boolean::PathOp::MinusFront,
            Some("intersect") => zigzag::boolean::PathOp::Intersect,
            Some("exclude") => zigzag::boolean::PathOp::Exclude,
            _ => return Err("op must be unite, minus_front, intersect or exclude".into()),
        };
        let mut new = 0;
        self.structural(|d| {
            new = d.pathfinder(&idx, op)?;
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))?;
        self.replan_soon();
        self.set_selection(&[new]);
        Ok(ToolResult::json(
            &json!({ "added": [self.object_json(new)] }),
        ))
    }

    /// The id of the group called `name` (its name as it's shown, or its
    /// id), among the groups objects show in.
    fn group_called(&self, name: &str) -> Result<String, String> {
        let mut ids: Vec<String> = Vec::new();
        for i in 0..self.doc.shapes.len() {
            if let Some(g) = self.shown_group(i)
                && !ids.contains(&g)
            {
                ids.push(g);
            }
        }
        let is = |g: &&String| {
            *g == name || Studio::group_label(g).eq_ignore_ascii_case(name.trim())
        };
        let found: Vec<&String> = ids.iter().filter(is).collect();
        match found.as_slice() {
            [g] => Ok((*g).clone()),
            [] => Err(format!(
                "no group called '{name}'. Groups: {}",
                if ids.is_empty() {
                    "none".to_string()
                } else {
                    ids.iter()
                        .map(|g| Studio::group_label(g))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            )),
            many => Err(format!(
                "{} groups are called '{name}': name the one by its id ({})",
                many.len(),
                many.iter().map(|g| g.as_str()).collect::<Vec<_>>().join(", ")
            )),
        }
    }

    /// group_objects: a group made of objects, or objects put into one.
    fn tool_group(&mut self, args: &Value) -> Result<ToolResult, String> {
        let idx = self.resolve_many(args.get("objects"))?;
        let name = args.get("name").and_then(Value::as_str);
        let uids: Vec<usize> = idx.iter().map(|&i| self.doc.shapes[i].uid).collect();
        let group = match args.get("into").and_then(Value::as_str) {
            Some(into) => {
                let id = self.group_called(into)?;
                // Sewn with the group, after the last of it: listed with
                // it, not as a part of it somewhere else in the order.
                let last = self
                    .group_members(&id)
                    .into_iter()
                    .filter(|m| !idx.contains(m))
                    .max();
                let before = last.and_then(|last| {
                    (last + 1..self.doc.shapes.len())
                        .find(|i| !idx.contains(i))
                        .map(|i| self.doc.shapes[i].uid)
                });
                let mut moved = false;
                self.structural(|d| {
                    moved = d.regroup(&idx, Some(&id))?;
                    if !moved {
                        anyhow::bail!("they're in that group already");
                    }
                    if last.is_some() {
                        Studio::reorder(d, &uids, before);
                    }
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
                id
            }
            None => {
                if idx.len() < 2 {
                    return Err(
                        "a group is of two or more objects (or `into` a group there already)"
                            .into(),
                    );
                }
                let Some(name) = name.filter(|n| !n.trim().is_empty()) else {
                    return Err("give the group a `name` that says what it is".into());
                };
                // Sewn together, where the last of them is.
                let last = idx.iter().copied().max().unwrap_or(0);
                let before = (last + 1..self.doc.shapes.len())
                    .find(|i| !idx.contains(i))
                    .map(|i| self.doc.shapes[i].uid);
                let mut id = String::new();
                self.structural(|d| {
                    let made = d.group(&idx)?;
                    id = d.rename_group(&made, name)?;
                    Studio::reorder(d, &uids, before);
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
                id
            }
        };
        self.replan_soon();
        let members = self.group_members(&group);
        Ok(ToolResult::text(format!(
            "The group '{}' has {} objects: {}.",
            Studio::group_label(&group),
            members.len(),
            members
                .iter()
                .filter_map(|&i| self.names.get(i).cloned())
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }

    pub(crate) fn resolve_many(&self, v: Option<&Value>) -> Result<Vec<usize>, String> {
        match v {
            // (Everything, unless something is called that.)
            Some(Value::String(s))
                if s == "all" && !self.doc.shapes.is_empty() && self.resolve(v).is_err() =>
            {
                Ok((0..self.doc.shapes.len()).collect())
            }
            Some(Value::Array(a)) if !a.is_empty() => {
                let mut out = Vec::new();
                for x in a {
                    let i = self.resolve(Some(x))?;
                    if !out.contains(&i) {
                        out.push(i);
                    }
                }
                Ok(out)
            }
            Some(Value::Array(_)) => Err("`objects` is empty".into()),
            v => self.resolve(v).map(|i| vec![i]),
        }
    }

    fn pair(args: &Value, k: &str) -> Result<Option<(f64, f64)>, String> {
        match args.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(a)) if a.len() == 2 => match (a[0].as_f64(), a[1].as_f64()) {
                (Some(x), Some(y)) => Ok(Some((x, y))),
                _ => Err(format!("`{k}` must be [x, y] numbers")),
            },
            _ => Err(format!("`{k}` must be [x, y]")),
        }
    }

    fn tool_transform(&mut self, args: &Value) -> Result<ToolResult, String> {
        use zigzag::geom::{affine_mul, rotate_about, scale_about, translation};
        let idx = self.resolve_many(args.get("objects"))?;
        self.set_selection(&idx);
        let b = self.selection_bounds();
        if b.is_empty() {
            return Err("those objects have no size".into());
        }
        let c = b.center();
        let num = |k: &str| {
            args.get(k)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
        };
        let (mut sx, mut sy) = (1.0, 1.0);
        if let Some(k) = num("scale") {
            if k <= 0.0 {
                return Err("scale must be positive".into());
            }
            (sx, sy) = (k, k);
        }
        for k in ["width_mm", "height_mm"] {
            if num(k).is_some_and(|v| v <= 0.0) {
                return Err(format!("{k} must be positive"));
            }
        }
        // (A line has no height to make 2 mm of: the scale would be
        // millions.)
        for (k, extent) in [("width_mm", b.width()), ("height_mm", b.height())] {
            if num(k).is_some() && extent < 0.01 {
                return Err(format!(
                    "these objects have no {} to resize: give the other side, or scale",
                    if k == "width_mm" { "width" } else { "height" }
                ));
            }
        }
        match (num("width_mm"), num("height_mm")) {
            (Some(w), Some(h)) => {
                (sx, sy) = (
                    w * 10.0 / b.width().max(1e-6),
                    h * 10.0 / b.height().max(1e-6),
                )
            }
            (Some(w), None) => {
                (sx, sy) = (
                    w * 10.0 / b.width().max(1e-6),
                    w * 10.0 / b.width().max(1e-6),
                )
            }
            (None, Some(h)) => {
                (sx, sy) = (
                    h * 10.0 / b.height().max(1e-6),
                    h * 10.0 / b.height().max(1e-6),
                )
            }
            _ => {}
        }
        let mut m = scale_about(c, sx, sy);
        match args.get("mirror").and_then(Value::as_str) {
            Some("horizontal") => m = affine_mul(scale_about(c, -1.0, 1.0), m),
            Some("vertical") => m = affine_mul(scale_about(c, 1.0, -1.0), m),
            Some(_) => return Err("mirror must be horizontal or vertical".into()),
            None => {}
        }
        if let Some(deg) = num("rotate_deg") {
            m = affine_mul(rotate_about(c, deg), m);
        }
        let shift = match (
            Self::pair(args, "move_to_mm")?,
            Self::pair(args, "move_by_mm")?,
        ) {
            (Some((x, y)), _) => P::new(x * 10.0, y * 10.0) - c,
            (None, Some((dx, dy))) => P::new(dx * 10.0, dy * 10.0),
            _ => P::default(),
        };
        m = affine_mul(translation(shift), m);
        if !self.transform_selected(m, "Transformed") && m != zigzag::geom::IDENTITY {
            return Err(self.status.clone());
        }
        self.replan_soon();
        let objs: Vec<Value> = self
            .sel_indices()
            .iter()
            .map(|&i| self.object_json(i))
            .collect();
        Ok(ToolResult::json(
            &json!({ "objects": objs, "design_problems": self.issues.iter().filter(|x| x.shape.is_none()).map(|x| issue_json(x, &self.names)).collect::<Vec<_>>() }),
        ))
    }

    fn tool_duplicate(&mut self, args: &Value) -> Result<ToolResult, String> {
        let idx = self.resolve_many(args.get("objects"))?;
        let (dx, dy) = Self::pair(args, "offset_mm")?.unwrap_or((3.0, 3.0));
        let mut new = Vec::new();
        self.structural(|d| {
            new = d.duplicate(&idx, P::new(dx * 10.0, dy * 10.0))?;
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))?;
        self.replan_soon();
        self.set_selection(&new);
        Ok(ToolResult::json(
            &json!({ "added": new.iter().map(|&i| self.object_json(i)).collect::<Vec<_>>() }),
        ))
    }

    fn tool_set_document(&mut self, args: &Value) -> Result<ToolResult, String> {
        let mut s = self.doc.settings.clone();
        if let Some(f) = args.get("fabric") {
            let id = f.as_str().ok_or("fabric must be a string")?;
            zigzag::params::Fabric::by_id(id).ok_or(format!(
                "unknown fabric; use one of {}",
                FABRICS.iter().map(|f| f.id).collect::<Vec<_>>().join(", ")
            ))?;
            s.fabric = id.to_string();
        }
        if let Some(m) = args.get("machine") {
            let name = m.as_str().ok_or("machine must be a string")?;
            if name.trim().eq_ignore_ascii_case("none") {
                s.machine = None;
            } else {
                let p = self.machines.named(name).ok_or(format!(
                    "unknown machine; use \"none\" or one of {} (see list_machines)",
                    self.machines
                        .all()
                        .map(|p| p.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))?;
                p.apply(&mut s, !self.doc.shapes.is_empty());
            }
        }
        if let Some(v) = args.get("min_stitch_mm") {
            s.min_stitch_mm = v
                .as_f64()
                .filter(|m| zigzag::document::MIN_STITCH_MM.contains(m))
                .ok_or("min_stitch_mm must be from 0.1 to 1")?;
        }
        if let Some(v) = args.get("narrow_satin_mm") {
            s.narrow_satin_mm = v
                .as_f64()
                .filter(|m| zigzag::document::NARROW_SATIN_MM.contains(m))
                .ok_or("narrow_satin_mm must be from 0 to 3")?;
        }
        if let Some(v) = args.get("lock") {
            s.lock = zigzag::machine::profile::lock_from(v, s.lock).ok_or(
                "lock must have a style of back, triangle or none, and may have length_mm and count",
            )?;
        }
        if let Some(h) = args.get("hoop") {
            let id = h.as_str().ok_or("hoop must be a string")?;
            s.hoop = zigzag::document::parse_hoop(id).ok_or(format!(
                "hoop must be width by height in mm, each side 10 to 1000 (e.g. 150x220); the standard hoops are {}",
                HOOPS.iter().map(|h| h.0).collect::<Vec<_>>().join(", ")
            ))?;
        }
        if let Some(v) = args.get("remove_overlaps") {
            s.remove_overlaps = v.as_bool().ok_or("remove_overlaps must be true or false")?;
        }
        if let Some(v) = args.get("hat_order") {
            s.hat_order = v.as_bool().ok_or("hat_order must be true or false")?;
        }
        if let Some(v) = args.get("over_sewn") {
            s.over_sewn = v.as_bool().ok_or("over_sewn must be true or false")?;
        }
        if let Some(v) = args.get("connect_mm") {
            s.connect_mm = v
                .as_f64()
                .filter(|x| x.is_finite() && *x >= 0.0)
                .ok_or("connect_mm must be a number of millimeters")?
                .min(50.0);
        }
        if let Some(v) = args.get("blend_mm") {
            s.blend_mm = v
                .as_f64()
                .filter(|x| x.is_finite() && (0.0..=zigzag::document::BLEND_MM_MOST).contains(x))
                .ok_or("blend_mm must be from 0 to 15")?;
        }
        if let Some(v) = args.get("preview_color") {
            s.cloth = v
                .as_str()
                .and_then(parse_hex)
                .ok_or("preview_color must look like #rrggbb")?;
        }
        let on = [
            ("over_sewn", s.over_sewn),
            ("connect_mm", s.connect_mm > 0.0),
            ("blend_mm", s.blend_mm > 0.0),
        ];
        self.structural(|d| {
            d.settings = s;
            Ok(())
        })
        .map_err(|e| e.to_string())?;
        // Who turned each trim setting on: the agent, or the person (a card
        // of choices applies the person's).
        let by_agent = !self.person_choosing;
        let set = self.agent_set.entry(self.session.rec).or_default();
        for (key, on) in on {
            if args.get(key).is_some() {
                if on && by_agent {
                    set.insert(key);
                } else {
                    set.remove(key);
                }
            }
        }
        self.replan_soon();
        Ok(ToolResult::json(&self.design_json()["document"]))
    }

    /// review_design: what the design costs to sew and what may not read
    /// clearly, with what to do about each, and the stitches at about
    /// their real size.
    fn tool_review_design(&mut self) -> Result<ToolResult, String> {
        use std::collections::HashMap;
        use zigzag::formats::perceptual_distance as delta;
        let d = &self.design;
        let shapes = &self.doc.shapes;
        let name = |i: usize| {
            self.names
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("object {i}"))
        };
        let sewn = |i: usize| d.per_shape.get(i).copied().unwrap_or(0) > 0;
        // Trims, each put down to the object sewn after it.
        let mut by_object: HashMap<usize, usize> = HashMap::new();
        for (b, blk) in d.blocks.iter().enumerate() {
            for (r, run) in blk.runs.iter().enumerate() {
                if !run.trim_before {
                    continue;
                }
                if let Some(&(_, si)) = d
                    .stitch_shapes
                    .get(b)
                    .and_then(|x| x.get(r))
                    .and_then(|x| x.first())
                {
                    *by_object.entry(si).or_default() += 1;
                }
            }
        }
        let trims = d.trim_count();
        let mut worst: Vec<(usize, usize)> = by_object.into_iter().collect();
        worst.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let trims_by_object: Vec<Value> = worst
            .iter()
            .take(12)
            .map(|&(i, n)| json!({ "object": name(i), "trims": n }))
            .collect();
        // Threads sewn in more than one block: a thread change each time.
        let mut blocks_of: Vec<([u8; 3], usize)> = Vec::new();
        for blk in &d.blocks {
            match blocks_of.iter_mut().find(|(c, _)| *c == blk.color) {
                Some(x) => x.1 += 1,
                None => blocks_of.push((blk.color, 1)),
            }
        }
        let repeated: Vec<Value> = blocks_of
            .iter()
            .filter(|x| x.1 > 1)
            .map(|&(c, n)| json!({ "thread": hex(c), "blocks": n }))
            .collect();
        // Pieces too small to read, or to sew well.
        let small: Vec<Value> = (0..shapes.len())
            .filter(|&i| sewn(i))
            .filter_map(|i| {
                let b = shape_bounds(&shapes[i]);
                let big = b.width().max(b.height()) / 10.0;
                (big < 1.5).then(|| {
                    json!({ "object": name(i), "kind": kind_name(shapes[i].params.kind.unwrap_or(Kind::Fill)), "size_mm": mm(big) })
                })
            })
            .take(20)
            .collect();
        // Threads too close to the fabric, and touching objects in threads
        // too alike to tell apart.
        // (Only a thread with something sewn straight on the fabric: one
        // that's all on top of a fill sewn before it shows against that.)
        let cloth = self.doc.settings.cloth;
        let bounds: Vec<zigzag::geom::Bounds> = shapes.iter().map(shape_bounds).collect();
        let mut place = vec![usize::MAX; shapes.len()];
        for (n, &i) in d.sew_order.iter().enumerate() {
            if let Some(p) = place.get_mut(i) {
                *p = n;
            }
        }
        let (bounds, place) = (&bounds, &place);
        let on_fabric = |c: [u8; 3]| {
            let pos = |i: usize| place[i];
            (0..shapes.len())
                .filter(|&i| sewn(i) && shapes[i].color == c)
                .any(|i| {
                    let b = bounds[i];
                    !(0..shapes.len()).any(|j| {
                        j != i
                            && sewn(j)
                            && matches!(shapes[j].geometry, Geometry::Fill { .. })
                            && shapes[j].color != c
                            && pos(j) < pos(i)
                            && {
                                let u = bounds[j];
                                u.min.x <= b.min.x
                                    && u.min.y <= b.min.y
                                    && u.max.x >= b.max.x
                                    && u.max.y >= b.max.y
                            }
                    })
                })
        };
        let faint: Vec<Value> = blocks_of
            .iter()
            .filter(|(c, _)| delta(*c, cloth) < 12.0 && on_fabric(*c))
            .map(|&(c, _)| json!({ "thread": hex(c), "fabric": hex(cloth), "difference": mm(delta(c, cloth)) }))
            .collect();
        let mut alike: Vec<Value> = Vec::new();
        let mut seen: Vec<([u8; 3], [u8; 3])> = Vec::new();
        let mut apart: std::collections::HashMap<([u8; 3], [u8; 3]), f64> = Default::default();
        let near = |a: &zigzag::geom::Bounds, b: &zigzag::geom::Bounds| {
            a.min.x <= b.max.x + 5.0
                && b.min.x <= a.max.x + 5.0
                && a.min.y <= b.max.y + 5.0
                && b.min.y <= a.max.y + 5.0
        };
        let live: Vec<usize> = (0..shapes.len()).filter(|&i| sewn(i)).collect();
        for (k, &i) in live.iter().enumerate() {
            for &j in &live[k + 1..] {
                let (a, b) = (shapes[i].color, shapes[j].color);
                if a == b || seen.contains(&(a, b)) || seen.contains(&(b, a)) {
                    continue;
                }
                // (How alike two threads are is worked out once a pair.)
                let pair = if a < b { (a, b) } else { (b, a) };
                let dist = *apart.entry(pair).or_insert_with(|| delta(a, b));
                if dist < 6.0 && near(&bounds[i], &bounds[j]) {
                    seen.push((a, b));
                    alike.push(json!({ "objects": [name(i), name(j)], "threads": [hex(a), hex(b)], "difference": mm(dist) }));
                }
            }
        }
        let bridges = self.bridges_in_view();
        // The settings that are the person's to choose (they can show on
        // the fabric), tried on copies: what each would save.
        let what_ifs = self.trim_what_ifs(trims);
        let (stitch_types, fill_angles) = self.stitch_suggestions();
        let (underlay_off, too_open, same_direction, common_angle) = self.sewing_basics();
        let (outline_only, border, small_lettering, underlay_mismatch, lines_on_ground, stray_loops) =
            self.finishing_checks();
        let count = |l: Level| self.issues.iter().filter(|x| x.level == l).count();
        // Everything the Issues button counts, notes too (holes, piled
        // thread, covered objects are notes): the person sees them all.
        let problems: Vec<Value> = self
            .issues
            .iter()
            .take(30)
            .map(|x| issue_json(x, &self.names))
            .collect();
        // What to do: the sewing itself first (what an embroiderer looks at
        // before anything else), then what it costs.
        let mut findings: Vec<String> = Vec::new();
        // (The trim settings the agent turned on itself; the rest are the
        // person's, and stay as they are.)
        let mine = |k: &str| self.agent_set.get(&self.session.rec).is_some_and(|s| s.contains(k));
        if !underlay_off.is_empty() {
            findings.push(format!(
                "{} object(s) sew with no underlay where they need it (underlay_off). Underlay holds the cloth still and lifts the top stitches: without it a fill sinks, puckers the cloth and shows it between its rows, and a wide satin's edges go ragged. Put it back (set_object with each one's fix); turn underlay off only on a fleck too small for it.",
                underlay_off.len()
            ));
        }
        if !too_open.is_empty() {
            findings.push(format!(
                "{} object(s) sew their rows further apart than the fabric's usual (too_open): the cloth shows between them, so they read as stripes or outlines rather than filled in. Set them back to the usual (set_object spacing_mm null). A lighter look is a different stitch type or a thread a shade off, never open rows. (Only a ground, a background fill other pieces sit on, may be sewn lighter, and only on cloth close to its thread's color: up to twice the usual spacing, still with underlay.)",
                too_open.len()
            ));
        }
        if !same_direction.is_empty() || common_angle.is_some() {
            let most = common_angle
                .map(|(a, n, of)| format!(" {n} of the {of} fills run at {a}°;"))
                .unwrap_or_default();
            findings.push(format!(
                "Fills running the same way pull the same way:{most} {} sit on a fill at its own angle (same_direction). Rows that run on from the rows beside them add their pull together, which puckers the cloth and gaps the edges, and the shapes don't stand apart. Give each piece its own direction: along its length where it has one (fill_angles), and at least 30° from what it sits on or touches; a background ground at a different angle from everything on it.",
                same_direction.len()
            ));
        }
        if !underlay_mismatch.is_empty() {
            findings.push(format!(
                "{} satin(s) have underlay set by hand that doesn't suit their width (underlay_mismatch): set them back to auto (set_object satin_underlay \"auto\"), which puts none under 1.5 mm, a center walk up to 4 mm and an edge walk with zigzag above.",
                underlay_mismatch.len()
            ));
        }
        if !outline_only.is_empty() {
            findings.push(format!(
                "{} shape(s) are sewn only round their edge with nothing filling them (outline_only): they read as empty outlines. Look at each close up; fill the ones meant to be solid (set_object kind fill), and leave rings, frames and line drawings.",
                outline_only.len()
            ));
        }
        if !lines_on_ground.is_empty() {
            findings.push(format!(
                "{} line(s) are sewn across the background ground over nothing else (lines_on_ground). Keep a ground plain: delete each one that isn't part of the picture; if one is meant (a road, a horizon), keep it and say so in your summary.",
                lines_on_ground.len()
            ));
        }
        if !stray_loops.is_empty() {
            findings.push(format!(
                "{} small shape(s) sit inside another color sewn only as a loop of running stitch (stray_loops): usually specks the trace left. Look at each close up; delete the stray ones, and sew the ones that are real details as fill.",
                stray_loops.len()
            ));
        }
        if let Some(b) = &border {
            findings.push(format!(
                "The patch border {}: {}.",
                b["object"].as_str().unwrap_or(""),
                b["why"].as_str().unwrap_or("")
            ));
        }
        if !small_lettering.is_empty() {
            findings.push(format!(
                "{} lettering object(s) are too small to read in thread (small_lettering, each with its tallest letter): make them bigger, or fewer words larger. Lettering is the person's message, so if it can't grow, ask them what to do with offer_choices.",
                small_lettering.len()
            ));
        }
        if self.doc.settings.over_sewn {
            findings.push(if mine("over_sewn") {
                "You turned Sew over fill on: where an object's thread can't get from one part to the next out of sight it runs across finished fill, and those lines show on top of it and clutter it. Turn it off (set_document over_sewn false) unless the person asks for it: a trim is cheaper than a line across the work.".to_string()
            } else {
                "Sew over fill is on, as the person set it: the lines its thread leaves across finished fill are their trade-off for fewer trims. Leave it, and say what it costs in your summary.".to_string()
            });
        }
        if trims > 0 {
            let top: Vec<String> = worst
                .iter()
                .take(4)
                .map(|&(i, n)| format!("{} ({n})", name(i)))
                .collect();
            findings.push(format!(
                "{trims} trims (besides {} thread changes); most before {}. Each separate piece of a color is a trim: delete specks and stray bits, or join pieces that should be one shape. Don't take out shading, highlights or features for it.",
                d.blocks.len().saturating_sub(1),
                top.join(", ")
            ));
        }
        // The setting that saves the most and doesn't show: set it. Those
        // that save more but show are the person's to choose.
        // (Never sewing over fill: its thread lies across finished stitching,
        // which an embroiderer sees as clutter even in its own color.)
        let hidden = what_ifs
            .iter()
            .filter(|w| w["shows"] == false && w["set_document"].get("over_sewn").is_none())
            .max_by_key(|w| w["saves"].as_u64().unwrap_or(0));
        if let Some(h) = hidden {
            findings.push(format!(
                "{} would bring trims from {trims} to {} without a line of thread showing: its travel ({} mm) lies over stitching of its own color, where it reads as texture. Set it (set_document {}), and say so in your summary.",
                h["change"].as_str().unwrap_or(""),
                h["trims"],
                h["thread_in_view_mm"],
                h["set_document"]
            ));
        }
        // (Asked about only when it's worth the person's while: five
        // fewer stops to cut the thread, or a tenth of them.)
        let worth = |w: &Value| {
            let saves = w["saves"].as_u64().unwrap_or(0) as usize;
            saves >= 5 || saves * 10 >= trims.max(1)
        };
        let showing: Vec<String> = what_ifs
            .iter()
            .filter(|w| w["shows"] != false && worth(w))
            .filter(|w| hidden.is_none_or(|h| w["trims"].as_u64() < h["trims"].as_u64()))
            .map(|w| {
                format!(
                    "{}: {} trims, with about {} mm of thread lying on another color",
                    w["change"].as_str().unwrap_or(""),
                    w["trims"],
                    w["on_another_color_mm"]
                )
            })
            .collect();
        if !showing.is_empty() {
            findings.push(format!(
                "Settings that would save more trims show in the stitching ({}). Those are the person's to choose: offer them with offer_choices (each choice's steps: set_document with its set_document), saying in plain words what they'd see and what it means for sewing (the machine stops to cut the thread fewer times, so it sews faster, but some short lines of thread show).",
                showing.join("; ")
            ));
        }
        if !repeated.is_empty() {
            findings.push(format!(
                "{} thread(s) sewn in more than one block, a thread change each time: group_colors joins them where nothing between has to sit on top.",
                repeated.len()
            ));
        }
        if !stitch_types.is_empty() {
            findings.push(format!(
                "{} object(s) may sew better as another stitch type (stitch_types, with why): change one with set_object kind, look at it close up with render_preview, and keep it if it reads better.",
                stitch_types.len()
            ));
        }
        if !fill_angles.is_empty() {
            findings.push(format!(
                "{} fill(s) would read with more flow or dimension at another angle (fill_angles: each with the angle to try and why): set_object angle, look at the stitches close up, and keep what reads better.",
                fill_angles.len()
            ));
        }
        if !small.is_empty() {
            findings.push(format!(
                "{} piece(s) under 1.5 mm: fine as running-stitch details (eyes, claws); a filled piece that small won't read, so make it bigger, sew it as run, or delete it if it's a stray bit.",
                small.len()
            ));
        }
        if bridges > 0 {
            findings.push(if mine("connect_mm") {
                format!("{bridges} gap(s) you had sewn across (connect_mm) show on what they cross. Set connect_mm back to 0, or to what saves trims without showing (what_ifs), unless the person asks for it.")
            } else {
                format!("{bridges} gap(s) are sewn across where the thread shows (connect_mm, the person's setting). That's their trade-off: leave it, and say what it costs in your summary.")
            });
        }
        if !faint.is_empty() {
            findings.push("A thread is close to the fabric's color and will barely show: use a lighter or darker shade, or an outline.".into());
        }
        if !alike.is_empty() {
            findings.push("Touching objects in threads too alike to tell apart: make them one thread, or set them further apart in shade.".into());
        }
        if !self.issues.is_empty() {
            findings.push(format!(
                "The Issues button shows the person {} issue(s): {} error(s), {} warning(s), {} note(s) (each in problems, with its object). Fix every error and warning. Fix the notes that are about the sewing: needle holes bunched in one spot, thread piled thick, an object covered so it won't sew, thread crossing stitching already sewn. Notes that only say what the planner did (a wide satin split), or come from a trim setting the person chose (Sew over fill, Connect, Blend), can stay. Run review_design again after, and say in your summary which issues are left and why.",
                self.issues.len(),
                count(Level::Error),
                count(Level::Warning),
                count(Level::Info)
            ));
        }
        if findings.is_empty() {
            findings.push("Nothing to fix: it should sew cleanly and read well.".into());
        }
        let total = d.stitch_count();
        let b = d.bounds();
        let report = json!({
            "findings": findings,
            "cost": {
                "threads": blocks_of.len(),
                "thread_changes": d.blocks.len().saturating_sub(1),
                "trims": trims,
                "stitches": total,
                "minutes": (total as f64 / SPM).ceil(),
            },
            "trims_by_object": trims_by_object,
            "what_ifs": what_ifs,
            "stitch_types": stitch_types,
            "fill_angles": fill_angles,
            "underlay_off": underlay_off,
            "too_open": too_open,
            "same_direction": same_direction,
            "underlay_mismatch": underlay_mismatch,
            "outline_only": outline_only,
            "patch_border": border,
            "small_lettering": small_lettering,
            "lines_on_ground": lines_on_ground,
            "stray_loops": stray_loops,
            "threads_in_more_than_one_block": repeated,
            "small_pieces": small,
            "gaps_sewn_in_view": bridges,
            "threads_close_to_fabric": faint,
            "touching_threads_alike": alike,
            "problems": problems,
            "picture": if b.width() / 10.0 / 25.4 * 96.0 >= 200.0 {
                "the stitches at about their real size on a screen (96 pixels an inch): does it still read?".to_string()
            } else {
                format!(
                    "the stitches, drawn about {:.1} times their real size (the design is small): at its real size it's that much harder to read.",
                    200.0 / (b.width() / 10.0 / 25.4 * 96.0).max(1.0)
                )
            },
        });
        // (At 96 per inch; a small design is drawn no smaller than 200 px.)
        let px = ((b.width() / 10.0) / 25.4 * 96.0).round().max(1.0) as u64;
        let mut out = ToolResult::json(&report);
        if total > 0
            && let Ok(r) = self.tool_render(&json!({ "width_px": px, "show_jumps": false }))
        {
            out.content.extend(
                r.content
                    .into_iter()
                    .filter(|c| matches!(c, Content::Image(_))),
            );
        }
        Ok(out)
    }

    /// Make the ticked changes on the card of choices at line `k` of the
    /// chat: their steps in one batch (all or nothing, one undo step).
    pub(crate) fn apply_choices(&mut self, k: usize) {
        use crate::agent::Entry;
        let Some(Entry::Choices {
            choices, applied, ..
        }) = self.agent.log.get(k)
        else {
            return;
        };
        if applied.is_some() || self.agent.running() {
            return;
        }
        // (The one clicked: a card asks for one choice.)
        let picked: Vec<&crate::agent::Choice> =
            choices.iter().filter(|c| c.picked).take(1).collect();
        if picked.is_empty() {
            return;
        }
        let labels: Vec<String> = picked.iter().map(|c| c.label.clone()).collect();
        // Keeping it as it is: nothing to make; the card closes.
        if picked
            .iter()
            .all(|c| c.steps.as_array().is_none_or(|s| s.is_empty()))
        {
            if let Some(Entry::Choices { applied, .. }) = self.agent.log.get_mut(k) {
                *applied = Some(labels);
            }
            // (The card says so.)
            return;
        }
        let steps: Vec<Value> = picked
            .iter()
            .flat_map(|c| c.steps.as_array().cloned().unwrap_or_default())
            .collect();
        self.person_choosing = true;
        let r = self.run_tool(
            "batch",
            &json!({ "steps": steps, "label": format!("Applied {}", labels.join(", ")) }),
        );
        self.person_choosing = false;
        if r.is_error {
            let said = serde_json::from_str::<Value>(&text_of(&r))
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_else(|| text_of(&r));
            self.agent
                .log
                .push(Entry::Info(format!("Couldn't apply that: {said}")));
            return;
        }
        if let Some(Entry::Choices { applied, .. }) = self.agent.log.get_mut(k) {
            *applied = Some(labels.clone());
        }
        self.agent
            .log
            .push(Entry::Info(format!("Applied {}.", labels.join(", "))));
        self.agent_changed();
    }

    /// offer_choices: a card of optional changes in the chat, for the
    /// person to pick from (see `Studio::apply_choices`).
    /// A step with its objects given by name instead of by number, where
    /// just one object has the name.
    fn pinned(&self, step: &Value) -> Value {
        let mut step = step.clone();
        let name = |v: &Value| -> Option<Value> {
            let i = v.as_u64()? as usize;
            let n = self.names.get(i)?;
            (self.names.iter().filter(|x| *x == n).count() == 1).then(|| json!(n))
        };
        if let Some(a) = step.get_mut("arguments").and_then(Value::as_object_mut) {
            for key in ["object", "objects"] {
                match a.get_mut(key) {
                    Some(Value::Array(list)) => {
                        for v in list.iter_mut() {
                            if let Some(n) = name(v) {
                                *v = n;
                            }
                        }
                    }
                    Some(v) => {
                        if let Some(n) = name(v) {
                            *v = n;
                        }
                    }
                    None => {}
                }
            }
        }
        step
    }

    fn tool_offer_choices(&mut self, args: &Value) -> Result<ToolResult, String> {
        /// Steps a choice can't have: what reaches outside this document
        /// or its history.
        const NOT: [&str; 12] = [
            "redo",
            "open_file",
            "new_document",
            "switch_document",
            "save",
            "export",
            "send_to_skitch",
            "undo",
            "batch",
            "offer_choices",
            "start_variants",
            "save_variant",
        ];
        let known: Vec<String> = tool_specs()
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let list = args
            .get("choices")
            .and_then(Value::as_array)
            .filter(|a| (1..=4).contains(&a.len()))
            .ok_or("`choices` must be a list of 1 to 4 choices (keep it to the few that matter: doing them all, and keeping it as it is, are added)")?;
        let mut choices = Vec::new();
        for (k, c) in list.iter().enumerate() {
            let said = |why: &str| format!("choice {}: {why}", k + 1);
            let label = c
                .get("label")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .ok_or_else(|| said("needs a `label`"))?;
            let steps = c
                .get("steps")
                .and_then(Value::as_array)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| said("needs `steps`: [{tool, arguments}, …]"))?;
            for st in steps {
                let tool = st.get("tool").and_then(Value::as_str).unwrap_or("");
                if !known.iter().any(|t| t == tool) {
                    return Err(said(&format!("there's no tool `{tool}`")));
                }
                if NOT.contains(&tool) || only_reads(tool) {
                    return Err(said(&format!("`{tool}` can't be a step of a choice")));
                }
                // (Found out now, while there's someone to put it right:
                // not when the person clicks.)
                let a = st.get("arguments").cloned().unwrap_or_else(|| json!({}));
                check_args(tool, &a).map_err(|e| said(&e))?;
                for key in ["object", "objects"] {
                    let named = match a.get(key) {
                        Some(Value::Array(list)) => list.clone(),
                        Some(v) if !v.is_null() => vec![v.clone()],
                        _ => Vec::new(),
                    };
                    for v in named
                        .iter()
                        .filter(|v| !is_ref(v) && v.as_str() != Some("all"))
                    {
                        self.resolve(Some(v)).map_err(|e| said(&e))?;
                    }
                }
            }
            // By name, where a name says which: a place in the sewing
            // order may be another object's by the time it's clicked.
            let steps: Vec<Value> = steps.iter().map(|st| self.pinned(st)).collect();
            let steps = &steps;
            choices.push(crate::agent::Choice {
                label: label.to_string(),
                detail: c
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                steps: Value::Array(steps.clone()),
                picked: false,
            });
        }
        let text = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .ok_or(format!(
                    "`{k}` is required: a clear title, and the question you ask"
                ))
        };
        let (title, question) = (text("title")?, text("question")?);
        // All of them at once, when they go together: two or more, and no
        // two setting the same thing differently.
        if choices.len() >= 2
            && args.get("combine").and_then(Value::as_bool) != Some(false)
            && combinable(&choices)
        {
            let steps: Vec<Value> = choices
                .iter()
                .flat_map(|c| c.steps.as_array().cloned().unwrap_or_default())
                .collect();
            choices.push(crate::agent::Choice {
                label: ALL.into(),
                detail: String::new(),
                steps: Value::Array(steps),
                picked: false,
            });
        }
        // Always a way out: no one is made to decide.
        choices.push(crate::agent::Choice {
            label: KEEP.into(),
            detail: String::new(),
            steps: Value::Array(Vec::new()),
            picked: false,
        });
        let n = choices.len();
        self.agent.log.push(crate::agent::Entry::Choices {
            title,
            question,
            choices,
            applied: None,
        });
        Ok(ToolResult::json(&json!({
            "offered": n,
            "note": "They're in the chat for the person to pick from; the one they click is made at once. Don't make these changes yourself.",
        })))
    }

    /// review_design's stitch-type pass: objects that may sew better as
    /// another type (a band filled that a satin column would sew cleaner, a
    /// satin too wide to hold), and fill angles for flow and dimension. A
    /// filled shape's width is taken as twice its area over its outline's
    /// length (a long thin shape's width), island by island, so a curved or
    /// diagonal band counts as the band it is.
    ///
    /// Angles: a fill with a clear length (its outline's principal axis at
    /// least 1.6 times its cross axis) is suggested rows along it (flow, as
    /// a digitizer lays stitches down a limb); then touching fills still
    /// within 20° of each other are set 45° apart (dimension: each catches
    /// the light its own way), turning the one without a length of its own,
    /// else the smaller. Only a change of 15° or more is suggested.
    fn stitch_suggestions(&self) -> (Vec<Value>, Vec<Value>) {
        let shapes = &self.doc.shapes;
        let sewn = |i: usize| self.design.per_shape.get(i).copied().unwrap_or(0) > 0;
        let name = |i: usize| self.names.get(i).cloned().unwrap_or_default();
        let mut types = Vec::new();
        // The fills: (index, area, bounds, angle now, its length's angle).
        let mut fills: Vec<(usize, f64, zigzag::geom::Bounds, f64, Option<f64>)> = Vec::new();
        for i in (0..shapes.len()).filter(|&i| sewn(i)) {
            let s = &shapes[i];
            let Geometry::Fill { rings, rule } = &s.geometry else {
                continue;
            };
            let kind = s.params.kind.unwrap_or(Kind::Fill);
            let (mut area, mut outline) = (0.0, 0.0);
            let comps = zigzag::stitch::satin::components(rings, *rule);
            let one_piece = comps.len() == 1;
            for c in comps {
                for (k, r) in c.iter().enumerate() {
                    let a = zigzag::boolean::ring_area(r);
                    area += if k == 0 { a } else { -a };
                    outline += zigzag::geom::polyline_length(r)
                        + r.last().zip(r.first()).map_or(0.0, |(a, b)| a.dist(*b));
                }
            }
            if area <= 0.0 || outline <= 0.0 {
                continue;
            }
            let width = 2.0 * area / outline / 10.0;
            let b = shape_bounds(s);
            let long = b.width().max(b.height()) / 10.0;
            // (A narrow average isn't enough: a lobed flower or leaf has a
            // long outline for its area too. A band is gone once shrunk by
            // a little over half its width; a broad shape keeps a core.)
            let band = (1.0..=4.0).contains(&width)
                && long >= 4.0 * width
                && {
                    let core: f64 = zigzag::boolean::offset(rings, *rule, -0.6 * width * 10.0)
                        .iter()
                        .map(|r| zigzag::boolean::ring_area(r))
                        .sum();
                    core.abs() < 0.05 * area
                };
            match kind {
                // (A band: no wider than a satin sews well, and long for
                // its width; not a broad piece with a long outline.)
                Kind::Fill if band => {
                    types.push(json!({
                        "object": name(i), "now": "fill", "suggest": "satin", "width_mm": mm(width),
                        "why": format!("about {width:.1} mm wide: a satin column sews a band this narrow cleaner than rows of fill, with a sheen that reads at a distance"),
                    }));
                }
                Kind::Satin if s.params.border != Some(true) && width > 7.0 => {
                    types.push(json!({
                        "object": name(i), "now": "satin", "suggest": "fill", "width_mm": mm(width),
                        "why": format!("about {width:.1} mm wide: satin stitches that long sit loose and snag"),
                    }));
                }
                _ => {}
            }
            // (A band goes to satin, whose stitches run across it anyway.)
            if kind == Kind::Fill && !band {
                let length = one_piece.then(|| principal_axis(rings)).flatten();
                fills.push((
                    i,
                    area,
                    b,
                    crate::widgets::normalize_angle(s.params.fill_angle()),
                    length.filter(|&(_, elong)| elong >= 1.6).map(|(a, _)| a),
                ));
            }
        }
        let apart = |a: f64, b: f64| {
            let d = (a - b).abs() % 180.0;
            d.min(180.0 - d)
        };
        // Flow first; then dimension, in sewing order.
        let mut want: Vec<(f64, &'static str, Option<usize>)> = fills
            .iter()
            .map(|f| match f.4 {
                Some(a) => (a, "flow", None),
                None => (f.3, "", None),
            })
            .collect();
        let touch = |a: &zigzag::geom::Bounds, b: &zigzag::geom::Bounds| {
            a.min.x <= b.max.x + 5.0
                && b.min.x <= a.max.x + 5.0
                && a.min.y <= b.max.y + 5.0
                && b.min.y <= a.max.y + 5.0
        };
        for x in 0..fills.len() {
            for y in x + 1..fills.len() {
                if !touch(&fills[x].2, &fills[y].2) || apart(want[x].0, want[y].0) >= 20.0 {
                    continue;
                }
                // The one without a length of its own turns; else the smaller.
                let (turn, keep) = match (fills[x].4.is_some(), fills[y].4.is_some()) {
                    (true, true) => continue,
                    (true, false) => (y, x),
                    (false, true) => (x, y),
                    (false, false) if fills[x].1 < fills[y].1 => (x, y),
                    _ => (y, x),
                };
                want[turn] = (
                    crate::widgets::normalize_angle(want[keep].0 + 45.0),
                    "dimension",
                    Some(fills[keep].0),
                );
            }
        }
        let angles: Vec<Value> = fills
            .iter()
            .zip(&want)
            .filter(|(f, w)| apart(f.3, w.0) >= 15.0)
            .map(|(f, &(a, why, by))| {
                let why = match (why, by) {
                    ("flow", _) => "flow: rows along its length, as stitches follow a form".to_string(),
                    (_, Some(j)) => format!("dimension: set apart from {}, so each catches the light its own way", name(j)),
                    _ => String::new(),
                };
                json!({ "object": name(f.0), "angle_now": f.3.round(), "suggest": a.round(), "why": why })
            })
            .take(30)
            .collect();
        types.truncate(20);
        (types, angles)
    }

    /// What an embroiderer checks first, before looks or cost: underlay
    /// under what needs it, rows close enough to cover the cloth, and fills
    /// that don't all pull the same way. (underlay_off, too_open,
    /// same_direction, and the angle most fills share when most do.)
    fn sewing_basics(&self) -> (Vec<Value>, Vec<Value>, Vec<Value>, Option<(f64, usize, usize)>) {
        let shapes = &self.doc.shapes;
        let f = self.doc.settings.fabric();
        let d = &self.design;
        let sewn = |i: usize| d.per_shape.get(i).copied().unwrap_or(0) > 0;
        let name = |i: usize| self.names.get(i).cloned().unwrap_or_default();
        let apart = |a: f64, b: f64| {
            let d = (a - b).abs() % 180.0;
            d.min(180.0 - d)
        };
        let mut place = vec![usize::MAX; shapes.len()];
        for (n, &i) in d.sew_order.iter().enumerate() {
            if let Some(p) = place.get_mut(i) {
                *p = n;
            }
        }
        let bounds: Vec<zigzag::geom::Bounds> = shapes.iter().map(shape_bounds).collect();
        let is_fill = |i: usize| {
            sewn(i)
                && matches!(shapes[i].geometry, Geometry::Fill { .. })
                && shapes[i].params.kind.unwrap_or(Kind::Fill) == Kind::Fill
        };
        let fills: Vec<usize> = (0..shapes.len()).filter(|&i| is_fill(i)).collect();
        // The fill sewn before `i` whose outline holds it, nearest first.
        let under = |i: usize| {
            let b = bounds[i];
            fills
                .iter()
                .copied()
                .filter(|&j| {
                    j != i && place[j] < place[i] && {
                        let u = bounds[j];
                        u.min.x <= b.min.x && u.min.y <= b.min.y && u.max.x >= b.max.x && u.max.y >= b.max.y
                    }
                })
                .max_by_key(|&j| place[j])
        };
        // A ground: a fill three or more others are sewn on. It may be
        // sewn lighter (a patch's background is), but not without underlay.
        let mut on = vec![0usize; shapes.len()];
        for &i in &fills {
            if let Some(j) = under(i) {
                on[j] += 1;
            }
        }
        let (mut underlay_off, mut too_open) = (Vec::new(), Vec::new());
        for i in (0..shapes.len()).filter(|&i| sewn(i)) {
            let s = &shapes[i];
            let p = &s.params;
            // (Its narrower side, mm: a fleck under 3 mm can go without.)
            let small = bounds[i].width().min(bounds[i].height()) / 10.0;
            match p.kind.unwrap_or(Kind::Fill) {
                Kind::Fill if is_fill(i) => {
                    if !p.underlay_on() && small >= 3.0 {
                        underlay_off.push(json!({
                            "object": name(i), "kind": "fill", "fix": { "underlay": true },
                            "why": "a fill with no underlay sinks into the cloth, puckers it and shows it between its rows",
                        }));
                    }
                    let (s_now, s_std) = (p.fill_spacing(&f), f.fill_spacing);
                    // (Lighter only where the cloth between its rows is close
                    // to its thread: navy rows on cream read as stripes.)
                    let blends = zigzag::formats::perceptual_distance(s.color, self.doc.settings.cloth) < 20.0;
                    let light = if on[i] >= 3 && blends { 2.0 } else { 1.5 };
                    if s_now > s_std * light {
                        too_open.push(json!({
                            "object": name(i), "kind": "fill", "spacing_mm": mm(s_now), "usual_mm": mm(s_std),
                            "fix": { "spacing_mm": null },
                            "why": "rows this far apart leave the cloth showing between them: it reads as stripes, not filled in",
                        }));
                    }
                }
                Kind::Satin => {
                    let w = crate::app::mean_satin_width(s);
                    if w >= f.satin_underlay_from.max(1.5) && p.satin_underlay_for(&f, w) == 0 {
                        underlay_off.push(json!({
                            "object": name(i), "kind": "satin", "width_mm": mm(w),
                            "fix": { "underlay": true, "satin_underlay": "auto" },
                            "why": "a satin this wide with no underlay sinks, and its edges pull in ragged",
                        }));
                    }
                    let (s_now, s_std) = (p.satin_spacing(&f), f.satin_spacing);
                    if s_now > s_std * 1.3 {
                        too_open.push(json!({
                            "object": name(i), "kind": "satin", "spacing_mm": mm(s_now), "usual_mm": mm(s_std),
                            "fix": { "spacing_mm": null },
                            "why": "stitches this far apart leave the cloth showing through the column",
                        }));
                    }
                }
                _ => {}
            }
        }
        // Fills sewn on (or cut into) a fill sewn before them, at its
        // angle: their rows run on from its rows, so the pull of both adds
        // up along one line and the edge between them doesn't read.
        let angle = |i: usize| crate::widgets::normalize_angle(shapes[i].params.fill_angle());
        let mut same_direction = Vec::new();
        for &i in &fills {
            if let Some(j) = under(i)
                && apart(angle(i), angle(j)) < 20.0
            {
                // Along its length where that's well off the ground's;
                // else 45° off it, turning one way and the next the other,
                // so the pieces on it don't all end up parallel again.
                let along = match &shapes[i].geometry {
                    Geometry::Fill { rings, .. } => principal_axis(rings)
                        .filter(|&(a, elong)| elong >= 1.6 && apart(a, angle(j)) >= 30.0)
                        .map(|(a, _)| a),
                    _ => None,
                };
                let turn = if same_direction.len() % 2 == 0 { 45.0 } else { -45.0 };
                let suggest = along.unwrap_or(angle(j) + turn);
                same_direction.push(json!({
                    "object": name(i), "angle_now": angle(i).round(), "sits_on": name(j), "its_angle": angle(j).round(),
                    "suggest": crate::widgets::normalize_angle(suggest).round(),
                    "why": "its rows run the same way as the fill it sits on, so their pull adds up and the edge between them doesn't read: turn it at least 30° from it, along its length where it has one",
                }));
            }
        }
        // Most of the fills at one angle (usually the 0° they start at).
        let mut common = None;
        if fills.len() >= 5 {
            let best = fills
                .iter()
                .map(|&i| {
                    let a = angle(i);
                    (a, fills.iter().filter(|&&j| apart(a, angle(j)) < 10.0).count())
                })
                .max_by_key(|x| x.1);
            if let Some((a, n)) = best
                && n * 10 >= fills.len() * 6
            {
                common = Some((a.round(), n, fills.len()));
            }
        }
        same_direction.truncate(30);
        underlay_off.truncate(30);
        too_open.truncate(30);
        (underlay_off, too_open, same_direction, common)
    }

    /// The rest of an embroiderer's checklist: shapes sewn only as an
    /// outline with nothing filling them, a patch border that isn't sewn
    /// last (or is too narrow to hold the edge), lettering too small to
    /// read in thread, and satin underlay set by hand that doesn't suit
    /// the column's width.
    fn finishing_checks(&self) -> (Vec<Value>, Option<Value>, Vec<Value>, Vec<Value>, Vec<Value>, Vec<Value>) {
        let shapes = &self.doc.shapes;
        let d = &self.design;
        let f = self.doc.settings.fabric();
        let sewn = |i: usize| d.per_shape.get(i).copied().unwrap_or(0) > 0;
        let name = |i: usize| self.names.get(i).cloned().unwrap_or_default();
        let bounds: Vec<zigzag::geom::Bounds> = shapes.iter().map(shape_bounds).collect();
        let area = |b: &zigzag::geom::Bounds| b.width().max(0.0) * b.height().max(0.0);
        let live: Vec<usize> = (0..shapes.len()).filter(|&i| sewn(i)).collect();
        let text = |i: usize| self.doc.lettering(i).is_some() || self.doc.records_text(i);
        let kind = |i: usize| shapes[i].params.kind.unwrap_or(Kind::Fill);
        // Sewn as a solid shape: a fill, or a satin across the shape.
        let solid = |i: usize| {
            matches!(shapes[i].geometry, Geometry::Fill { .. })
                && !matches!(kind(i), Kind::Run | Kind::Skip | Kind::Manual)
                && !(kind(i) == Kind::Satin && shapes[i].params.is_border())
        };
        // Sewn only round its edge.
        let edge_only = |i: usize| match &shapes[i].geometry {
            Geometry::Fill { .. } => kind(i) == Kind::Run || (kind(i) == Kind::Satin && shapes[i].params.is_border()),
            Geometry::Stroke { lines, .. } => !shapes[i].outline && lines.iter().any(|(_, closed)| *closed),
        };

        let mut outline_only = Vec::new();
        for &i in &live {
            let b = bounds[i];
            let small = b.width().min(b.height()) / 10.0;
            if small < 4.0 || !edge_only(i) || text(i) {
                continue;
            }
            // Something solid about its size over most of it: it's filled.
            let filled = live.iter().any(|&j| {
                j != i && solid(j) && area(&bounds[j]) <= 4.0 * area(&b) && {
                    let u = bounds[j];
                    let w = (u.max.x.min(b.max.x) - u.min.x.max(b.min.x)).max(0.0);
                    let h = (u.max.y.min(b.max.y) - u.min.y.max(b.min.y)).max(0.0);
                    w * h >= 0.5 * area(&b)
                }
            });
            if !filled {
                outline_only.push(json!({
                    "object": name(i), "kind": kind_name(kind(i)), "size_mm": mm(small),
                    "why": "sewn only round its edge with nothing filling it, so it reads as an empty outline rather than a shape: if it's meant to be solid, sew it as a fill (set_object kind fill, or a filled shape under it); a ring, frame or line drawing meant as an outline is fine",
                }));
            }
        }
        outline_only.truncate(20);

        // A patch border: a satin round the edge that holds everything else.
        let border = live
            .iter()
            .copied()
            .filter(|&i| kind(i) == Kind::Satin && edge_only(i))
            .filter(|&i| {
                let b = bounds[i];
                let others: Vec<usize> = live.iter().copied().filter(|&j| j != i).collect();
                let inside = others
                    .iter()
                    .filter(|&&j| {
                        let u = bounds[j];
                        u.min.x >= b.min.x - 10.0
                            && u.min.y >= b.min.y - 10.0
                            && u.max.x <= b.max.x + 10.0
                            && u.max.y <= b.max.y + 10.0
                    })
                    .count();
                others.len() >= 3 && inside * 10 >= others.len() * 9
            })
            .max_by(|&a, &b| area(&bounds[a]).total_cmp(&area(&bounds[b])))
            .and_then(|i| {
                let last = d.sew_order.iter().rev().copied().find(|&j| sewn(j));
                let width = crate::app::mean_satin_width(&shapes[i]);
                let mut why = Vec::new();
                if last != Some(i) {
                    why.push("it isn't sewn last: a border sewn early perforates the backing, and what's sewn after it pulls the patch out of shape inside it. Move it to the end of the sewing order (move_object to the last place)");
                }
                if width < 2.5 {
                    why.push("it's narrower than 2.5 mm: if this is a patch to be cut out, its edge needs 3 to 5 mm of satin to cover the cut and hold it (as a frame sewn on a garment it's fine)");
                }
                (!why.is_empty()).then(|| {
                    json!({ "object": name(i), "width_mm": mm(width), "sewn_last": last == Some(i), "why": why.join("; ") })
                })
            });

        // Lettering: its tallest letter, against what reads in thread.
        let least = if self.doc.settings.hat_order { 6.5 } else { 5.0 };
        let mut small_lettering = Vec::new();
        for &i in live.iter().filter(|&&i| text(i)) {
            let Geometry::Fill { rings, rule } = &shapes[i].geometry else {
                continue;
            };
            let tallest = zigzag::stitch::satin::components(rings, *rule)
                .iter()
                .filter_map(|c| c.first())
                .map(|r| {
                    let (lo, hi) = r.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.y), hi.max(p.y)));
                    (hi - lo) / 10.0
                })
                .fold(0.0, f64::max);
            if tallest > 0.0 && tallest < least {
                small_lettering.push(json!({
                    "object": name(i), "tallest_letter_mm": mm(tallest), "least_mm": least,
                    "why": format!("letters under {least} mm tall{} close up in thread and won't read: make it bigger, or say fewer words larger", if least > 5.0 { " on a cap" } else { "" }),
                }));
            }
        }

        // Satin underlay set by hand that doesn't suit the column.
        let mut underlay_mismatch = Vec::new();
        for &i in live.iter().filter(|&&i| kind(i) == Kind::Satin) {
            let p = &shapes[i].params;
            let Some(m) = p.satin_underlay else { continue };
            if !p.underlay_on() || m == 0 {
                continue;
            }
            let w = crate::app::mean_satin_width(&shapes[i]);
            let heavy = underlay::CONTOUR | underlay::ZIGZAG;
            let why = if w >= 5.0 && m & heavy == 0 {
                "a center walk alone can't hold a column this wide: it needs an edge walk and zigzag under it"
            } else if w < f.satin_underlay_from.max(1.5) && m & heavy != 0 {
                "an edge walk or zigzag under a column this narrow piles thread and pokes out of its sides"
            } else {
                continue;
            };
            underlay_mismatch.push(json!({
                "object": name(i), "width_mm": mm(w), "fix": { "satin_underlay": "auto" }, "why": why,
            }));
        }
        underlay_mismatch.truncate(20);

        // Grounds: fills that three or more others are sewn on.
        let mut place = vec![usize::MAX; shapes.len()];
        for (n, &i) in d.sew_order.iter().enumerate() {
            if let Some(p) = place.get_mut(i) {
                *p = n;
            }
        }
        let holds = |g: usize, i: usize| {
            let (u, b) = (bounds[g], bounds[i]);
            u.min.x <= b.min.x && u.min.y <= b.min.y && u.max.x >= b.max.x && u.max.y >= b.max.y
        };
        let grounds: Vec<usize> = live
            .iter()
            .copied()
            .filter(|&g| {
                solid(g)
                    && kind(g) == Kind::Fill
                    && live.iter().filter(|&&i| i != g && solid(i) && place[g] < place[i] && holds(g, i)).count() >= 3
            })
            .collect();
        let inside = |j: usize, p: P| match &shapes[j].geometry {
            Geometry::Fill { rings, rule } => zigzag::geom::point_inside(rings, *rule, p),
            Geometry::Stroke { .. } => false,
        };
        // The lines a running stitch follows.
        let run_lines = |i: usize| -> Vec<Vec<P>> {
            match &shapes[i].geometry {
                Geometry::Fill { rings, .. } => rings.iter().map(|r| {
                    let mut r = r.clone();
                    if let Some(&first) = r.first() {
                        r.push(first);
                    }
                    r
                }).collect(),
                Geometry::Stroke { lines, .. } => lines.iter().map(|(l, _)| l.clone()).collect(),
            }
        };
        let is_run = |i: usize| {
            !shapes[i].outline
                && !text(i)
                && match &shapes[i].geometry {
                    Geometry::Fill { .. } => kind(i) == Kind::Run,
                    Geometry::Stroke { .. } => matches!(kind(i), Kind::Run) || shapes[i].params.kind.is_none(),
                }
        };
        let flagged: Vec<String> = outline_only.iter().filter_map(|x| x["object"].as_str().map(str::to_string)).collect();

        // Lines sewn across a ground, over nothing else: what's not part of
        // the picture there is clutter on the background.
        let mut lines_on_ground = Vec::new();
        if !grounds.is_empty() {
            for &i in live.iter().filter(|&&i| is_run(i) && !flagged.contains(&name(i))) {
                let pts: Vec<P> = run_lines(i)
                    .iter()
                    .flat_map(|l| zigzag::geom::sample_by_arclength(l, 10.0).into_iter().map(|(p, _)| p))
                    .collect();
                let length = run_lines(i).iter().map(|l| zigzag::geom::polyline_length(l)).sum::<f64>() / 10.0;
                if pts.len() < 4 || length < 10.0 {
                    continue;
                }
                let bare = pts
                    .iter()
                    .filter(|&&p| {
                        grounds.iter().any(|&g| g != i && inside(g, p))
                            && !live.iter().any(|&j| j != i && !grounds.contains(&j) && solid(j) && inside(j, p))
                    })
                    .count();
                if bare * 2 >= pts.len() {
                    lines_on_ground.push(json!({
                        "object": name(i), "length_mm": mm(length),
                        "why": "a line sewn across the ground over nothing else: unless it's part of the picture (a road, a horizon, a motion line you meant), it's clutter on the background, and its thread lies on top of the ground's rows",
                    }));
                }
            }
        }
        lines_on_ground.truncate(20);

        // Small filled shapes sewn only as their outline, inside a shape of
        // another color: what tracing leaves behind (a speck, a stray
        // highlight), sewn as a little loop of running stitch.
        let mut stray_loops = Vec::new();
        for &i in live.iter().filter(|&&i| matches!(shapes[i].geometry, Geometry::Fill { .. }) && is_run(i)) {
            let b = bounds[i];
            let big = b.width().max(b.height()) / 10.0;
            if big >= 8.0 {
                continue;
            }
            let c = P::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0);
            // On another color's shape, or in a hole the trace left in one
            // (its outline all round it).
            let other = |j: usize| j != i && solid(j) && !grounds.contains(&j) && shapes[j].color != shapes[i].color;
            let on = live
                .iter()
                .copied()
                .find(|&j| other(j) && inside(j, c))
                .or_else(|| live.iter().copied().find(|&j| other(j) && holds(j, i)));
            if let Some(j) = on {
                stray_loops.push(json!({
                    "object": name(i), "size_mm": mm(big), "inside": name(j),
                    "why": "a small filled shape sewn only as a loop of running stitch inside another color: usually a speck the trace left. Delete it, or if it's a detail that matters (an eye, a highlight), sew it as fill (set_object kind fill) when it's at least 1.5 mm",
                }));
            }
        }
        stray_loops.truncate(20);
        (outline_only, border, small_lettering, underlay_mismatch, lines_on_ground, stray_loops)
    }

    /// The gaps sewn across (the document's Connect) whose thread shows on
    /// what it lies over: another color, or cloth it stands out on, with
    /// nothing sewn later to cover it. (A gap across its own color, or one
    /// a later object covers, is out of sight, and isn't counted.)
    fn bridges_in_view(&self) -> usize {
        let d = &self.design;
        let shapes = &self.doc.shapes;
        let cloth = self.doc.settings.cloth;
        let mut place = vec![usize::MAX; shapes.len()];
        for (n, &i) in d.sew_order.iter().enumerate() {
            if let Some(p) = place.get_mut(i) {
                *p = n;
            }
        }
        let covers: Vec<usize> = (0..shapes.len())
            .filter(|&i| {
                d.per_shape.get(i).copied().unwrap_or(0) > 0
                    && matches!(shapes[i].geometry, Geometry::Fill { .. })
                    && !matches!(shapes[i].params.kind.unwrap_or(Kind::Fill), Kind::Run | Kind::Skip | Kind::Manual)
                    && !shapes[i].params.is_border()
            })
            .collect();
        let inside = |j: usize, q: P| match &shapes[j].geometry {
            Geometry::Fill { rings, rule } => zigzag::geom::point_inside(rings, *rule, q),
            Geometry::Stroke { .. } => false,
        };
        let mut shown = 0;
        for &(a, b) in &d.bridges {
            // The stitch it is: its thread, and the object it's sewn for.
            let mut found = None;
            'find: for (bi, blk) in d.blocks.iter().enumerate() {
                for (ri, run) in blk.runs.iter().enumerate() {
                    if let Some(k) = run.pts.windows(2).position(|w| w[0] == a && w[1] == b) {
                        let shape = d
                            .stitch_shapes
                            .get(bi)
                            .and_then(|x| x.get(ri))
                            .and_then(|x| x.iter().filter(|(pos, _)| *pos <= k + 1).next_back())
                            .map(|&(_, s)| s);
                        found = Some((blk.color, shape));
                        break 'find;
                    }
                }
            }
            let Some((thread, shape)) = found else {
                shown += 1;
                continue;
            };
            let at = shape.map_or(usize::MAX, |s| place[s]);
            let n = (a.dist(b) / 5.0).ceil().max(1.0) as usize;
            let showing = (0..n)
                .filter(|&k| {
                    let q = a.lerp(b, (k as f64 + 0.5) / n as f64);
                    if covers.iter().any(|&j| place[j] > at && place[j] != usize::MAX && inside(j, q)) {
                        return false;
                    }
                    let under = covers
                        .iter()
                        .copied()
                        .filter(|&j| place[j] <= at && inside(j, q))
                        .max_by_key(|&j| place[j])
                        .map_or(cloth, |j| shapes[j].color);
                    zigzag::design::contrast(thread, under) > 0.5
                })
                .count();
            // (A millimetre of it in sight is a line you see.)
            if showing as f64 * a.dist(b) / n as f64 >= 10.0 {
                shown += 1;
            }
        }
        shown
    }

    /// Settings that would cut trims, each tried on a copy of the design
    /// (planned side by side): those that save any, with what they come to.
    fn trim_what_ifs(&self, trims: usize) -> Vec<Value> {
        // (Planning the design over again for each try is the cost of a
        // review: once for a design as it stands, however often it's asked.)
        let key = (self.session.rec, self.doc.rev(), trims);
        if let Some((k, v)) = &*self.what_ifs.borrow()
            && *k == key
        {
            return v.clone();
        }
        let v = self.trim_what_ifs_now(trims);
        *self.what_ifs.borrow_mut() = Some((key, v.clone()));
        v
    }

    fn trim_what_ifs_now(&self, trims: usize) -> Vec<Value> {
        let s = &self.doc.settings;
        let mut tries: Vec<(&str, Option<bool>, Option<f64>, Option<f64>)> = Vec::new();
        // (Across what the thread blends into first: it's the one that
        // doesn't show.)
        for (label, b) in [("Blend 6 mm", 6.0), ("Blend 12 mm", 12.0)] {
            if s.blend_mm < b {
                tries.push((label, None, None, Some(b)));
            }
        }
        if !s.over_sewn {
            tries.push(("Sew over fill", Some(true), None, None));
        }
        for c in [2.0, 3.0] {
            if s.connect_mm < c {
                tries.push((
                    if c == 2.0 {
                        "Connect 2 mm"
                    } else {
                        "Connect 3 mm"
                    },
                    None,
                    Some(c),
                    None,
                ));
            }
        }
        if !s.over_sewn && s.connect_mm < 3.0 {
            tries.push((
                "Sew over fill and Connect 3 mm",
                Some(true),
                Some(3.0),
                None,
            ));
        }
        if tries.is_empty() || trims == 0 {
            return Vec::new();
        }
        // The thread in plain sight now: travel lying on finished
        // stitching, and gaps sewn across bare fabric. A setting "shows"
        // when it adds to that.
        let in_view = |d: &zigzag::design::Design| {
            zigzag::checks::exposed_travel(d)
                .iter()
                .map(|e| e.mm)
                .sum::<f64>()
                + d.bridges
                    .iter()
                    .map(|(a, b)| a.dist(*b) / 10.0)
                    .sum::<f64>()
                + d.blends
                    .iter()
                    .flat_map(|w| w.windows(2))
                    .map(|w| w[0].dist(w[1]) / 10.0)
                    .sum::<f64>()
        };
        let now = in_view(&self.design);
        let in_view = &in_view;
        // And drawn, framed on the design: of that thread, what lies on a
        // color not its own (a line that shows) rather than on its own
        // (texture), from where the drawing changes color.
        let frame = {
            let b = self.design.bounds();
            zigzag::geom::Bounds {
                min: b.min - P::new(20.0, 20.0),
                max: b.max + P::new(20.0, 20.0),
            }
        };
        let opt = zigzag::formats::preview::PreviewOptions {
            hoop_mm: None,
            background: self.doc.settings.cloth,
            show_jumps: false,
            thread_mm: THREAD_MM,
            marks: Vec::new(),
            region: Some(frame),
            art: None,
        };
        // (As the picture is really drawn: a tall one is held to 4096 px high.)
        let px_per_mm = (LOOK_PX as f64 / (frame.width() / 10.0).max(1.0))
            .min(4096.0 / (frame.height() / 10.0).max(1.0));
        let drawn = zigzag::formats::preview::render_rgba(&self.design, &opt, LOOK_PX).ok();
        let (drawn, opt) = (&drawn, &opt);
        let doc = &self.doc;
        let planned: Vec<Option<(usize, usize, f64, Option<f64>)>> = std::thread::scope(|sc| {
            let runs: Vec<_> = tries
                .iter()
                .map(|&(_, over, connect, blend)| {
                    sc.spawn(move || {
                        let mut d = doc.clone();
                        if let Some(b) = blend {
                            d.settings.blend_mm = b;
                        }
                        if let Some(o) = over {
                            d.settings.over_sewn = o;
                        }
                        if let Some(c) = connect {
                            d.settings.connect_mm = c;
                        }
                        let p = zigzag::design::caught(|| d.plan()).ok()?;
                        let contrast = drawn.as_ref().and_then(|now| {
                            let then =
                                zigzag::formats::preview::render_rgba(&p, opt, LOOK_PX).ok()?;
                            Some(
                                changed_px(now, &then) as f64 / (THREAD_MM * px_per_mm * px_per_mm),
                            )
                        });
                        Some((
                            p.trim_count(),
                            p.bridges.len(),
                            (in_view(&p) - now).max(0.0),
                            contrast,
                        ))
                    })
                })
                .collect();
            runs.into_iter().map(|r| r.join().ok().flatten()).collect()
        });
        tries
            .iter()
            .zip(planned)
            .filter_map(|(&(change, over, connect, blend), got)| {
                let (n, gaps, shown, contrast) = got?;
                // (Not known to lie on its own color is taken to show.)
                let line = contrast.unwrap_or(shown).min(shown);
                (n < trims).then(|| {
                    let mut set = serde_json::Map::new();
                    if let Some(o) = over {
                        set.insert("over_sewn".into(), json!(o));
                    }
                    if let Some(c) = connect {
                        set.insert("connect_mm".into(), json!(c));
                    }
                    if let Some(b) = blend {
                        set.insert("blend_mm".into(), json!(b));
                    }
                    json!({
                        "change": change, "set_document": set, "trims": n, "saves": trims - n,
                        "gaps_sewn_in_view": gaps,
                        "thread_in_view_mm": mm(shown),
                        "on_another_color_mm": mm(line),
                        "shows": line > SHOWS_MM,
                    })
                })
            })
            .collect()
    }

    fn tool_render(&mut self, args: &Value) -> Result<ToolResult, String> {
        let width = args
            .get("width_px")
            .and_then(Value::as_u64)
            .unwrap_or(900)
            .clamp(200, 2000) as u32;
        let show_jumps = args
            .get("show_jumps")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let fit_hoop = match args.get("fit") {
            None => false,
            Some(v) => match v.as_str() {
                Some("design") => false,
                Some("hoop") => true,
                _ => return Err("fit must be design or hoop".into()),
            },
        };
        let artwork = match args.get("artwork") {
            None => false,
            Some(v) => v.as_bool().ok_or("`artwork` must be true or false")?,
        };
        let region = self.preview_region(args)?;
        let art = artwork.then(|| self.artwork());
        let opt = PreviewOptions {
            hoop_mm: fit_hoop.then(|| self.hoop_dims()),
            background: self.doc.settings.cloth,
            show_jumps,
            thread_mm: 0.38,
            marks: self.preview_marks(),
            region,
            art,
        };
        let png = render_png(&self.design, &opt, width).map_err(|e| format!("{e:#}"))?;
        let b = self.design.bounds();
        let legend: Vec<String> = self
            .design
            .sew_order
            .iter()
            .filter(|&&i| self.design.per_shape.get(i).copied().unwrap_or(0) > 0)
            .map(|&i| {
                format!(
                    "{} ({}, {})",
                    self.names[i],
                    kind_name(self.doc.shapes[i].params.kind.unwrap_or(Kind::Fill)),
                    hex(self.doc.shapes[i].color)
                )
            })
            .collect();
        let mut text = format!(
            "{:.1} × {:.1} mm, {} stitches, {} colors. Sewing order: {}.",
            b.width() / 10.0,
            b.height() / 10.0,
            self.design.stitch_count(),
            self.design.blocks.len(),
            legend.join(" → ")
        );
        if artwork {
            text.push_str(" Showing the artwork as drawn, not the stitches.");
        }
        if let Some(r) = region {
            text.push_str(&format!(
                " Close-up: {:.1} × {:.1} mm around ({:.1}, {:.1}).",
                r.width() / 10.0,
                r.height() / 10.0,
                r.center().x / 10.0,
                r.center().y / 10.0
            ));
        }
        Ok(ToolResult {
            content: vec![Content::Image(png), Content::Text(text)],
            is_error: false,
        })
    }

    pub fn fit_view(&mut self) {
        self.request_fit();
    }

    /// render_preview's close-up, from `region` or `object` (design units).
    fn preview_region(&mut self, args: &Value) -> Result<Option<Bounds>, String> {
        let region = args.get("region").filter(|v| !v.is_null());
        let object = args.get("object").filter(|v| !v.is_null());
        match (region, object) {
            (Some(_), Some(_)) => Err("pass region or object, not both".into()),
            (Some(r), None) => {
                let num = |k: &str| {
                    r.get(k)
                        .and_then(Value::as_f64)
                        .filter(|v| v.is_finite())
                        .ok_or(format!("region.{k} must be a number"))
                };
                let (x, y) = (num("x_mm")?, num("y_mm")?);
                let (w, h) = (num("width_mm")?, num("height_mm")?);
                if w < 1.0 || h < 1.0 {
                    return Err("a region must be at least 1 mm wide and high".into());
                }
                Ok(Some(Bounds {
                    min: P::new((x - w / 2.0) * 10.0, (y - h / 2.0) * 10.0),
                    max: P::new((x + w / 2.0) * 10.0, (y + h / 2.0) * 10.0),
                }))
            }
            (None, Some(o)) => {
                let i = self.resolve(Some(o))?;
                let b = shape_bounds(&self.doc.shapes[i]);
                if b.is_empty() {
                    return Err(format!("{} has nothing to show", self.names[i]));
                }
                // Room around it: a tenth of its size, at least 2 mm, and
                // never less than 5 mm across in all.
                let c = b.center();
                let half = |s: f64| (s * 0.6).max(s / 2.0 + 20.0).max(25.0);
                let (hw, hh) = (half(b.width()), half(b.height()));
                Ok(Some(Bounds {
                    min: P::new(c.x - hw, c.y - hh),
                    max: P::new(c.x + hw, c.y + hh),
                }))
            }
            (None, None) => Ok(None),
        }
    }

    /// The document's shapes as drawn, for render_preview's artwork view:
    /// what's hidden or not sewn left out.
    fn artwork(&self) -> Vec<Art> {
        self.doc
            .shapes
            .iter()
            .filter(|s| !s.params.hidden() && s.params.kind != Some(Kind::Skip))
            .map(|s| match &s.geometry {
                Geometry::Fill { rings, rule } => Art::Fill {
                    rings: rings.clone(),
                    rule: *rule,
                    color: s.color,
                },
                Geometry::Stroke { lines, width } => Art::Stroke {
                    lines: lines.clone(),
                    width: *width,
                    color: s.color,
                },
            })
            .collect()
    }
}

impl Studio {
    fn tool_send_to_skitch(&mut self) -> Result<ToolResult, String> {
        // (A document set up for a machine that takes a file isn't sent.)
        if !self.sends_direct() {
            return Err(format!(
                "This document is set up for {}, which ZigZag doesn't send to: export a file for it instead (or change the document's machine with set_document).",
                self.machine_label()
                    .or_else(|| self
                        .machines
                        .current
                        .as_deref()
                        .and_then(|id| self.machines.find(id))
                        .map(|p| p.name.clone()))
                    .unwrap_or("a machine".into())
            ));
        }
        let s = self.machine.snapshot();
        self.machine.open = true;
        if s.link != crate::machine::Link::Connected {
            return Err("The Skitch isn't connected. Ask the person to click Connect in the Skitch window (it's open now), then try again.".into());
        }
        if self.design.blocks.is_empty() {
            return Err("There's nothing to sew yet.".into());
        }
        if self.design_over_hoop() {
            return Err("The design is bigger than the hoop.".into());
        }
        let ctx = self.egui_ctx.clone().unwrap_or_default();
        self.send_design(&ctx);
        Ok(ToolResult::text(
            "Sending. The Skitch window now walks the person through tracing the outline and sewing; call machine_status to follow along.",
        ))
    }

    /// The machine's state for agents.
    pub(crate) fn machine_json(&self) -> Value {
        use zigzag::machine::protocol;
        let s = self.machine.snapshot();
        let ours = self.machine.ours(&s);
        let step = crate::sew::step(&s, ours.is_some());
        let link = match &s.link {
            crate::machine::Link::Off => "not connected".to_string(),
            crate::machine::Link::Searching => "connecting".into(),
            crate::machine::Link::NotFound => "no machine found".into(),
            crate::machine::Link::NotPaired => "not paired with this computer".into(),
            crate::machine::Link::Failed(e) => format!("failed: {e}"),
            crate::machine::Link::Connected => "connected".into(),
            crate::machine::Link::Lost(e) => format!("connection lost: {e}"),
        };
        let error = protocol::error_help(s.error)
            .map(|h| json!({ "code": s.error, "title": h.title, "what_to_do": h.steps }));
        let mut v = json!({
            "connection": link,
            "simulated": s.sim,
            "step": crate::sew::step_name(step),
            "status": s.status.map(|st| st.label()),
            "error": error,
            "model": s.info.as_ref().map(|i| i.model.trim().to_string()),
            "has_our_design": ours.is_some(),
        });
        if let Some(sent) = ours {
            let total = sent.pen.stitches;
            let at = s.progress.map(|p| p.current_stitch as usize).unwrap_or(0);
            let threads: Vec<Value> = sent
                .pen
                .blocks
                .iter()
                .map(|b| {
                    let c = sent
                        .design
                        .blocks
                        .get(b.block)
                        .map(|k| k.color)
                        .unwrap_or([0; 3]);
                    json!({
                        "color": format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]),
                        "stitches": b.end - b.start,
                        "done": at >= b.end,
                    })
                })
                .collect();
            v["progress"] = json!({
                "stitch": at,
                "of": total,
                "thread": sent
                    .pen
                    .thread_at(at, s.status == Some(zigzag::machine::protocol::Status::WaitingForColorChange))
                    .map(|k| k + 1),
                "threads": threads,
            });
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terse_steps_one_added_object_is_named_by_dollar_name() {
        // (A terse step lists what it added by name alone; `made` too.)
        let terse = [json!({ "ok": true, "added": ["leaf"] })];
        assert_eq!(resolve_refs(&json!("$0.name"), &terse).unwrap(), "leaf");
        let made = [json!({ "ok": true, "made": [{ "name": "stem" }] })];
        assert_eq!(resolve_refs(&json!("$0.name"), &made).unwrap(), "stem");
        // And the hint for a miss names a reference that works.
        let two = [json!({ "ok": true, "added": ["a", "b"] })];
        let e = resolve_refs(&json!("$0.name"), &two).unwrap_err();
        assert!(e.contains("\"$0.added.0\"") && !e.contains("added.0.name"), "{e}");
    }

    #[test]
    fn a_missing_name_lists_a_dozen_objects_not_hundreds() {
        let few: Vec<String> = ["Face", "Mouth"].map(String::from).to_vec();
        assert_eq!(no_object_named("Hat", &few), "no object named 'Hat'. Objects: Face, Mouth");
        let many: Vec<String> = (0..80).map(|i| format!("trace {i}")).collect();
        let why = no_object_named("Illustrated blue ground", &many);
        assert!(why.contains("trace 11, and 68 more (get_design lists them all)"), "{why}");
        assert!(!why.contains("trace 12"), "{why}");
    }
    use crate::app::Studio;
    use std::io::{Read, Write};

    const BADGE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
        <circle id="patch" cx="50" cy="50" r="48" fill="#1d3557"/>
        <path id="ring" d="M50,8 a42,42 0 1,0 0.01,0 Z M50,14 a36,36 0 1,0 0.01,0 Z" fill="#f1faee" fill-rule="evenodd"/>
        <rect id="bar" x="20" y="46" width="60" height="8" fill="#e63946"/>
        <path id="stitchline" d="M25,70 L75,70" stroke="#f1faee" stroke-width="1" fill="none"/>
    </svg>"##;

    fn text(r: &ToolResult) -> String {
        r.content
            .iter()
            .filter_map(|c| match c {
                Content::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn ok(r: ToolResult) -> Value {
        assert!(!r.is_error, "tool failed: {}", text(&r));
        serde_json::from_str(&text(&r)).unwrap_or(Value::Null)
    }

    #[test]
    fn tool_list_is_well_formed() {
        let tools = tool_specs();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for want in [
            "get_design",
            "list_machines",
            "add_svg",
            "set_object",
            "render_preview",
            "check_design",
            "save",
            "export",
            "undo",
        ] {
            assert!(names.contains(&want), "{want} missing");
        }
        for t in tools.as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            assert!(t["description"].as_str().unwrap().len() > 10);
        }
    }

    #[test]
    fn a_save_that_fails_says_so_even_with_nothing_unsaved() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({ "svg": BADGE, "replace": true })));
        s.modified = false;
        let nowhere = std::env::temp_dir()
            .join(format!("zz-no-such-dir-{}", std::process::id()))
            .join("a.svg");
        let r = s.run_tool("save", &json!({ "path": nowhere }));
        assert!(r.is_error, "{}", text(&r));
    }

    #[test]
    fn text_on_a_path_undoes_with_its_path_sewn_again() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({ "svg": BADGE, "replace": true })));
        let kind = |s: &Studio| {
            let i = s.names.iter().position(|n| n == "stitchline").unwrap();
            s.doc.shapes[i].params.kind
        };
        let before = kind(&s);
        // Text that can't be made leaves the path as it was.
        let r = s.run_tool("add_text", &json!({ "text": " ", "on_path": "stitchline" }));
        assert!(r.is_error);
        assert_eq!(kind(&s), before);
        ok(s.run_tool(
            "add_text",
            &json!({ "text": "Hi", "on_path": "stitchline" }),
        ));
        assert_eq!(kind(&s), Some(Kind::Skip));
        s.run_tool("undo", &json!({}));
        assert_eq!(kind(&s), before, "one step: the text and its path");
    }

    #[test]
    fn get_design_says_what_is_selected() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({ "svg": BADGE, "replace": true })));
        ok(s.run_tool("select", &json!({ "object": "bar" })));
        let d = ok(s.run_tool("get_design", &json!({})));
        assert_eq!(d["document"]["selected"], json!(["bar"]));
    }

    #[test]
    fn objects_are_lined_up_combined_renamed_and_changed_together() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool(
            "add_svg",
            &json!({ "svg": BADGE, "width_mm": 50.0, "replace": true }),
        ));
        let center = |s: &mut Studio, name: &str| {
            let d = ok(s.run_tool("get_design", &json!({})));
            let o = d["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["name"] == name)
                .unwrap_or_else(|| panic!("no {name} in {d}"))
                .clone();
            (
                o["center_mm"][0].as_f64().unwrap(),
                o["center_mm"][1].as_f64().unwrap(),
            )
        };
        // Off to one side, then back to the middle of the hoop.
        ok(s.run_tool(
            "transform_objects",
            &json!({ "objects": "bar", "move_by_mm": [7, 3] }),
        ));
        ok(s.run_tool(
            "align_objects",
            &json!({ "objects": "bar", "align": "center", "to": "hoop" }),
        ));
        ok(s.run_tool(
            "align_objects",
            &json!({ "objects": "bar", "align": "middle", "to": "hoop" }),
        ));
        let (x, y) = center(&mut s, "bar");
        assert!(x.abs() < 0.05 && y.abs() < 0.05, "{x}, {y}");
        // Several changed the same way, as one undo step.
        let undo = s.undo.len();
        let r = s.run_tool(
            "set_object",
            &json!({ "objects": ["patch", "bar"], "color": "#112233" }),
        );
        assert!(!r.is_error, "{}", text(&r));
        assert_eq!(s.undo.len(), undo + 1);
        for n in ["patch", "bar"] {
            let i = s.names.iter().position(|x| x == n).unwrap();
            assert_eq!(s.doc.shapes[i].color, [0x11, 0x22, 0x33]);
        }
        ok(s.run_tool(
            "rename_object",
            &json!({ "object": "bar", "name": "banner" }),
        ));
        assert!(s.names.iter().any(|n| n == "banner"), "{:?}", s.names);
        // Undone and done again.
        s.run_tool("undo", &json!({}));
        assert!(s.names.iter().any(|n| n == "bar"));
        assert_eq!(text(&s.run_tool("redo", &json!({}))), "Redone.");
        assert!(s.names.iter().any(|n| n == "banner"));
        // Two shapes made one.
        let n = s.doc.shapes.len();
        let r = ok(s.run_tool("combine_shapes", &json!({ "objects": ["patch", "banner"] })));
        assert_eq!(r["added"].as_array().unwrap().len(), 1);
        assert_eq!(s.doc.shapes.len(), n - 1);
        // Part of a name doesn't delete; several named in full do.
        assert!(
            s.run_tool("delete_object", &json!({ "object": "stitch" }))
                .is_error
        );
        let r = s.run_tool(
            "delete_object",
            &json!({ "objects": ["ring", "stitchline"] }),
        );
        assert!(!r.is_error, "{}", text(&r));
        assert_eq!(s.doc.shapes.len(), n - 3);
    }

    #[test]
    fn a_closer_look_and_what_a_short_answer_keeps() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool(
            "add_svg",
            &json!({ "svg": BADGE, "width_mm": 50.0, "replace": true }),
        ));
        // A reading tool's answer is never the short one.
        assert!(only_reads("get_path") && only_reads("measure_stitching"));
        // A wide-open stitch keeps its spacing; an end at 1 is the far end.
        ok(s.run_tool(
            "set_object",
            &json!({ "object": "patch", "kind": "meander", "spacing_mm": 3 }),
        ));
        let i = s.names.iter().position(|x| x == "patch").unwrap();
        assert_eq!(s.doc.shapes[i].params.spacing, Some(3.0));
        ok(s.run_tool("set_object", &json!({ "object": "stitchline", "end": 1 })));
        let i = s.names.iter().position(|x| x == "stitchline").unwrap();
        assert!(s.doc.shapes[i].params.end.unwrap() > 0.99);
        // A file under another format's name isn't written.
        let to = std::env::temp_dir().join(format!("zz-{}.dst", std::process::id()));
        assert!(
            s.run_tool("export", &json!({ "format": "pes", "path": to }))
                .is_error
        );
        assert!(!to.exists());
        // A line has no height to resize.
        assert!(
            s.run_tool(
                "transform_objects",
                &json!({ "objects": "stitchline", "height_mm": 2 })
            )
            .is_error
        );
        // And a turn too far is only an angle.
        ok(s.run_tool("set_object", &json!({ "object": "bar", "angle": 1e300 })));
    }

    #[test]
    fn render_preview_zooms_in_and_shows_the_artwork() {
        let mut s = Studio::with_file(None, None);
        // A blue square on the left, a small red dot well off to the right.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="20mm" viewBox="0 0 80 20">
            <rect id="square" x="0" y="0" width="20" height="20" fill="#0000ff"/>
            <circle id="dot" cx="70" cy="10" r="3" fill="#ff0000"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        let dot = ok(s.run_tool("get_design", &json!({})))["objects"][1]["center_mm"].clone();
        let (dx, dy) = (dot[0].as_f64().unwrap(), dot[1].as_f64().unwrap());
        let pixels = |r: &ToolResult| {
            assert!(!r.is_error, "{}", text(r));
            let png = r
                .content
                .iter()
                .find_map(|c| match c {
                    Content::Image(b) => Some(b.clone()),
                    _ => None,
                })
                .unwrap();
            resvg::tiny_skia::Pixmap::decode_png(&png).unwrap()
        };
        let center = |p: &resvg::tiny_skia::Pixmap| {
            let c = p.pixel(p.width() / 2, p.height() / 2).unwrap();
            [c.red(), c.green(), c.blue()]
        };
        let blue = |p: &resvg::tiny_skia::Pixmap| {
            p.pixels()
                .iter()
                .filter(|c| c.blue() > 200 && c.red() < 60)
                .count()
        };

        // The artwork, close up on the dot: solid red in the middle, and
        // the square nowhere in the frame.
        let r = s.run_tool(
            "render_preview",
            &json!({ "artwork": true, "object": "dot" }),
        );
        let img = pixels(&r);
        assert_eq!(center(&img), [255, 0, 0]);
        assert_eq!(blue(&img), 0);
        assert!(text(&r).contains("artwork as drawn") && text(&r).contains("Close-up"));

        // The stitches in a region around the dot: no square either, and
        // the frame is that region's shape (twice as wide as high).
        let r = s.run_tool(
            "render_preview",
            &json!({ "region": { "x_mm": dx, "y_mm": dy, "width_mm": 16, "height_mm": 8 }, "width_px": 400 }),
        );
        let img = pixels(&r);
        assert_eq!(blue(&img), 0);
        assert_eq!((img.width(), img.height()), (400, 200));

        // The whole artwork has both.
        let img = pixels(&s.run_tool("render_preview", &json!({ "artwork": true })));
        assert!(blue(&img) > 0);

        // Mistakes are said.
        for bad in [
            json!({ "object": "dot", "region": { "x_mm": 0, "y_mm": 0, "width_mm": 5, "height_mm": 5 } }),
            json!({ "region": { "x_mm": 0, "y_mm": 0, "width_mm": 0.1, "height_mm": 5 } }),
            json!({ "region": { "x_mm": 0, "y_mm": 0 } }),
            json!({ "artwork": "yes" }),
        ] {
            assert!(s.run_tool("render_preview", &bad).is_error, "{bad}");
        }
    }

    #[test]
    fn choices_that_cant_go_together_arent_offered_as_all() {
        let c = |steps: Value| crate::agent::Choice {
            label: "x".into(),
            detail: String::new(),
            steps,
            picked: false,
        };
        let set = |o: &str, k: &str| json!({ "tool": "set_object", "arguments": { "object": o, "kind": k } });
        // Two settings of different objects go together.
        assert!(combinable(&[
            c(json!([set("a", "satin")])),
            c(json!([set("b", "run")]))
        ]));
        // One deletes what the other changes.
        let del = json!({ "tool": "delete_object", "arguments": { "object": "a" } });
        assert!(!combinable(&[c(json!([del])), c(json!([set("a", "run")]))]));
        // One counts its steps from its own first.
        let dup = json!({ "tool": "duplicate_objects", "arguments": { "objects": "b" } });
        let moved = json!({ "tool": "transform_objects", "arguments": { "objects": "$0.added.0.name", "scale": 2 } });
        assert!(!combinable(&[
            c(json!([set("a", "satin")])),
            c(json!([dup, moved]))
        ]));
    }

    #[test]
    fn a_choice_is_checked_when_its_offered_and_names_its_objects() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({ "svg": BADGE, "replace": true })));
        let offer = |s: &mut Studio, steps: Value| {
            s.run_tool(
                "offer_choices",
                &json!({ "title": "t", "question": "q", "choices": [{ "label": "a", "steps": steps }] }),
            )
        };
        // A misspelt argument, or an object that isn't there, is said now.
        assert!(
            offer(
                &mut s,
                json!([{ "tool": "set_object", "arguments": { "object": "bar", "knd": "run" } }])
            )
            .is_error
        );
        assert!(offer(&mut s, json!([{ "tool": "set_object", "arguments": { "object": "nothing", "kind": "run" } }])).is_error);
        // By its place in the sewing order: kept by name.
        let i = s.names.iter().position(|n| n == "bar").unwrap();
        let r = offer(
            &mut s,
            json!([{ "tool": "set_object", "arguments": { "object": i, "kind": "run" } }]),
        );
        assert!(!r.is_error, "{}", text(&r));
        let kept = s.agent.log.iter().rev().find_map(|e| match e {
            crate::agent::Entry::Choices { choices, .. } => Some(choices[0].steps.clone()),
            _ => None,
        });
        assert_eq!(kept.unwrap()[0]["arguments"]["object"], "bar");
    }

    #[test]
    fn get_design_says_each_prompt_once() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({ "svg": BADGE, "replace": true })));
        let all: Vec<usize> = (0..s.doc.shapes.len()).collect();
        s.doc.set_prompt(&all, "a badge").unwrap();
        let d = ok(s.run_tool("get_design", &json!({})));
        assert_eq!(d["prompts"], json!(["a badge"]));
        assert!(
            d["objects"]
                .as_array()
                .unwrap()
                .iter()
                .all(|o| o["prompt"] == 0),
            "{d}"
        );
    }

    #[test]
    fn trim_before_schema_set_get_and_reset() {
        let specs = tool_specs();
        let spec = specs
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "set_object")
            .unwrap();
        assert_eq!(
            spec["inputSchema"]["properties"]["trim_before"]["type"],
            json!(["boolean", "null"])
        );
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({"svg": BADGE, "replace": true})));
        for on in [json!(true), json!(false), Value::Null] {
            let r = ok(s.run_tool("set_object", &json!({"object": "bar", "trim_before": on})));
            assert_eq!(r["settings"]["trim_before"], on);
            let r = ok(s.run_tool("get_design", &json!({})));
            let object = r["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["name"] == "bar")
                .unwrap();
            assert_eq!(object["settings"]["trim_before"], on);
            let svg = s.doc.to_svg().unwrap();
            let back = zigzag::document::Document::from_svg(&svg, "x", None).unwrap();
            assert_eq!(
                back.shapes
                    .iter()
                    .filter_map(|s| s.params.trim_before)
                    .next(),
                on.as_bool()
            );
        }
        assert!(
            s.run_tool(
                "set_object",
                &json!({"object": "bar", "trim_before": "yes"})
            )
            .is_error
        );
    }

    #[test]
    fn the_agent_groups_objects_by_name_and_sees_which_group_each_is_in() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="30mm" viewBox="0 0 60 30">
            <rect id="sky" x="0" y="0" width="60" height="30" fill="#203040"/>
            <rect id="wave_one" x="5" y="18" width="50" height="3" fill="#f0e0c0"/>
            <rect id="wave_two" x="5" y="23" width="50" height="3" fill="#f0e0c0"/>
            <rect id="sun" x="40" y="4" width="8" height="8" fill="#f0a020"/>
        </svg>"##;
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({"svg": svg, "replace": true})));
        let group = |s: &mut Studio, name: &str| -> Value {
            let r = ok(s.run_tool("get_design", &json!({})));
            r["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["name"] == name)
                .unwrap_or_else(|| panic!("no {name}"))["group"]
                .clone()
        };
        let run = |s: &mut Studio, tool: &str, args: Value| !s.run_tool(tool, &args).is_error;
        assert_eq!(group(&mut s, "wave one"), Value::Null);
        // A group is of things that belong together, and says what it is.
        let waves = json!(["wave one", "wave two"]);
        assert!(!run(&mut s, "group_objects", json!({"objects": waves})), "no name");
        assert!(!run(&mut s, "group_objects", json!({"objects": ["sun"], "name": "sun"})));
        let undo = s.undo.len();
        assert!(run(&mut s, "group_objects", json!({"objects": waves, "name": "Waves"})));
        assert_eq!(s.undo.len(), undo + 1, "one step");
        assert_eq!(group(&mut s, "wave one"), json!("Waves"));
        assert_eq!(group(&mut s, "wave two"), json!("Waves"));
        assert_eq!(group(&mut s, "sun"), Value::Null);
        assert_eq!(group(&mut s, "sky"), Value::Null);
        // Into a group that's there, by its name; not one that isn't.
        assert!(run(&mut s, "group_objects", json!({"objects": ["sun"], "into": "waves"})));
        assert_eq!(group(&mut s, "sun"), json!("Waves"));
        // (Sewn with the group, so it's listed as one: not in two parts.)
        let members = s.group_members("Waves");
        assert_eq!(members.len(), 3);
        assert_eq!(s.group_run("Waves", members[0]), members);
        assert!(!run(&mut s, "group_objects", json!({"objects": ["sky"], "into": "clouds"})));
        // One shape, or a shape and its own outline, is no group.
        s.set_selection(&[0]);
        assert!(!s.action_enabled(crate::commands::Action::Group));
        assert!(!s.action_enabled(crate::commands::Action::Ungroup));
        let waves_now = s.group_members("Waves");
        s.set_selection(&waves_now[..1]);
        assert!(s.action_enabled(crate::commands::Action::Ungroup));
        // Out again: the one, then every one (a second time there's nothing to do).
        assert!(run(&mut s, "ungroup_objects", json!({"objects": "sun"})));
        assert_eq!(group(&mut s, "sun"), Value::Null);
        assert_eq!(group(&mut s, "wave two"), json!("Waves"));
        assert!(run(&mut s, "ungroup_objects", json!({"objects": "all"})));
        assert_eq!(group(&mut s, "wave two"), Value::Null);
        assert!(!run(&mut s, "ungroup_objects", json!({"objects": "all"})));
        // What it's told: how to group what it draws, and that it can.
        assert!(INSTRUCTIONS.contains("group_objects"));
        let specs = tool_specs();
        let add = specs
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "add_svg")
            .unwrap();
        assert!(add["description"].as_str().unwrap().contains("parts of one thing"));
    }

    #[test]
    fn blend_is_a_document_setting_that_saves_and_comes_back() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({ "svg": BADGE, "replace": true })));
        assert!(
            s.run_tool("set_document", &json!({ "blend_mm": 40 }))
                .is_error
        );
        ok(s.run_tool("set_document", &json!({ "blend_mm": 8 })));
        let d = ok(s.run_tool("get_design", &json!({})));
        assert_eq!(d["document"]["blend_mm"], 8.0);
        assert!(d["stats"]["blended_gaps"].is_u64());
        assert_eq!(s.doc.plan_options().blend, 80.0);
        // In the file, and back from it.
        let svg = s.doc.to_svg().unwrap();
        assert!(svg.contains("blend=8"), "{}", &svg[..200.min(svg.len())]);
        let back = zigzag::document::Document::from_svg(&svg, "x", None).unwrap();
        assert_eq!(back.settings.blend_mm, 8.0);
    }

    #[test]
    fn a_cut_is_listed_and_routed_by_hand_or_along_a_drawn_line() {
        let mut s = Studio::with_file(None, None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="20mm" viewBox="0 0 80 20">
            <rect id="left" x="0" y="0" width="10" height="10" fill="#aa0000"/>
            <rect id="right" x="60" y="0" width="10" height="10" fill="#aa0000"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        // One cut, between the two, with where its ends are.
        let cuts = ok(s.run_tool("get_cuts", &json!({})));
        let list = cuts["cuts"].as_array().unwrap();
        assert_eq!(list.len(), 1, "{cuts}");
        assert_eq!(list[0]["after"], "left");
        assert_eq!(list[0]["begins_on"], "right");
        assert!(list[0]["gap_mm"].as_f64().unwrap() > 40.0);
        // (Too far to look for a way: nothing said about it.)
        assert!(list[0].get("straight_crosses").is_none(), "{cuts}");
        let trims = s.design.trim_count();
        // Routed by the agent: gone, kept in the file, and undone in one.
        let r = ok(s.run_tool(
            "set_object",
            &json!({ "object": "right", "route_mm": [[-10.0, 12.0], [10.0, 12.0]] }),
        ));
        assert_eq!(
            r["settings"]["route_mm"],
            json!([[-10.0, 12.0], [10.0, 12.0]])
        );
        assert_eq!(s.design.trim_count(), trims - 1);
        assert!(s.doc.to_svg().unwrap().contains("via=-10:12+10:12"));
        let cuts = ok(s.run_tool("get_cuts", &json!({})));
        assert_eq!(cuts["routed_by_hand"][0]["to"], "right");
        ok(s.run_tool(
            "set_object",
            &json!({ "object": "right", "route_mm": null }),
        ));
        assert_eq!(s.design.trim_count(), trims);
        // By hand: a line drawn from one to the other, and the menu's
        // Route Thread Along Path with it and the object selected.
        let line = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="20mm" viewBox="0 0 80 20">
            <path id="way" d="M10 12 L30 16 L60 12" stroke="#000" stroke-width="0.3" fill="none"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": line })));
        let at = |s: &Studio, n: &str| s.names.iter().position(|x| x == n).unwrap();
        let (way, right) = (at(&s, "way"), at(&s, "right"));
        s.set_selection(&[way, right]);
        assert_eq!(s.route_pair(), Some((way, right)));
        let undo = s.undo.len();
        s.route_along_selected();
        assert!(!s.names.iter().any(|n| n == "way"), "the line is used up");
        let right = at(&s, "right");
        assert_eq!(
            s.doc.shapes[right].params.via.as_ref().map(Vec::len),
            Some(3)
        );
        assert_eq!(s.undo.len(), undo + 1, "one step");
        assert_eq!(s.design.trim_count(), trims - 1);
        // Its route back as a line to reshape, and put back from that alone.
        s.set_selection(&[right]);
        s.edit_route_as_path();
        let guide = s.sel_indices()[0];
        assert_eq!(s.doc.shapes[guide].params.kind, Some(Kind::Skip));
        assert!(s.route_pair().is_some(), "the line knows whose it is");
        s.route_along_selected();
        let right = at(&s, "right");
        assert!(s.doc.shapes[right].params.via.is_some());
        s.set_selection(&[right]);
        s.clear_routes();
        assert!(s.doc.shapes[at(&s, "right")].params.via.is_none());
        assert_eq!(s.design.trim_count(), trims);
    }

    #[test]
    fn a_cut_says_what_it_crosses_and_the_way_that_shows_least() {
        let mut s = Studio::with_file(None, None);
        // Two red squares on a red ground, a black bar between them.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm" viewBox="0 0 40 40">
            <rect id="ground" x="0" y="0" width="40" height="40" fill="#d20a0a"/>
            <rect id="bar" x="18" y="8" width="4" height="9" fill="#000000"/>
            <rect id="left" x="8" y="10" width="5" height="5" fill="#c80000"/>
            <rect id="right" x="27" y="10" width="5" height="5" fill="#c80000"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        let cuts = ok(s.run_tool("get_cuts", &json!({})));
        let cut = cuts["cuts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["begins_on"] == "right")
            .unwrap_or_else(|| panic!("{cuts}"))
            .clone();
        // Straight across is over the bar, as plain as it gets.
        let over: Vec<&str> = cut["straight_crosses"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["over"].as_str())
            .collect();
        assert!(over.contains(&"bar") && over.contains(&"ground"), "{cut}");
        // Round it is on the red all the way: taken as the route, the
        // cut is gone.
        let way = &cut["least_seen_way"];
        assert_eq!(way["straight"], false, "{cut}");
        let trims = s.design.trim_count();
        ok(s.run_tool(
            "set_object",
            &json!({ "object": "right", "route_mm": way["way_mm"] }),
        ));
        assert_eq!(s.design.trim_count(), trims - 1);
        // Or left to the planner, with that much Blend.
        ok(s.run_tool(
            "set_object",
            &json!({ "object": "right", "route_mm": null }),
        ));
        let (needs, length) = (
            way["needs_blend_mm"].as_f64().unwrap(),
            way["length_mm"].as_f64().unwrap(),
        );
        assert!(needs < length + 1.0, "all of it on its own red: {way}");
        assert!(
            way["way_mm"].as_array().unwrap().len() <= 6,
            "in few points: {way}"
        );
    }

    #[test]
    fn agent_workflow_end_to_end() {
        let mut s = Studio::with_file(None, None);
        let r = ok(s.run_tool(
            "add_svg",
            &json!({ "svg": BADGE, "width_mm": 50.0, "replace": true }),
        ));
        assert_eq!(r["added"].as_array().unwrap().len(), 4);
        let design = ok(s.run_tool("get_design", &json!({})));
        let w = design["stats"]["width_mm"].as_f64().unwrap();
        assert!((w - 50.0).abs() < 1.5, "scaled to 50 mm: {w}");
        let names: Vec<String> = design["objects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["patch", "ring", "bar", "stitchline"]);

        // Ring and bar as satin columns; thread color; fabric for a thin cap.
        let ring = ok(s.run_tool("set_object", &json!({ "object": "ring", "kind": "satin" })));
        assert_eq!(ring["kind"], "satin");
        assert!(ring["problems"].as_array().unwrap().is_empty(), "{ring}");
        ok(s.run_tool("set_object", &json!({ "object": 2, "kind": "satin", "color": "#ffb703", "satin_underlay": ["center", "contour"] })));
        let bad = s.run_tool(
            "set_object",
            &json!({ "object": "stitchline", "kind": "fill" }),
        );
        assert!(bad.is_error && text(&bad).contains("stroke"));
        ok(s.run_tool(
            "set_document",
            &json!({ "fabric": "thin-cap", "hat_order": true, "preview_color": "#c1121f" }),
        ));

        // Preview is a real PNG with a summary.
        let img = s.run_tool("render_preview", &json!({ "width_px": 400 }));
        assert!(!img.is_error);
        let png = img
            .content
            .iter()
            .find_map(|c| match c {
                Content::Image(b) => Some(b.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(text(&img).contains("Sewing order"));

        // Undo the last change (document settings).
        ok(s.run_tool("undo", &json!({})));
        assert_eq!(s.doc.settings.fabric, "standard");
        ok(s.run_tool("set_document", &json!({ "fabric": "thin-cap" })));

        // Save, reopen, everything is still there.
        let dir = std::env::temp_dir().join(format!("zigzag-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("badge.svg");
        let r = s.run_tool("save", &json!({ "path": path.to_str().unwrap() }));
        assert!(!r.is_error, "{}", text(&r));
        let reopened = Document::open(&path).unwrap();
        assert_eq!(reopened.settings.fabric, "thin-cap");
        let bar = reopened
            .shapes
            .iter()
            .find(|x| zigzag::svg::name_words(&x.id) == vec!["bar"])
            .unwrap();
        assert_eq!(bar.params.kind, Some(Kind::Satin));
        assert_eq!(bar.color, [0xff, 0xb7, 0x03]);
        assert_eq!(
            bar.params.satin_underlay,
            Some(underlay::CENTER | underlay::CONTOUR)
        );

        let pes = dir.join("badge.pes");
        let r = s.run_tool(
            "export",
            &json!({ "format": "pes", "path": pes.to_str().unwrap() }),
        );
        assert!(!r.is_error, "{}", text(&r));
        assert!(std::fs::read(&pes).unwrap().starts_with(b"#PES0001"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_can_set_a_document_up_for_a_machine() {
        let mut s = Studio::with_file(None, None);
        let doc = |s: &mut Studio| ok(s.run_tool("get_design", &json!({})))["document"].clone();
        assert!(doc(&mut s)["machine"].is_null());
        let list = ok(s.run_tool("list_machines", &json!({})));
        let melco = list["machines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == "melco-summit")
            .unwrap();
        assert_eq!(melco["hoops"], json!(["410x393"]));
        assert_eq!(
            (
                &melco["format"],
                &melco["yours"],
                &melco["sewn_from_zigzag"]
            ),
            (&json!("exp"), &json!(false), &json!(false))
        );
        assert!(list["document"].is_null() && list["new_documents"].is_null());
        // By name: an empty document goes to the machine's default hoop,
        // which is named for the machine when no standard hoop is that size.
        ok(s.run_tool("set_document", &json!({ "machine": "Melco Summit" })));
        let d = doc(&mut s);
        assert_eq!(
            d["machine"],
            json!({ "id": "melco-summit", "name": "Melco Summit", "format": "exp" })
        );
        assert_eq!(d["hoop"]["id"], "410x393");
        assert_eq!(d["hoop"]["name"], "410 × 393 mm · Melco Summit");
        // (Which machine new documents start with is the user's to say.)
        assert_eq!(s.machines.current, None);
        // Replacing the artwork keeps the document's settings.
        ok(s.run_tool("set_document", &json!({ "fabric": "thin-cap" })));
        ok(s.run_tool(
            "add_svg",
            &json!({ "svg": BADGE, "width_mm": 50.0, "replace": true }),
        ));
        let d = doc(&mut s);
        assert_eq!(
            (&d["machine"]["id"], &d["hoop"]["id"], &d["fabric"]),
            (
                &json!("melco-summit"),
                &json!("410x393"),
                &json!("thin-cap")
            )
        );
        // Export without a format writes the file the machine reads.
        let dir = std::env::temp_dir().join(format!("zigzag-machine-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("badge.exp");
        let r = s.run_tool("export", &json!({ "path": out.to_str().unwrap() }));
        assert!(!r.is_error, "{}", text(&r));
        let exp =
            zigzag::formats::write_format_with(&s.design, "exp", &s.doc.settings.export_options())
                .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), exp);
        // A hoop given along with the machine is the hoop.
        ok(s.run_tool(
            "set_document",
            &json!({ "machine": "brother-5x7", "hoop": "100x100" }),
        ));
        assert_eq!(doc(&mut s)["hoop"]["id"], "100x100");
        // A design in a hoop the next machine has stays in it; in one it
        // hasn't, it moves to that machine's default.
        ok(s.run_tool("set_document", &json!({ "machine": "brother-pr" })));
        assert_eq!(doc(&mut s)["hoop"]["id"], "100x100");
        ok(s.run_tool("set_document", &json!({ "machine": "janome-550e" })));
        assert_eq!(doc(&mut s)["hoop"]["id"], "200x360");
        // No machine again: the hoop stays, and export needs its format,
        // from the call or from what the path ends in.
        ok(s.run_tool("set_document", &json!({ "machine": "none" })));
        let d = doc(&mut s);
        assert!(d["machine"].is_null());
        assert_eq!(d["hoop"]["id"], "200x360");
        let r = s.run_tool("export", &json!({ "path": out.to_str().unwrap() }));
        assert!(!r.is_error, "{}", text(&r));
        let plain = dir.join("badge");
        let r = s.run_tool("export", &json!({ "path": plain.to_str().unwrap() }));
        assert!(
            r.is_error && text(&r).contains("format is required"),
            "{}",
            text(&r)
        );
        let r = s.run_tool("set_document", &json!({ "machine": "nope" }));
        assert!(
            r.is_error && text(&r).contains("melco-summit"),
            "{}",
            text(&r)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_can_set_the_shortest_stitch_and_the_locks() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool(
            "add_svg",
            &json!({ "svg": BADGE, "width_mm": 50.0, "replace": true }),
        ));
        let doc = |s: &mut Studio| ok(s.run_tool("get_design", &json!({})))["document"].clone();
        let d = doc(&mut s);
        assert_eq!(d["min_stitch_mm"], 0.1);
        assert_eq!(
            d["lock"],
            json!({ "style": "back", "length_mm": 0.4, "count": 4 })
        );
        // The stitches of the file as written, shortest first.
        let dir = std::env::temp_dir().join(format!("zigzag-minst-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("badge.dst");
        let shortest = |s: &mut Studio| {
            let r = s.run_tool(
                "export",
                &json!({ "format": "dst", "path": out.to_str().unwrap() }),
            );
            assert!(!r.is_error, "{}", text(&r));
            let data = std::fs::read(&out).unwrap();
            let back = zigzag::formats::read::read_stitch_file(&data, "dst").unwrap();
            let mut lens: Vec<f64> = Vec::new();
            for b in &back.blocks {
                for r in &b.runs {
                    // (Not the needle going down where a jump landed:
                    // that's no stitch, and reads back as no distance.)
                    lens.extend(r.windows(2).map(|w| w[0].dist(w[1])).filter(|l| *l > 0.0));
                }
            }
            (
                lens.iter().copied().fold(f64::INFINITY, f64::min),
                lens.len(),
            )
        };
        let (usual, n_usual) = shortest(&mut s);
        assert!(usual < 4.0, "the usual file has stitches under 0.4 mm");
        // A machine that drops short stitches: none in the file, locks
        // included, and about as many stitches as before.
        ok(s.run_tool("set_document", &json!({ "machine": "melco-summit" })));
        assert_eq!(doc(&mut s)["min_stitch_mm"], 0.4);
        let (min, n) = shortest(&mut s);
        assert!(min >= 4.0, "a {min} stitch");
        assert!(n as f64 > n_usual as f64 * 0.97, "{n} of {n_usual}");
        // Given values win over the machine's, and a lock can change in part.
        let d = ok(s.run_tool(
            "set_document",
            &json!({ "machine": "melco-summit", "min_stitch_mm": 0.6, "lock": { "style": "triangle" } }),
        ));
        assert_eq!(d["min_stitch_mm"], 0.6);
        assert_eq!(
            d["lock"],
            json!({ "style": "triangle", "length_mm": 0.4, "count": 6 })
        );
        let d = ok(s.run_tool(
            "set_document",
            &json!({ "lock": { "count": 3, "length_mm": 0.5 } }),
        ));
        assert_eq!(
            d["lock"],
            json!({ "style": "triangle", "length_mm": 0.5, "count": 3 })
        );
        let (min, _) = shortest(&mut s);
        assert!(min >= 6.0, "a {min} stitch");
        // No locks: fewer stitches, the same cuts.
        let (_, with) = shortest(&mut s);
        let d = ok(s.run_tool("set_document", &json!({ "lock": "none" })));
        assert_eq!(d["lock"], json!({ "style": "none" }));
        let (_, without) = shortest(&mut s);
        assert!(without < with);
        // A machine that says nothing of either puts the usual back.
        let d = ok(s.run_tool("set_document", &json!({ "machine": "brother-5x7" })));
        assert_eq!(d["min_stitch_mm"], 0.1);
        assert_eq!(d["lock"]["style"], "back");
        for bad in [
            json!({ "min_stitch_mm": 3 }),
            json!({ "lock": { "style": "knot" } }),
        ] {
            assert!(s.run_tool("set_document", &bad).is_error, "{bad}");
        }
        let list = ok(s.run_tool("list_machines", &json!({})));
        let melco = list["machines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == "melco-summit")
            .unwrap();
        assert_eq!(melco["min_stitch_mm"], 0.4);
        assert!(melco["lock"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_terse_answer_says_ok_what_was_added_and_what_is_newly_wrong() {
        let mut s = Studio::with_file(None, None);
        // Adding: the objects' names, not the objects.
        let r = ok(s.run_tool(
            "add_svg",
            &json!({ "svg": BADGE, "width_mm": 50.0, "terse": true }),
        ));
        let added = r["added"].as_array().unwrap().clone();
        assert!(
            added.len() >= 3 && added.iter().all(Value::is_string),
            "{r}"
        );
        assert_eq!(r["ok"], true);
        assert!(r.get("total_objects").is_none(), "{r}");
        let bar = added
            .iter()
            .find(|n| n.as_str().unwrap().contains("bar"))
            .unwrap()
            .clone();
        // A change that breaks nothing: one line's worth.
        let full = text(&s.run_tool("set_object", &json!({ "object": bar, "kind": "satin" })));
        let r = s.run_tool(
            "set_object",
            &json!({ "object": bar, "kind": "fill", "terse": true }),
        );
        assert_eq!(ok(r), json!({ "ok": true, "new_problems": [] }));
        assert!(full.len() > 300, "the full answer is the whole object");
        // One that does: the problem it made, once.
        let r = ok(s.run_tool("set_document", &json!({ "hoop": "20x20", "terse": true })));
        let new = r["new_problems"].as_array().unwrap();
        assert_eq!(new.len(), 1, "{r}");
        assert!(
            new[0]["message"]
                .as_str()
                .unwrap()
                .contains("the hoop sews")
        );
        let r = ok(s.run_tool("set_document", &json!({ "fabric": "knit", "terse": true })));
        assert_eq!(r["new_problems"], json!([]), "still wrong, not newly wrong");
        assert_eq!(
            problem_kind("Design is 50.6 × 49.5 mm; the hoop sews 20 × 20 mm."),
            "Design is # × # mm; the hoop sews # × # mm."
        );
        ok(s.run_tool("set_document", &json!({ "hoop": "100x100" })));
        // A batch: the count, what was added, what the reading steps found
        // and what's newly wrong; later steps still use earlier results.
        let steps = json!([
            { "tool": "add_svg", "arguments": { "svg": BADGE, "width_mm": 20.0, "x_mm": 30.0 } },
            { "tool": "set_object", "arguments": { "object": "$0.added.0.name", "kind": "contour" } },
            { "tool": "set_object", "arguments": { "object": "$0.added.1.name", "angle": 30 } },
            { "tool": "check_design", "arguments": {} },
        ]);
        let before = s.doc.shapes.len();
        let full = text(&s.run_tool("batch", &json!({ "steps": steps })));
        ok(s.run_tool("undo", &json!({})));
        assert_eq!(s.doc.shapes.len(), before);
        let r = s.run_tool("batch", &json!({ "steps": steps, "terse": true }));
        let short = text(&r);
        let r = ok(r);
        assert_eq!((&r["ok"], &r["steps"]), (&json!(true), &json!(4)));
        assert_eq!(r["added"].as_array().unwrap().len(), added.len());
        let kept = r["results"].as_array().unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            (&kept[0]["step"], &kept[0]["tool"]),
            (&json!(3), &json!("check_design"))
        );
        assert!(r["new_problems"].is_array());
        assert!(
            short.len() * 4 < full.len(),
            "{} of {}",
            short.len(),
            full.len()
        );
        let first = s.doc.shapes.len() - added.len();
        assert_eq!(s.doc.shapes[first].params.kind, Some(Kind::Contour));
        // A sentence stays a sentence; a tool that only reads has nothing
        // to cut short; a failure says what failed.
        let dir = std::env::temp_dir().join(format!("zigzag-terse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("b.dst");
        let r = s.run_tool(
            "export",
            &json!({ "format": "dst", "path": out.to_str().unwrap(), "terse": true }),
        );
        assert!(
            !r.is_error && text(&r).starts_with("Wrote "),
            "{}",
            text(&r)
        );
        assert!(s.run_tool("get_design", &json!({ "terse": true })).is_error);
        let r = s.run_tool("set_object", &json!({ "object": "nope", "terse": true }));
        assert!(r.is_error);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_can_say_how_narrow_a_satin_may_be() {
        let mut s = Studio::with_file(None, None);
        // A 0.9 mm satin line.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 10"><path id="stem_satin" d="M2 5 L38 5" fill="none" stroke="#111" stroke-width="0.9"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "width_mm": 40.0 })));
        let narrow = |s: &mut Studio| {
            ok(s.run_tool("check_design", &json!({})))
                .to_string()
                .contains("too narrow to look smooth")
        };
        assert!(narrow(&mut s));
        let d = ok(s.run_tool("set_document", &json!({ "narrow_satin_mm": 0.8 })));
        assert_eq!(d["narrow_satin_mm"], 0.8);
        assert!(!narrow(&mut s));
        assert!(s.doc.to_svg().unwrap().contains("narrowsatin=0.8"));
        assert!(
            s.run_tool("set_document", &json!({ "narrow_satin_mm": 9 }))
                .is_error
        );
        // And which way its stitches lean.
        let r = ok(s.run_tool("set_object", &json!({ "object": 0, "lean": 8 })));
        assert_eq!(r["settings"]["lean"], 8.0);
        assert!(s.doc.to_svg().unwrap().contains("lean=8"));
        let r = ok(s.run_tool("set_object", &json!({ "object": 0, "lean": 200 })));
        assert_eq!(r["settings"]["lean"], 60.0);
        let r = ok(s.run_tool("set_object", &json!({ "object": 0, "lean": null })));
        assert_eq!(r["settings"]["lean"], 0.0);
        assert!(!s.doc.to_svg().unwrap().contains("lean="));
        // Its zigzag is even unless it's said not to be (which is all a
        // file records).
        assert_eq!(r["settings"]["even"], true);
        // (A filled satin's corners: cut unless asked not to be, and then
        // mitred where that's clean, or everywhere.)
        if r["settings"].get("cut").is_some() {
            assert_eq!(r["settings"]["cut"], true);
            let c = ok(s.run_tool("set_object", &json!({ "object": 0, "cut": false })));
            assert_eq!(c["settings"]["cut"], false);
            assert_eq!(c["settings"]["mitred"], false);
            let c = ok(s.run_tool("set_object", &json!({ "object": 0, "mitred": true })));
            assert_eq!(c["settings"]["mitred"], true);
            let c = ok(s.run_tool("set_object", &json!({ "object": 0, "mitred": false })));
            assert_eq!(c["settings"]["mitred"], false);
            let c = ok(s.run_tool("set_object", &json!({ "object": 0, "cut": true })));
            assert_eq!(c["settings"]["cut"], true);
        }
        let r = ok(s.run_tool("set_object", &json!({ "object": 0, "even": false })));
        assert_eq!(r["settings"]["even"], false);
        assert!(s.doc.to_svg().unwrap().contains("even=0"));
        let r = ok(s.run_tool("set_object", &json!({ "object": 0, "even": true })));
        assert_eq!(r["settings"]["even"], true);
        assert!(!s.doc.to_svg().unwrap().contains("even="));
        // Rails are two of the object's own subpaths.
        for bad in [json!([1, 1]), json!([0, 1]), json!([1, 99]), json!("1+2")] {
            assert!(
                s.run_tool("set_object", &json!({ "object": 0, "rails": bad }))
                    .is_error
            );
        }
        assert!(!s.doc.to_svg().unwrap().contains("rails="));
    }

    #[test]
    fn an_agent_reshapes_a_broken_out_column_point_by_point() {
        let mut s = Studio::with_file(None, None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><path id="H_satin" fill="#123456" d="M10 10 L13 10 L13 19 L27 19 L27 10 L30 10 L30 30 L27 30 L27 21 L13 21 L13 30 L10 30 Z"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "width_mm": 20.0 })));
        ok(s.run_tool("break_into_columns", &json!({ "objects": 0 })));
        // A column reads as two rails and its rungs, points in mm.
        let p = ok(s.run_tool("get_path", &json!({ "object": 0 })));
        let subs = p["subpaths"].as_array().unwrap().clone();
        assert!(subs.len() >= 3, "{p}");
        assert_eq!(subs[0]["role"], "rail");
        assert_eq!(subs[1]["role"], "rail");
        assert!(subs[2..].iter().all(|x| x["role"] == "rung"), "{p}");
        let at = |v: &Value| (v[0].as_f64().unwrap(), v[1].as_f64().unwrap());
        let first = at(&subs[0]["points"][0]["at_mm"]);
        // Move a rail's point, by an amount and to a place.
        let r = ok(s.run_tool(
            "edit_path",
            &json!({ "object": 0, "edits": [{ "op": "move", "subpath": 1, "point": 1, "by_mm": [0.5, 0.0] }] }),
        ));
        let moved = at(&r["subpaths"][0]["points"][0]["at_mm"]);
        assert!(
            (moved.0 - first.0 - 0.5).abs() < 0.02 && (moved.1 - first.1).abs() < 0.02,
            "{r}"
        );
        assert_eq!(r["settings"]["rails"], json!([1, 2]), "still its rails");
        let undo = s.undo.len();
        let r = ok(s.run_tool(
            "edit_path",
            &json!({ "object": 0, "edits": [{ "op": "move", "subpath": 1, "point": 1, "to_mm": [first.0, first.1] }] }),
        ));
        assert_eq!(s.undo.len(), undo + 1, "one undo step a call");
        let back = at(&r["subpaths"][0]["points"][0]["at_mm"]);
        assert!((back.0 - first.0).abs() < 0.02 && (back.1 - first.1).abs() < 0.02);
        // A rung added across the column, a point added to a rail, and
        // both taken away again, in one call each.
        let n = r["subpaths"].as_array().unwrap().len();
        let rail_points = r["subpaths"][0]["points"].as_array().unwrap().len();
        let a = at(&r["subpaths"][0]["points"][0]["at_mm"]);
        let b = at(&r["subpaths"][1]["points"][0]["at_mm"]);
        let (ax, ay, bx, by) = (
            a.0 + (a.0 - b.0) * 0.1,
            a.1 + (a.1 - b.1) * 0.1 + 0.3,
            b.0 + (b.0 - a.0) * 0.1,
            b.1 + (b.1 - a.1) * 0.1 + 0.3,
        );
        let r = ok(s.run_tool(
            "edit_path",
            &json!({ "object": 0, "edits": [
                { "op": "add_subpath", "points_mm": [[ax, ay], [bx, by]] },
                { "op": "insert", "subpath": 1, "after": 1 }
            ] }),
        ));
        assert_eq!(r["subpaths"].as_array().unwrap().len(), n + 1);
        assert_eq!(r["subpaths"][n]["role"], "rung");
        assert_eq!(
            r["subpaths"][0]["points"].as_array().unwrap().len(),
            rail_points + 1
        );
        let r = ok(s.run_tool(
            "edit_path",
            &json!({ "object": 0, "edits": [
                { "op": "delete_subpath", "subpath": n + 1 },
                { "op": "delete", "subpath": 1, "point": 2 }
            ] }),
        ));
        assert_eq!(r["subpaths"].as_array().unwrap().len(), n);
        assert_eq!(
            r["subpaths"][0]["points"].as_array().unwrap().len(),
            rail_points
        );
        // What can't be done says why, and changes nothing.
        let before = s.doc.to_svg().unwrap();
        for bad in [
            json!([{ "op": "delete_subpath", "subpath": 1 }]),
            json!([{ "op": "move", "subpath": 99, "point": 1, "by_mm": [1, 0] }]),
            json!([{ "op": "move", "subpath": 1, "point": 1 }]),
            // (Several subpaths: which one must be said.)
            json!([{ "op": "move", "point": 1, "by_mm": [1, 0] }]),
            json!([{ "op": "move", "subpath": 1, "point": 1, "by_mm": [1, 0] }, { "op": "bend" }]),
            json!([]),
        ] {
            assert!(
                s.run_tool("edit_path", &json!({ "object": 0, "edits": bad }))
                    .is_error,
                "{bad}"
            );
        }
        assert_eq!(s.doc.to_svg().unwrap(), before);
        // It still sews, as a satin.
        let d = ok(s.run_tool("check_design", &json!({})));
        assert!(d.to_string().len() > 2);
        assert!(s.doc.plan().stitch_count() > 100);
    }

    #[test]
    fn deleting_a_shape_then_its_outline_in_a_batch_goes_through() {
        // As an agent sent it: the leg, then the leg's outline (gone with it).
        let mut s = Studio::with_file(None, None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40">
            <rect id="Leg" x="0" y="0" width="10" height="20" fill="#a86"/>
            <rect id="Paw" x="20" y="0" width="10" height="10" fill="#321"/>
            <rect id="Ear" x="20" y="20" width="10" height="10" fill="#654"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        ok(s.run_tool(
            "add_outline",
            &json!({ "objects": ["Leg", "Paw"], "kind": "run" }),
        ));
        assert!(s.names.iter().any(|n| n == "Leg (outline)"));
        let r = ok(s.run_tool(
            "batch",
            &json!({ "steps": [
                { "tool": "delete_object", "arguments": { "object": "Leg" } },
                { "tool": "delete_object", "arguments": { "object": "Leg (outline)" } },
                { "tool": "delete_object", "arguments": { "objects": ["Paw", "Paw (outline)"] } }
            ] }),
        ));
        assert!(r.to_string().contains("already went with its shape"), "{r}");
        assert_eq!(s.names, ["Ear"]);
        // A name that never was is still a mistake.
        assert!(
            s.run_tool("delete_object", &json!({ "object": "Tail (outline)" }))
                .is_error
        );
        assert!(
            s.run_tool("delete_object", &json!({ "object": "Tail" }))
                .is_error
        );
        assert_eq!(s.names, ["Ear"]);
    }

    #[test]
    fn a_failed_batch_says_which_step_took_the_object_and_what_was_sent() {
        let mut s = Studio::with_file(None, None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40">
            <rect id="Leg" x="0" y="0" width="10" height="20" fill="#a86"/>
            <rect id="Ear" x="20" y="20" width="10" height="10" fill="#654"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        let r = s.run_tool(
            "batch",
            &json!({ "steps": [
                { "tool": "set_object", "arguments": { "object": "Ear", "kind": "satin" } },
                { "tool": "delete_object", "arguments": { "object": "Leg" } },
                { "tool": "set_object", "arguments": { "object": "Leg", "color": "#ffffff" } }
            ] }),
        );
        assert!(r.is_error);
        let v: Value = serde_json::from_str(&text(&r)).unwrap();
        let e = v["error"].as_str().unwrap();
        assert!(e.contains("step 1 (delete_object) took it away"), "{e}");
        assert!(e.contains("send the batch again"), "{e}");
        assert_eq!(v["failed_step"]["step"], 2);
        assert_eq!(v["failed_step"]["arguments"]["color"], "#ffffff");
        assert_eq!(s.names, ["Leg", "Ear"], "nothing changed");
        // A name that was never there: no story about an earlier step.
        let r = s.run_tool(
            "batch",
            &json!({ "steps": [{ "tool": "set_object", "arguments": { "object": "Tail", "kind": "run" } }] }),
        );
        let v: Value = serde_json::from_str(&text(&r)).unwrap();
        assert!(!v["error"].as_str().unwrap().contains("took it away"));
    }

    #[test]
    fn review_design_says_what_it_costs_and_what_wont_read() {
        let mut s = Studio::with_file(None, None);
        // Cream on a green patch, cream on the bare (cream) fabric, and two
        // red specks far apart.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40">
            <rect id="Patch" x="0" y="0" width="40" height="40" fill="#2f5d3a"/>
            <rect id="On_patch" x="10" y="10" width="10" height="10" fill="#f3ead8"/>
            <rect id="On_fabric" x="50" y="10" width="10" height="10" fill="#f3ead8"/>
            <rect id="Speck_one" x="45" y="35" width="1" height="1" fill="#c0392b"/>
            <rect id="Speck_two" x="75" y="2" width="1" height="1" fill="#c0392b"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        ok(s.run_tool("set_document", &json!({ "preview_color": "#ece7dc" })));
        let r = s.run_tool("review_design", &json!({}));
        assert!(!r.is_error, "{}", text(&r));
        assert!(
            r.content.iter().any(|c| matches!(c, Content::Image(_))),
            "a picture at size"
        );
        let v: Value = serde_json::from_str(&text(&r)).unwrap();
        assert_eq!(v["cost"]["threads"], 3, "{v}");
        let trims = v["cost"]["trims"].as_u64().unwrap();
        assert!(trims >= 1, "{v}");
        let put_down: u64 = v["trims_by_object"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["trims"].as_u64().unwrap())
            .sum();
        assert_eq!(put_down, trims, "every trim put down to an object: {v}");
        let small: Vec<&str> = v["small_pieces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["object"].as_str().unwrap())
            .collect();
        assert!(
            small.contains(&"Speck one") && small.contains(&"Speck two"),
            "{small:?}"
        );
        assert_eq!(
            v["threads_close_to_fabric"].as_array().unwrap().len(),
            1,
            "{v}"
        );
        assert!(v["findings"].as_array().unwrap().len() >= 3, "{v}");
        // With nothing cream on the bare fabric, the cream on the patch is fine.
        ok(s.run_tool("delete_object", &json!({ "object": "On fabric" })));
        let v: Value =
            serde_json::from_str(&text(&s.run_tool("review_design", &json!({})))).unwrap();
        assert!(
            v["threads_close_to_fabric"].as_array().unwrap().is_empty(),
            "{v}"
        );
    }

    #[test]
    fn review_design_passes_on_every_issue_the_person_sees_notes_too() {
        let mut s = Studio::with_file(None, None);
        // A small square under a big one sewn after it: covered, a note.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm" viewBox="0 0 40 40">
            <rect id="Under" x="10" y="10" width="10" height="10" fill="#c0392b"/>
            <rect id="Over" x="0" y="0" width="40" height="40" fill="#2f5d3a"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        let r = s.run_tool("review_design", &json!({}));
        let v: Value = serde_json::from_str(&text(&r)).unwrap();
        let problems = v["problems"].as_array().unwrap();
        assert_eq!(problems.len(), s.issues.len(), "all of them: {v}");
        assert!(
            problems
                .iter()
                .any(|x| x["level"] == "note" && x["object"] == "Under"),
            "{v}"
        );
        let findings = v["findings"].to_string();
        assert!(findings.contains("Issues button"), "{findings}");
        assert!(!findings.contains("Nothing to fix"), "{findings}");
    }

    #[test]
    fn review_design_says_what_document_settings_would_save_without_setting_them() {
        let mut s = Studio::with_file(None, None);
        // Two red lines end to end, 2 mm apart: a trim, unless short gaps
        // are sewn across.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="20mm" viewBox="0 0 40 20">
            <path id="One" d="M2 10 L12 10" fill="none" stroke="#c0392b" stroke-width="0.4"/>
            <path id="Two" d="M14 10 L24 10" fill="none" stroke="#c0392b" stroke-width="0.4"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        ok(s.run_tool("set_document", &json!({ "connect_mm": 0 })));
        let before = s.doc.settings.clone();
        let v: Value =
            serde_json::from_str(&text(&s.run_tool("review_design", &json!({})))).unwrap();
        let trims = v["cost"]["trims"].as_u64().unwrap();
        assert!(trims >= 1, "{v}");
        let ifs = v["what_ifs"].as_array().unwrap();
        let three = ifs
            .iter()
            .find(|w| w["change"] == "Connect 3 mm")
            .unwrap_or_else(|| panic!("{v}"));
        assert!(three["saves"].as_u64().unwrap() >= 1);
        assert_eq!(three["set_document"]["connect_mm"], 3.0);
        assert!(
            v["findings"].to_string().contains("person's to choose"),
            "{v}"
        );
        // Tried on copies: the design is as it was.
        assert_eq!(s.doc.settings.connect_mm, before.connect_mm);
        assert_eq!(s.doc.settings.over_sewn, before.over_sewn);
        assert_eq!(s.design.trim_count() as u64, trims);
    }

    #[test]
    fn review_design_finds_specks_left_by_a_trace_and_lines_across_the_ground() {
        let mut s = Studio::with_file(None, None);
        // A ground with a flower on it; a speck sewn as a loop inside the
        // flower, another in a hole the trace left in it, a line across
        // the bare ground, and an eye drawn as a stroke (a detail, kept).
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="90mm" height="70mm" viewBox="0 0 90 70">
            <rect id="Ground" x="5" y="5" width="80" height="60" fill="#173da5"/>
            <path id="Flower" d="M15 15 H45 V40 H15 Z M33 20 H38 V25 H33 Z" fill="#f47b20" fill-rule="evenodd"/>
            <rect id="Leaf" x="55" y="40" width="12" height="8" fill="#67962b"/>
            <rect id="Stem" x="60" y="15" width="3" height="20" fill="#67962b"/>
            <circle id="Speck_run" cx="22" cy="30" r="1.5" fill="#1f3655"/>
            <rect id="Hole_run" x="33" y="20" width="5" height="5" fill="#1f3655"/>
            <path id="Road" d="M50 58 L82 52" fill="none" stroke="#0b3d91" stroke-width="0.3"/>
            <path id="Eye" d="M24 20 L28 20" fill="none" stroke="#1f3655" stroke-width="0.3"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        let v: Value =
            serde_json::from_str(&text(&s.run_tool("review_design", &json!({})))).unwrap();
        let names = |k: &str| -> Vec<String> {
            v[k].as_array()
                .unwrap()
                .iter()
                .map(|x| x["object"].as_str().unwrap().to_string())
                .collect()
        };
        let stray = names("stray_loops");
        assert!(stray.iter().any(|n| n.starts_with("Speck")), "{v}");
        assert!(stray.iter().any(|n| n.starts_with("Hole")), "{v}");
        assert!(!stray.iter().any(|n| n.starts_with("Eye")), "{v}");
        let lines = names("lines_on_ground");
        assert!(lines.iter().any(|n| n.starts_with("Road")), "{v}");
        assert!(!lines.iter().any(|n| n.starts_with("Eye")), "on the flower: {v}");
    }

    #[test]
    fn review_design_finds_empty_outlines_border_order_small_text_and_underlay() {
        let mut s = Studio::with_file(None, None);
        // A patch: a ground, a blank ring and an empty-looking square sewn
        // round their edge (the square has a fill of its size under it), a
        // wide satin with only a center walk, and a 2 mm border sewn first.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="90mm" height="70mm" viewBox="0 0 90 70">
            <rect id="Edge_satin_w=2" x="5" y="5" width="80" height="60" rx="6" fill="none" stroke="#76ced2" stroke-width="2"/>
            <rect id="Ground" x="6" y="6" width="78" height="58" fill="#173da5"/>
            <circle id="Wheel_run" cx="25" cy="25" r="8" fill="#feba35"/>
            <rect id="Box" x="45" y="15" width="14" height="14" fill="#f47b20"/>
            <rect id="Box_line_run" x="45" y="15" width="14" height="14" fill="#1f3655"/>
            <rect id="Bar_satin_ul=center" x="15" y="44" width="40" height="6" fill="#fff1d0"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        ok(s.run_tool("add_text", &json!({ "text": "hi", "size_mm": 3.0 })));
        let review = |s: &mut Studio| -> Value {
            serde_json::from_str(&text(&s.run_tool("review_design", &json!({})))).unwrap()
        };
        let has = |v: &Value, k: &str, n: &str| {
            v[k].as_array()
                .unwrap()
                .iter()
                .any(|x| x["object"].as_str().unwrap().starts_with(n))
        };
        let v = review(&mut s);
        assert!(has(&v, "outline_only", "Wheel"), "{v}");
        assert!(!has(&v, "outline_only", "Box"), "a fill sits under it: {v}");
        assert!(has(&v, "underlay_mismatch", "Bar"), "{v}");
        assert!(v["small_lettering"].as_array().unwrap().len() == 1, "{v}");
        let b = &v["patch_border"];
        assert!(b["object"].as_str().unwrap().starts_with("Edge"), "{v}");
        assert_eq!(b["sewn_last"], false, "{v}");
        assert!(b["why"].as_str().unwrap().contains("2.5 mm"), "{v}");

        // Fixed as the review says: nothing left to report.
        let last = s.doc.shapes.len() - 1;
        ok(s.run_tool("move_object", &json!({ "object": b["object"], "to": last })));
        ok(s.run_tool("set_object", &json!({ "object": b["object"], "width_mm": 4.0 })));
        ok(s.run_tool("set_object", &json!({ "object": "Bar_satin_ul=center", "satin_underlay": "auto" })));
        let t = (0..s.doc.shapes.len()).find(|&i| s.doc.lettering(i).is_some()).unwrap();
        ok(s.run_tool("edit_text", &json!({ "object": t, "size_mm": 8.0 })));
        let v = review(&mut s);
        assert!(v["patch_border"].is_null(), "{v}");
        assert!(v["underlay_mismatch"].as_array().unwrap().is_empty(), "{v}");
        assert!(v["small_lettering"].as_array().unwrap().is_empty(), "{v}");
    }

    #[test]
    fn review_design_puts_underlay_spacing_and_direction_first() {
        let mut s = Studio::with_file(None, None);
        // A patch ground with no underlay and open rows, two pieces on it at
        // its angle (one open too), a satin bar with no underlay, and Sew
        // over fill on: what an embroiderer sent back about a design.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="60mm" viewBox="0 0 80 60">
            <rect id="Ground_fill_a=0_s=0.42_u=0" x="5" y="5" width="70" height="50" fill="#173da5"/>
            <rect id="Leaf_fill" x="12" y="12" width="20" height="12" fill="#67962b"/>
            <rect id="Bag_fill_s=0.4_u=0" x="40" y="12" width="14" height="14" fill="#feba35"/>
            <rect id="Bar_satin_ul=none" x="12" y="40" width="40" height="4" fill="#fff1d0"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        ok(s.run_tool("set_document", &json!({ "over_sewn": true })));
        let review = |s: &mut Studio| -> Value {
            serde_json::from_str(&text(&s.run_tool("review_design", &json!({})))).unwrap()
        };
        let names = |v: &Value, k: &str| -> Vec<String> {
            v[k].as_array()
                .unwrap()
                .iter()
                .map(|x| x["object"].as_str().unwrap().to_string())
                .collect()
        };
        let v = review(&mut s);
        let off = names(&v, "underlay_off");
        assert!(off.iter().any(|n| n.starts_with("Ground")), "{v}");
        assert!(off.iter().any(|n| n.starts_with("Bag")), "{v}");
        assert!(off.iter().any(|n| n.starts_with("Bar")), "{v}");
        assert!(!off.iter().any(|n| n.starts_with("Leaf")), "{v}");
        let open = names(&v, "too_open");
        assert!(open.iter().any(|n| n.starts_with("Ground")) && open.iter().any(|n| n.starts_with("Bag")), "{v}");
        let same = names(&v, "same_direction");
        assert!(same.iter().any(|n| n.starts_with("Leaf")) && same.iter().any(|n| n.starts_with("Bag")), "{v}");
        let findings = v["findings"].as_array().unwrap();
        assert!(findings[0].as_str().unwrap().contains("underlay"), "{v}");
        assert!(v["findings"].to_string().contains("You turned Sew over fill on"), "{v}");

        // Taken: each fix as the review gives it, and nothing left to say.
        for (k, fix) in [("underlay_off", "fix"), ("too_open", "fix")] {
            for x in v[k].as_array().unwrap() {
                let mut args = x[fix].clone();
                args["object"] = x["object"].clone();
                ok(s.run_tool("set_object", &args));
            }
        }
        for x in v["same_direction"].as_array().unwrap() {
            ok(s.run_tool("set_object", &json!({ "object": x["object"], "angle": x["suggest"] })));
        }
        ok(s.run_tool("set_document", &json!({ "over_sewn": false })));
        let v = review(&mut s);
        for k in ["underlay_off", "too_open", "same_direction"] {
            assert!(v[k].as_array().unwrap().is_empty(), "{k}: {v}");
        }
        assert!(!v["findings"].to_string().contains("Sew over fill"), "{v}");
    }

    #[test]
    fn review_design_suggests_satin_for_bands_and_angles_for_flow_and_dimension() {
        let mut s = Studio::with_file(None, None);
        // A band 2 mm wide (and a diagonal one), a broad square, an arm
        // 30 mm long tilted 30° down, and two touching plates at one angle.
        let (c, sn) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
        let arm = |u: f64, v: f64| (10.0 + u * c - v * sn, 50.0 + u * sn + v * c);
        let pts: Vec<String> = [(0.0, -4.0), (30.0, -4.0), (30.0, 4.0), (0.0, 4.0)]
            .iter()
            .map(|&(u, v)| {
                let (x, y) = arm(u, v);
                format!("{x:.2},{y:.2}")
            })
            .collect();
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="80mm" viewBox="0 0 100 80">
            <rect id="Band" x="2" y="2" width="30" height="2" fill="#2584bb"/>
            <path id="Slant" d="M5 30 L25 10 L26.5 11.5 L6.5 31.5 Z" fill="#2584bb"/>
            <rect id="Block" x="35" y="10" width="20" height="20" fill="#2584bb"/>
            <polygon id="Arm" points="{}" fill="#2584bb"/>
            <rect id="Plate" x="60" y="40" width="12" height="12" fill="#0b3d91"/>
            <rect id="Rivet" x="72" y="44" width="5" height="5" fill="#a8deeb"/></svg>"##,
            pts.join(" ")
        );
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        let v: Value =
            serde_json::from_str(&text(&s.run_tool("review_design", &json!({})))).unwrap();
        let suggested: Vec<&str> = v["stitch_types"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["suggest"] == "satin")
            .map(|t| t["object"].as_str().unwrap())
            .collect();
        assert!(
            suggested.contains(&"Band") && suggested.contains(&"Slant"),
            "{v}"
        );
        assert!(
            !suggested.contains(&"Block") && !suggested.contains(&"Arm"),
            "broad pieces stay fill: {v}"
        );
        let angle = |name: &str| {
            v["fill_angles"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["object"] == name)
                .cloned()
        };
        // Flow: the arm's rows along its length.
        let a = angle("Arm").unwrap_or_else(|| panic!("{v}"));
        assert!((a["suggest"].as_f64().unwrap() - 30.0).abs() <= 2.0, "{a}");
        assert!(a["why"].as_str().unwrap().starts_with("flow"));
        // Dimension: the smaller of the touching plates turned away.
        let r = angle("Rivet").unwrap_or_else(|| panic!("{v}"));
        assert_eq!(r["suggest"].as_f64().unwrap().abs(), 45.0, "{r}");
        assert!(r["why"].as_str().unwrap().contains("Plate"), "{r}");
        assert!(angle("Plate").is_none() && angle("Block").is_none(), "{v}");
        // Taken: no longer suggested.
        ok(s.run_tool(
            "set_object",
            &json!({ "object": "Arm", "angle": a["suggest"] }),
        ));
        let v: Value =
            serde_json::from_str(&text(&s.run_tool("review_design", &json!({})))).unwrap();
        assert!(!v["fill_angles"].to_string().contains("Arm"), "{v}");
    }

    #[test]
    fn a_shape_with_one_subpath_is_edited_without_naming_it() {
        // As an agent sent it: a batch reordering, then moving a nose's
        // points without saying `subpath` (it has only the one).
        let mut s = Studio::with_file(None, None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40">
            <rect id="Body" x="0" y="0" width="30" height="20" fill="#a86"/>
            <path id="Nose" fill="#321" d="M20 20 L26 20 L26 25 L20 25 Z"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "replace": true })));
        let before = ok(s.run_tool("get_path", &json!({ "object": "Nose" })));
        let x0 = before["subpaths"][0]["points"][0]["at_mm"][0]
            .as_f64()
            .unwrap();
        let r = ok(s.run_tool(
            "batch",
            &json!({ "terse": true, "steps": [
                { "tool": "move_object", "arguments": { "object": "Nose", "to": 0 } },
                { "tool": "edit_path", "arguments": { "object": "Nose", "edits": [
                    { "op": "move", "point": 1, "by_mm": [-0.5, 0] },
                    { "op": "move", "point": 3, "by_mm": [-0.4, 0] }
                ] } }
            ] }),
        ));
        assert_eq!(r["ok"], true, "{r}");
        let after = ok(s.run_tool("get_path", &json!({ "object": "Nose" })));
        let x1 = after["subpaths"][0]["points"][0]["at_mm"][0]
            .as_f64()
            .unwrap();
        assert!((x1 - (x0 - 0.5)).abs() < 0.01, "{x0} → {x1}");
    }

    #[test]
    fn an_agent_measures_stitching_before_and_after_a_change() {
        let mut s = Studio::with_file(None, None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><path id="H_satin" fill="#123456" d="M10 10 L13 10 L13 19 L27 19 L27 10 L30 10 L30 30 L27 30 L27 21 L13 21 L13 30 L10 30 Z"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "width_mm": 20.0 })));
        // A clean H: one piece of thread, nothing outside it, nothing bare
        // to speak of.
        let was = ok(s.run_tool("measure_stitching", &json!({})));
        assert_eq!(was["objects"], json!([0]));
        assert_eq!(was["measures"]["trims"], 0);
        assert_eq!(was["measures"]["stitches_outside"], 0);
        assert_eq!(was["measures"]["filled_parts"], 0);
        assert!(was["measures"]["bare_mm2"].is_number());
        // The same again: the same.
        let again = ok(s.run_tool(
            "measure_stitching",
            &json!({ "objects": 0, "before": was["measures"] }),
        ));
        assert_eq!(again["compared"]["verdict"], "same", "{again}");
        // A change that makes it worse (stitches leaning hard) is said to
        // be, and which measure says so.
        ok(s.run_tool("set_object", &json!({ "object": 0, "lean": 45 })));
        let leant = ok(s.run_tool(
            "measure_stitching",
            &json!({ "objects": 0, "before": was["measures"] }),
        ));
        assert_eq!(leant["compared"]["verdict"], "worse", "{leant}");
        assert!(!leant["compared"]["worse"].as_array().unwrap().is_empty());
        // And putting it back, better than that.
        ok(s.run_tool("set_object", &json!({ "object": 0, "lean": null })));
        let back = ok(s.run_tool(
            "measure_stitching",
            &json!({ "objects": 0, "before": leant["measures"] }),
        ));
        assert_eq!(back["compared"]["verdict"], "better", "{back}");
        assert_eq!(back["measures"], was["measures"]);
        // Broken into columns, there's no outline to hold them to unless
        // one is kept: a copy of the shape, not sewn.
        ok(s.run_tool(
            "add_svg",
            &json!({ "svg": svg.replace("H_satin", "kept_skip"), "width_mm": 20.0 }),
        ));
        assert!(
            s.run_tool("measure_stitching", &json!({ "objects": 1 }))
                .is_error,
            "not a satin"
        );
        // With a satin, it's measured and the other is said to be left out.
        let mixed = ok(s.run_tool("measure_stitching", &json!({ "objects": [0, 1] })));
        assert_eq!(mixed["objects"], json!([0]), "{mixed}");
        assert_eq!(
            mixed["not_measured"]["objects"].as_array().unwrap().len(),
            1,
            "{mixed}"
        );
        let made = ok(s.run_tool("break_into_columns", &json!({ "objects": 0 })));
        let cols: Vec<Value> = made["made"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["index"].clone())
            .collect();
        let loose = ok(s.run_tool("measure_stitching", &json!({ "objects": cols })));
        assert!(loose["measures"]["stitches_outside"].is_null(), "{loose}");
        assert!(loose["note"].is_string());
        let kept = s.doc.shapes.len() - 1;
        let held = ok(s.run_tool(
            "measure_stitching",
            &json!({ "objects": cols, "within": kept }),
        ));
        assert!(held["measures"]["stitches_outside"].is_number(), "{held}");
        assert!(held["measures"]["bare_mm2"].is_number(), "{held}");
        assert_eq!(held["columns"], cols.len());
    }

    #[test]
    fn an_agent_makes_text_outlines_to_break_it_into_columns() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_text", &json!({ "text": "HI", "size_mm": 15.0 })));
        ok(s.run_tool("set_object", &json!({ "object": 0, "kind": "satin" })));
        // Text is text: its points aren't to be had, nor its columns.
        assert!(s.run_tool("get_path", &json!({ "object": 0 })).is_error);
        assert!(
            s.run_tool(
                "edit_path",
                &json!({ "object": 0, "edits": [{ "op": "delete", "subpath": 1, "point": 1 }] })
            )
            .is_error
        );
        assert!(
            s.run_tool("break_into_columns", &json!({ "objects": 0 }))
                .is_error
        );
        let r = ok(s.run_tool("create_outlines", &json!({ "objects": 0 })));
        assert_eq!(r["made"].as_array().unwrap().len(), 1);
        assert!(s.doc.lettering(0).is_none(), "a shape now");
        assert_eq!(s.doc.shapes[0].params.kind, Some(Kind::Satin));
        assert!(
            ok(s.run_tool("get_path", &json!({ "object": 0 })))["subpaths"]
                .as_array()
                .unwrap()
                .len()
                >= 2
        );
        // (A shape isn't made outlines again.)
        assert!(
            s.run_tool("create_outlines", &json!({ "objects": 0 }))
                .is_error
        );
        let r = ok(s.run_tool("break_into_columns", &json!({ "objects": 0 })));
        assert!(r["made"].as_array().unwrap().len() >= 3, "{r}");
    }

    #[test]
    fn overlapping_strokes_arent_offered_to_agents() {
        let mut s = Studio::with_file(None, None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><path id="H_satin" fill="#123456" d="M10 10 L13 10 L13 19 L27 19 L27 10 L30 10 L30 30 L27 30 L27 21 L13 21 L13 30 L10 30 Z"/></svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": svg, "width_mm": 20.0 })));
        let r = ok(s.run_tool("set_object", &json!({ "object": 0, "kind": "satin" })));
        assert!(
            r["settings"].get("overlap").is_none(),
            "not a setting to see"
        );
        assert!(
            s.run_tool("set_object", &json!({ "object": 0, "overlap": true }))
                .is_error
        );
        assert_eq!(s.doc.shapes[0].params.overlap, None);
        // (A file that has them says so.)
        s.doc.shapes[0].params.overlap = Some(true);
        let d = ok(s.run_tool("get_design", &json!({})));
        assert_eq!(d["objects"][0]["settings"]["overlap"], true);
    }

    #[test]
    fn export_can_write_which_object_each_stitch_is_for() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({ "svg": BADGE, "width_mm": 50.0 })));
        let dir = std::env::temp_dir().join(format!("zigzag-objects-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("badge.exp");
        let map = dir.join("badge.exp.objects.json");
        let r = s.run_tool(
            "export",
            &json!({ "format": "exp", "path": out.to_str().unwrap() }),
        );
        assert!(!r.is_error && !map.exists(), "not unless asked");
        let r = s.run_tool(
            "export",
            &json!({ "format": "exp", "path": out.to_str().unwrap(), "objects": true }),
        );
        assert!(!r.is_error, "{}", text(&r));
        assert!(text(&r).contains("badge.exp.objects.json"));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&map).unwrap()).unwrap();
        assert_eq!(v["file"], "badge.exp");
        let names: Vec<String> = v["objects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.iter().any(|n| n.contains("bar")), "{names:?}");
        assert!(v["ranges"].as_array().unwrap().len() >= names.len());
        // The Skitch's stream has no such map: the file is written, and
        // the answer says the map wasn't.
        let pen = dir.join("badge.pen");
        let r = s.run_tool(
            "export",
            &json!({ "format": "pen", "path": pen.to_str().unwrap(), "objects": true }),
        );
        assert!(r.is_error && pen.exists(), "{}", text(&r));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_can_choose_a_standard_hoop_or_give_any_size() {
        let mut s = Studio::with_file(None, None);
        let hoop =
            |s: &mut Studio| ok(s.run_tool("get_design", &json!({})))["document"]["hoop"].clone();
        assert_eq!(hoop(&mut s)["id"], "100x100");
        ok(s.run_tool("set_document", &json!({ "hoop": "200x300" })));
        let h = hoop(&mut s);
        assert_eq!(h["name"], "8×12 in (200 × 300 mm)");
        assert_eq!(
            (h["width_mm"].as_f64(), h["height_mm"].as_f64()),
            (Some(200.0), Some(300.0))
        );
        // A frame that isn't in the list.
        ok(s.run_tool("set_document", &json!({ "hoop": "150x220.5" })));
        let h = hoop(&mut s);
        assert_eq!(h["id"], "150x220.5");
        assert_eq!(h["name"], "Custom · 150 × 220.5 mm");
        for bad in ["huge", "5x100", "100x5000", "100"] {
            let r = s.run_tool("set_document", &json!({ "hoop": bad }));
            assert!(
                r.is_error && text(&r).contains("130x180"),
                "{bad}: {}",
                text(&r)
            );
        }
        assert_eq!(s.hoop_dims(), (150.0, 220.5));
    }

    #[test]
    fn agents_can_lay_out_objects() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool(
            "add_svg",
            &json!({ "svg": BADGE, "width_mm": 50.0, "replace": true }),
        ));
        let r = ok(s.run_tool(
            "transform_objects",
            &json!({ "objects": "bar", "width_mm": 20.0, "move_to_mm": [0, 30], "rotate_deg": 90 }),
        ));
        let bar = &r["objects"][0];
        assert!(
            (bar["size_mm"][1].as_f64().unwrap() - 20.0).abs() < 0.2,
            "{bar}"
        );
        assert!(
            (bar["center_mm"][1].as_f64().unwrap() - 30.0).abs() < 0.2,
            "{bar}"
        );
        let r = ok(s.run_tool(
            "duplicate_objects",
            &json!({ "objects": ["bar", "stitchline"], "offset_mm": [5, 0] }),
        ));
        assert_eq!(r["added"].as_array().unwrap().len(), 2);
        assert_eq!(s.doc.shapes.len(), 6);
        let svg = text(&s.run_tool("get_svg", &json!({})));
        assert!(svg.contains("transform=\"matrix("));

        let t = ok(s.run_tool(
            "add_text",
            &json!({ "text": "ALPINE", "size_mm": 6, "arc_mm": 20, "y_mm": -15 }),
        ));
        assert_eq!(t["lettering"]["text"], "ALPINE");
        assert_eq!(t["kind"], "satin");
        let t = ok(s.run_tool(
            "edit_text",
            &json!({ "object": t["index"], "text": "ALPINE CLUB", "kind": "fill" }),
        ));
        assert_eq!(t["lettering"]["text"], "ALPINE CLUB");
        assert_eq!(t["kind"], "fill");
        let dir = std::env::temp_dir().join(format!("zigzag-layout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for fmt in ["jef", "vp3", "exp", "html"] {
            let p = dir.join(format!("d.{fmt}"));
            let r = s.run_tool(
                "export",
                &json!({ "format": fmt, "path": p.to_str().unwrap() }),
            );
            assert!(!r.is_error, "{fmt}: {}", text(&r));
        }
        let r = ok(s.run_tool(
            "open_file",
            &json!({ "path": dir.join("d.jef").to_str().unwrap() }),
        ));
        assert!(
            r["objects"]
                .as_array()
                .unwrap()
                .iter()
                .all(|o| o["kind"] == "manual")
        );
        let _ = std::fs::remove_dir_all(&dir);
        let bad = s.run_tool("add_text", &json!({ "text": "x", "font": "No Such Font" }));
        assert!(bad.is_error && text(&bad).contains("list_fonts"));
    }

    #[test]
    fn errors_are_helpful() {
        let mut s = Studio::with_file(None, None);
        let r = s.run_tool("set_object", &json!({ "object": 3 }));
        assert!(r.is_error && text(&r).contains("no object 3"));
        let r = s.run_tool("save", &json!({}));
        assert!(r.is_error && text(&r).contains("absolute path"));
        let r = s.run_tool(
            "add_svg",
            &json!({ "svg": "<svg xmlns='http://www.w3.org/2000/svg'/>" }),
        );
        assert!(r.is_error && text(&r).contains("nothing visible"));
        let r = s.run_tool("nope", &json!({}));
        assert!(r.is_error);
    }

    #[test]
    fn objects_can_be_hidden_and_locked() {
        use crate::commands::{Action, Flag};
        let mut s = Studio::with_file(None, None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="40mm" viewBox="0 0 60 40">
            <rect id="Ground" x="2" y="2" width="56" height="36" fill="#203040"/>
            <rect id="Patch" x="10" y="10" width="15" height="15" fill="#c00000"/>
            <rect id="Badge" x="35" y="10" width="15" height="15" fill="#c2c5c3"/>
        </svg>"##;
        let ok = |r: ToolResult| {
            assert!(!r.is_error, "{}", text(&r));
            serde_json::from_str::<Value>(&text(&r)).unwrap_or(Value::Null)
        };
        ok(s.run_tool("add_svg", &json!({ "svg": svg })));
        s.replan();
        let all = s.design.stitch_count();
        let badge = s.design.per_shape[2];
        assert!(badge > 100);

        // Hidden: not sewn (what's under it is filled again, with nothing
        // on top to leave room for), still there, and said so. Shown
        // again, it's as it was.
        let o = ok(s.run_tool("set_object", &json!({ "object": "Badge", "hidden": true })));
        assert_eq!(o["settings"]["hidden"], true);
        assert_eq!(o["stitches"], 0);
        s.replan();
        assert_eq!(s.design.per_shape[2], 0);
        assert!(s.design.per_shape[0] > 0 && s.design.stitch_count() != all);
        assert_ne!(
            s.doc.shapes[2].params.kind,
            Some(Kind::Skip),
            "its stitch type is kept"
        );
        let ids = |s: &Studio| -> Vec<String> {
            let svg = s.doc.to_svg().unwrap();
            svg.split("id=\"")
                .skip(1)
                .filter_map(|t| t.split('"').next().map(str::to_string))
                .collect()
        };
        assert!(
            ids(&s)
                .iter()
                .any(|id| id.starts_with("Badge") && id.ends_with("hide=1")),
            "{:?}",
            ids(&s)
        );
        // (Kept in the file: opened again, it's still hidden.)
        let again =
            zigzag::document::Document::from_svg(&s.doc.to_svg().unwrap(), "t", None).unwrap();
        assert!(again.shapes[2].params.hidden() && !again.shapes[1].params.hidden());
        assert_eq!(svg_names(&again), ["Ground", "Patch", "Badge"]);
        let o = ok(s.run_tool("set_object", &json!({ "object": "Badge", "hidden": false })));
        assert!(o["settings"].get("hidden").is_none());
        s.replan();
        assert_eq!(
            (s.design.per_shape[2], s.design.stitch_count()),
            (badge, all)
        );

        // Locked: the pointer goes through it to nothing (or to what's
        // under it), a selection box leaves it out, and so does Select
        // All. It still sews.
        let on_patch = P::new(-120.0, -20.0);
        let on_ground = P::new(-270.0, 150.0);
        assert_eq!(s.pick(on_patch, 5.0), Some(1));
        assert_eq!(s.pick(on_ground, 5.0), Some(0));
        ok(s.run_tool("set_object", &json!({ "object": "Ground", "locked": true })));
        assert!(
            ids(&s)
                .iter()
                .any(|id| id.starts_with("Ground") && id.ends_with("lock=1")),
            "{:?}",
            ids(&s)
        );
        assert_eq!(s.pick(on_ground, 5.0), None);
        assert_eq!(s.pick(on_patch, 5.0), Some(1));
        // A box over the patch and well inside the ground: the patch.
        // Unlocked, the ground's fill is touched too.
        assert_eq!(s.in_box(P::new(-190.0, -90.0), P::new(-60.0, 40.0)), [1]);
        // A box over both small ones.
        assert_eq!(s.in_box(P::new(-190.0, -90.0), P::new(190.0, 40.0)), [1, 2]);
        // One that only catches a corner of the badge.
        assert_eq!(s.in_box(P::new(190.0, 40.0), P::new(260.0, 120.0)), [2]);
        // And one over bare ground: nothing.
        assert_eq!(
            s.in_box(P::new(-280.0, 120.0), P::new(-200.0, 170.0)),
            Vec::<usize>::new()
        );
        s.run_action(Action::SelectAll);
        assert_eq!(s.sel_indices(), [1, 2]);
        s.replan();
        assert!(s.design.per_shape[0] > 0);
        // Lock (⌘2) locks what's selected and lets go of it; Unlock All
        // brings everything back.
        assert!(s.action_enabled(Action::Lock) && s.action_enabled(Action::UnlockAll));
        s.run_action(Action::Lock);
        assert!(s.sel_indices().is_empty());
        assert!(s.flagged(&[0, 1, 2], Flag::Lock));
        assert!(!s.action_enabled(Action::Lock), "nothing selected");
        s.run_action(Action::UnlockAll);
        assert!(!s.doc.shapes.iter().any(|x| x.params.locked()));
        assert!(!s.action_enabled(Action::UnlockAll));
        assert_eq!(s.in_box(P::new(-190.0, -90.0), P::new(-60.0, 40.0)), [0, 1]);
        // Hide (⌘3) and Show All, the same.
        s.set_selection(&[1]);
        s.run_action(Action::Hide);
        assert!(s.doc.shapes[1].params.hidden() && s.sel_indices().is_empty());
        assert_eq!(s.pick(on_patch, 5.0), Some(0), "through the hidden patch");
        s.run_action(Action::ShowAll);
        assert!(!s.doc.shapes.iter().any(|x| x.params.hidden()));
    }

    fn svg_names(d: &zigzag::document::Document) -> Vec<String> {
        zigzag::svg::display_names(&d.shapes)
    }

    #[test]
    fn an_object_answers_to_the_id_an_agent_gave_it() {
        // An agent that drew the design knows its objects by the ids it
        // wrote; ZigZag shows those with spaces, and without the settings.
        let mut s = Studio::with_file(None, None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="40mm" viewBox="0 0 60 40">
            <rect id="Face_mask_fill_a=-45" x="5" y="5" width="30" height="30" fill="#777e84"/>
            <rect id="Face_mask_shadow" x="8" y="8" width="20" height="20" fill="#25272b"/>
            <rect id="Left_eye_satin" x="40" y="5" width="12" height="3" fill="#c2c5c3"/>
        </svg>"##;
        let r = s.run_tool("add_svg", &json!({ "svg": svg }));
        assert!(!r.is_error, "{}", text(&r));
        assert_eq!(s.names, ["Face mask", "Face mask shadow", "Left eye"]);
        for (given, object) in [
            ("Face_mask", 0),
            ("face_mask", 0),
            ("Face mask", 0),
            ("Face_mask_fill", 0),
            ("Face_mask_fill_a=-45", 0),
            ("Face_mask_shadow", 1),
            ("Left_eye", 2),
            ("Left_eye_satin", 2),
            // (Part of a name still finds it, when only one has it.)
            ("shadow", 1),
        ] {
            assert_eq!(s.resolve(Some(&json!(given))), Ok(object), "{given}");
        }
        let r = s.run_tool(
            "set_object",
            &json!({ "object": "Face_mask", "kind": "fill", "angle": 30, "terse": true }),
        );
        assert!(!r.is_error, "{}", text(&r));
        assert_eq!(s.doc.shapes[0].params.angle, Some(30.0));
        // What isn't there still isn't, and says what is.
        let r = s.run_tool(
            "set_object",
            &json!({ "object": "Right_eye", "kind": "fill" }),
        );
        assert!(r.is_error && text(&r).contains("Objects: Face mask, Face mask shadow, Left eye"));
    }

    #[test]
    fn simplify_design_reports_what_it_comes_to() {
        let mut s = Studio::with_file(None, None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40">
            <rect id="Red" x="2" y="2" width="20" height="20" fill="#c80000"/>
            <rect id="Blue" x="26" y="2" width="20" height="20" fill="#0000c8"/>
            <rect id="Light_red" x="50" y="2" width="8" height="8" fill="#d7140a"/>
            <rect id="Speck" x="70" y="2" width="0.6" height="0.6" fill="#00a000"/>
        </svg>"##;
        let ok = |r: ToolResult| -> Value {
            assert!(!r.is_error, "{}", text(&r));
            serde_json::from_str(&text(&r)).unwrap()
        };
        let no = |r: ToolResult, why: &str| {
            assert!(r.is_error && text(&r).contains(why), "{}", text(&r));
        };
        ok(s.run_tool("add_svg", &json!({ "svg": svg })));
        let undo = s.undo.len();
        // A dry run says what it would come to, and changes nothing.
        let args = json!({ "threads": 2, "min_size_mm": 1, "dry_run": true });
        let v = ok(s.run_tool("simplify_design", &args));
        assert_eq!(v["before"]["threads"], 4);
        assert_eq!(v["after"]["threads"], 2);
        assert_eq!(v["removed_objects"], 1);
        assert_eq!(v["changed"], false);
        assert_eq!((s.doc.shapes.len(), s.undo.len()), (4, undo));
        // For real: the speck goes, the light red sews in the red, each
        // thread once; one step to undo.
        let v = ok(s.run_tool(
            "simplify_design",
            &json!({ "threads": 2, "min_size_mm": 1 }),
        ));
        assert_eq!(v["changed"], true);
        assert_eq!(v["after"]["threads"], 2);
        assert_eq!(v["after"]["thread_changes"], 1);
        assert_eq!(
            v["merged_threads"],
            json!([{ "from": "#d7140a", "into": "#c80000" }])
        );
        assert_eq!(s.doc.shapes.len(), 3);
        assert_eq!(s.undo.len(), undo + 1);
        assert!(v["after"]["stitches"].as_u64() < v["before"]["stitches"].as_u64());
        // Uniting: the light red square doesn't touch the red, so nothing
        // to unite; with one over the red's edge there is.
        let v = ok(s.run_tool("simplify_design", &json!({ "unite": "all" })));
        assert_eq!(
            (v["united_objects"].as_u64(), v["changed"].as_bool()),
            (Some(0), Some(false))
        );
        let more = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="20mm" viewBox="0 0 20 20">
            <rect id="Patch" width="20" height="20" fill="#c80000"/>
        </svg>"##;
        ok(s.run_tool("add_svg", &json!({ "svg": more, "x_mm": -30, "y_mm": 5 })));
        let n = s.doc.shapes.len();
        let v = ok(s.run_tool("simplify_design", &json!({ "unite": "all" })));
        assert!(v["united_objects"].as_u64() >= Some(1), "{v}");
        assert!(s.doc.shapes.len() < n);
        assert!(!s.run_tool("undo", &json!({})).is_error);
        assert!(!s.run_tool("undo", &json!({})).is_error);
        assert_eq!(s.doc.shapes.len(), 3);
        // Nothing more to do: nothing to undo either.
        let v = ok(s.run_tool("simplify_design", &json!({ "threads": 2 })));
        assert_eq!(v["changed"], false);
        assert_eq!(s.undo.len(), undo + 1);
        // What it's asked has to make sense.
        no(
            s.run_tool("simplify_design", &json!({ "threads": 0 })),
            "threads",
        );
        no(
            s.run_tool("simplify_design", &json!({ "min_size_mm": 50 })),
            "min_size_mm",
        );
        no(
            s.run_tool("simplify_design", &json!({ "connect_mm": -1 })),
            "connect_mm",
        );
        no(
            s.run_tool("simplify_design", &json!({ "group_colors": "yes" })),
            "group_colors",
        );
        no(
            s.run_tool("simplify_design", &json!({ "unite": true })),
            "unite",
        );
        // Undone: the design as it was.
        assert!(!s.run_tool("undo", &json!({})).is_error);
        assert_eq!(s.doc.shapes.len(), 4);
    }

    #[test]
    fn tools_do_what_they_say_at_the_edges() {
        // What a sweep of every tool against its description turned up.
        let mut s = Studio::with_file(None, None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="40mm" viewBox="0 0 60 40">
            <rect id="Big_patch" x="5" y="5" width="30" height="20" fill="#203040"/>
            <rect id="Small_patch" x="40" y="5" width="12" height="12" fill="#c00000"/>
        </svg>"##;
        let ok = |r: ToolResult| {
            assert!(!r.is_error, "{}", text(&r));
            text(&r)
        };
        let no = |r: ToolResult, why: &str| {
            assert!(r.is_error && text(&r).contains(why), "{}", text(&r));
        };
        ok(s.run_tool("add_svg", &json!({ "svg": svg })));

        // Lettering that's been scaled can still be edited (each edit after
        // one that set h_scale failed, with two transforms on the element).
        ok(s.run_tool("add_text", &json!({ "text": "CAMP", "y_mm": 30 })));
        let text_obj = s.names.len() - 1;
        ok(s.run_tool("edit_text", &json!({ "object": text_obj, "h_scale": 150 })));
        let wide = shape_bounds(&s.doc.shapes[text_obj]).width();
        ok(s.run_tool(
            "edit_text",
            &json!({ "object": text_obj, "color": "#00aa00" }),
        ));
        ok(s.run_tool(
            "transform_objects",
            &json!({ "objects": text_obj, "move_by_mm": [3, 0] }),
        ));
        ok(s.run_tool("edit_text", &json!({ "object": text_obj, "case": "upper" })));
        let still = shape_bounds(&s.doc.shapes[text_obj]).width();
        assert!((wide - still).abs() < 1.0, "{wide} {still}");
        // A setting of the wrong sort is said, not passed over.
        no(
            s.run_tool(
                "edit_text",
                &json!({ "object": text_obj, "size_mm": "big" }),
            ),
            "`size_mm` must be a number",
        );
        no(
            s.run_tool("edit_text", &json!({ "object": text_obj, "bold": "yes" })),
            "`bold` must be true or false",
        );
        // Fonts to set type in: not the system's own, whose names begin
        // with a dot.
        let fonts = ok(s.run_tool("list_fonts", &json!({})));
        let fonts: Value = serde_json::from_str(&fonts).unwrap();
        assert!(
            fonts["fonts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| !f.as_str().unwrap().starts_with('.'))
        );

        // A size of nothing isn't a size.
        no(
            s.run_tool("transform_objects", &json!({ "objects": 0, "width_mm": 0 })),
            "width_mm must be positive",
        );
        no(
            s.run_tool("render_preview", &json!({ "fit": "page" })),
            "fit must be design or hoop",
        );
        // One object, or a list, as for transform_objects.
        ok(s.run_tool("round_corners", &json!({ "objects": "Big_patch" })));
        ok(s.run_tool("simplify", &json!({ "objects": 1 })));
        // What set_document sets, get_design says.
        ok(s.run_tool(
            "set_document",
            &json!({ "connect_mm": 3, "over_sewn": true }),
        ));
        let d: Value = serde_json::from_str(&ok(s.run_tool("get_design", &json!({})))).unwrap();
        assert_eq!(d["document"]["connect_mm"], 3.0);
        assert_eq!(d["document"]["over_sewn"], true);
        // (And it's kept in the file, when it isn't the usual: cut.)
        assert!(s.doc.to_svg().unwrap().contains(" oversewn=1"));
        ok(s.run_tool("set_document", &json!({ "over_sewn": false })));
        assert!(!s.doc.to_svg().unwrap().contains("oversewn"));
        no(
            s.run_tool("set_document", &json!({ "over_sewn": "yes" })),
            "over_sewn must be true or false",
        );

        // Files: save writes SVG and nothing else; export without a format
        // goes by what the path ends in; a file that can't be opened says
        // what can.
        let dir = std::env::temp_dir().join(format!("zigzag-edges-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let at = |n: &str| dir.join(n).display().to_string();
        no(
            s.run_tool("save", &json!({ "path": at("x.txt") })),
            "must end in .svg",
        );
        assert!(!dir.join("x.txt").exists());
        ok(s.run_tool("export", &json!({ "path": at("x.dst") })));
        assert_eq!(&std::fs::read(dir.join("x.dst")).unwrap()[..3], b"LA:");
        no(
            s.run_tool("export", &json!({ "path": at("x.bin") })),
            "format is required",
        );
        // A format that's written but not read says so; VP3 is read.
        ok(s.run_tool("export", &json!({ "path": at("x.pen"), "format": "pen" })));
        no(
            s.run_tool("open_file", &json!({ "path": at("x.pen") })),
            "isn't a file ZigZag can open",
        );
        ok(s.run_tool("export", &json!({ "path": at("x.vp3"), "format": "vp3" })));
        ok(s.run_tool("open_file", &json!({ "path": at("x.vp3") })));
        let _ = std::fs::remove_dir_all(&dir);

        // A batch step that isn't {tool, arguments} says what a step is.
        no(
            s.run_tool(
                "batch",
                &json!({ "steps": [{ "tool": "set_object", "args": { "object": 0, "kind": "fill" } }] }),
            ),
            "a step doesn't take `args`",
        );
        no(
            s.run_tool("batch", &json!({ "steps": [{ "arguments": {} }] })),
            "a step needs a `tool`",
        );
    }

    fn http(port: u16, body: &str, origin: Option<&str>) -> (u16, String) {
        http_as(port, body, origin, "application/json")
    }

    fn http_as(port: u16, body: &str, origin: Option<&str>, kind: &str) -> (u16, String) {
        let mut sock = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let origin = origin
            .map(|o| format!("Origin: {o}\r\n"))
            .unwrap_or_default();
        write!(sock, "POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Type: {kind}\r\n{origin}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        let mut out = String::new();
        sock.read_to_string(&mut out).unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        // (A long reply, the tool list for one, comes in chunks: a length
        // in hex, that much text, and so on until a length of zero.)
        let chunked = head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked");
        let body = if chunked {
            let mut text = String::new();
            let mut rest = body;
            while let Some((len, after)) = rest.split_once("\r\n") {
                let n = usize::from_str_radix(len.trim(), 16).unwrap();
                if n == 0 {
                    break;
                }
                text.push_str(&after[..n]);
                rest = after[n..].trim_start_matches("\r\n");
            }
            text
        } else {
            body.to_string()
        };
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            body,
        )
    }

    /// A real JSON-RPC session over HTTP, with tool calls answered by the
    /// "UI thread" (this test's main thread) through `handle_mcp`.
    #[test]
    fn json_rpc_over_http() {
        let server = crate::mcp::start(0, tool_specs(), INSTRUCTIONS.into(), || {}).unwrap();
        let port = server.addr.port();
        let mut studio = Studio::with_file(None, None);
        studio.mcp = Some(server);

        let client = std::thread::spawn(move || {
            let (code, body) = http(
                port,
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
                None,
            );
            assert_eq!(code, 200);
            let v: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(v["result"]["serverInfo"]["name"], "zigzag");
            assert!(
                v["result"]["instructions"]
                    .as_str()
                    .unwrap()
                    .contains("millimetres")
            );

            let (code, body) = http(
                port,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                None,
            );
            assert_eq!((code, body.as_str()), (202, ""));

            let (_, body) = http(
                port,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                None,
            );
            let v: Value = serde_json::from_str(&body).unwrap();
            assert!(v["result"]["tools"].as_array().unwrap().len() >= 12);

            let call = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"add_svg","arguments":{"svg": BADGE, "width_mm": 40}}});
            let (_, body) = http(port, &call.to_string(), Some("http://localhost:5173"));
            let v: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(v["result"]["isError"], false, "{body}");

            let call = json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"render_preview","arguments":{"width_px": 300}}});
            let (_, body) = http(port, &call.to_string(), None);
            let v: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(v["result"]["content"][0]["type"], "image");
            assert_eq!(v["result"]["content"][0]["mimeType"], "image/png");

            // Other websites can't drive the studio.
            let (code, _) = http(
                port,
                r#"{"jsonrpc":"2.0","id":5,"method":"tools/list"}"#,
                Some("https://evil.example"),
            );
            assert_eq!(code, 403);
            // Nor from a sandboxed iframe (Origin: null), which any site can make.
            let export = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"export","arguments":{"format":"dst","path":"/tmp/zigzag-pwned.dst"}}}"#;
            let (code, _) = http(port, export, Some("null"));
            assert_eq!(code, 403);
            // Nor as a "simple" text post, which browsers send without a
            // CORS preflight (and charset parameters still count as JSON).
            let (code, _) = http_as(port, export, None, "text/plain");
            assert_eq!(code, 415);
            let (code, _) = http_as(
                port,
                r#"{"jsonrpc":"2.0","id":8,"method":"tools/list"}"#,
                None,
                "application/json; charset=utf-8",
            );
            assert_eq!(code, 200);
            assert!(
                !std::path::Path::new("/tmp/zigzag-pwned.dst").exists(),
                "nothing was written"
            );

            let (_, body) = http(port, r#"{"jsonrpc":"2.0","id":6,"method":"bogus"}"#, None);
            assert!(body.contains("-32601"));
        });
        while !client.is_finished() {
            studio.handle_mcp(None);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        client.join().unwrap();
        assert_eq!(
            studio.doc.shapes.len(),
            4,
            "the agent's artwork is on the canvas"
        );
    }

    /// A big design's edit, asked for a short answer, is planned on the
    /// worker: the UI thread goes on between frames, and the answer (with
    /// what the edit broke) goes once the plan is back.
    #[test]
    fn a_slow_designs_edit_is_answered_once_the_worker_has_planned_it() {
        let server = crate::mcp::start(0, tool_specs(), INSTRUCTIONS.into(), || {}).unwrap();
        let port = server.addr.port();
        let path =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg"));
        let mut studio = Studio::with_file(Some(path), None);
        studio.replan();
        studio.mcp = Some(server);
        // As if its last plan had been slow.
        studio.planner.slow = true;
        let sun = studio.names.iter().position(|n| n == "sun").unwrap();
        let before = studio.design.per_shape[sun];
        let ctx = eframe::egui::Context::default();
        let client = std::thread::spawn(move || {
            let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"set_object","arguments":{"object":"sun","spacing_mm":0.8,"terse":true}}});
            let (_, body) = http(port, &call.to_string(), None);
            // And the next call, which waits for it.
            let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_design","arguments":{}}});
            let (_, next) = http(port, &call.to_string(), None);
            (body, next)
        });
        let mut waited = false;
        while !client.is_finished() {
            studio.handle_mcp(Some(&ctx));
            waited |= studio.mcp_waiting.is_some();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let (body, next) = client.join().unwrap();
        assert!(waited, "the answer waited for the worker's plan");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["isError"], false, "{body}");
        let answer: Value =
            serde_json::from_str(v["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(answer["ok"], true);
        assert!(answer["new_problems"].is_array(), "{answer}");
        let v: Value = serde_json::from_str(&next).unwrap();
        assert_eq!(v["result"]["isError"], false, "{next}");
        // The plan it answered after is the edit's.
        assert!(!studio.planner.pending && studio.mcp_waiting.is_none());
        assert_ne!(studio.design.per_shape[sun], before);
    }

    /// "$N.name" names what step N added, when it added one object (as an
    /// agent writes it for combine_shapes, say); with more, the error says
    /// what the result has and how to name one.
    #[test]
    fn a_batch_step_names_the_one_object_an_earlier_step_added() {
        let mut s = Studio::with_file(None, None);
        let one = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><path id="leaf" fill="#336633" d="M0 0 H10 V10 H0 Z"/></svg>"##;
        let r = s.run_tool(
            "batch",
            &json!({ "steps": [
                { "tool": "add_svg", "arguments": { "svg": one, "width_mm": 20.0 } },
                { "tool": "set_object", "arguments": { "object": "$0.name", "color": "#f47b20" } },
            ] }),
        );
        ok(r);
        let leaf = s.names.iter().position(|n| n == "leaf").unwrap();
        assert_eq!(s.doc.shapes[leaf].color, [0xf4, 0x7b, 0x20]);
        // More than one: which isn't guessed.
        let r = s.run_tool(
            "batch",
            &json!({ "steps": [
                { "tool": "add_svg", "arguments": { "svg": BADGE, "width_mm": 20.0 } },
                { "tool": "set_object", "arguments": { "object": "$0.name", "angle": 30 } },
            ] }),
        );
        assert!(r.is_error);
        let e = text(&r);
        assert!(e.contains("`added`") && e.contains("$0.added.0.name"), "{e}");
    }

    #[test]
    fn misspelt_arguments_are_refused_with_what_the_tool_takes() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({ "svg": BADGE })));
        let r = s.run_tool("set_object", &json!({ "object": 0, "widht_mm": 2.0 }));
        assert!(r.is_error);
        let t = text(&r);
        assert!(t.contains("`widht_mm`") && t.contains("`width_mm`"), "{t}");
    }

    #[test]
    fn an_atomic_batch_is_one_undo_step_and_passes_results_on() {
        let mut s = Studio::with_file(None, None);
        let undo0 = s.undo.len();
        let r = ok(s.run_tool(
            "batch",
            &json!({
                "label": "Badge in satin",
                "steps": [
                    { "tool": "add_svg", "arguments": { "svg": BADGE } },
                    { "tool": "set_object", "arguments": { "object": "$0.added.2.name", "kind": "satin" } },
                    { "tool": "get_design" }
                ]
            }),
        ));
        assert_eq!(r["results"].as_array().unwrap().len(), 3);
        assert_eq!(s.undo.len(), undo0 + 1, "one undo step");
        let bar = r["results"][0]["result"]["added"][2]["name"]
            .as_str()
            .unwrap()
            .to_string();
        let i = s.names.iter().position(|n| *n == bar).unwrap();
        assert_eq!(s.doc.shapes[i].params.kind, Some(Kind::Satin));
        assert_eq!(s.status, "Badge in satin");
    }

    /// Later steps of a batch name objects as they are after the earlier ones.
    #[test]
    fn a_batch_step_after_a_delete_finds_objects_by_name() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({ "svg": BADGE })));
        let names = s.names.clone();
        assert!(names.len() >= 3);
        let (first, last) = (names[0].clone(), names.last().unwrap().clone());
        let uid = s.doc.shapes.last().unwrap().uid;
        ok(s.run_tool(
            "batch",
            &json!({ "steps": [
                { "tool": "delete_object", "arguments": { "object": first } },
                { "tool": "set_object", "arguments": { "object": last, "spacing_mm": 0.7 } }
            ] }),
        ));
        assert_eq!(s.doc.shapes.len(), names.len() - 1);
        let edited = s.doc.shapes.iter().find(|x| x.uid == uid).unwrap();
        assert_eq!(edited.params.spacing, Some(0.7));
        assert_eq!(s.names.len(), s.doc.shapes.len());
    }

    #[test]
    fn a_failing_atomic_batch_changes_nothing() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({ "svg": BADGE })));
        let (n, undo) = (s.doc.shapes.len(), s.undo.len());
        s.modified = true;
        let failed = |r: ToolResult| -> Value {
            assert!(r.is_error, "a failed batch is an error for MCP clients");
            serde_json::from_str(&text(&r)).unwrap()
        };
        let r = failed(s.run_tool(
            "batch",
            &json!({ "steps": [
                { "tool": "add_svg", "arguments": { "svg": BADGE } },
                { "tool": "set_object", "arguments": { "object": "nope", "kind": "satin" } }
            ] }),
        ));
        assert!(
            r["error"].as_str().unwrap().contains("Step 1 (set_object)"),
            "{r}"
        );
        assert_eq!(s.doc.shapes.len(), n, "the first step was rolled back");
        assert_eq!(s.undo.len(), undo);
        assert!(s.modified, "still unsaved");
        // Opening a file would clear the history: refused, nothing lost.
        let r = failed(s.run_tool(
            "batch",
            &json!({ "steps": [ { "tool": "open_file", "arguments": { "path": "/tmp/x.svg" } } ] }),
        ));
        assert!(r["error"].as_str().unwrap().contains("open_file"), "{r}");
        assert_eq!(s.undo.len(), undo);
        // "$$" is a literal dollar sign, not a step reference.
        let r = ok(s.run_tool(
            "batch",
            &json!({ "steps": [ { "tool": "add_text", "arguments": { "text": "$$5" } } ] }),
        ));
        let name = r["results"][0]["result"]["name"]
            .as_str()
            .unwrap()
            .to_string();
        let i = s.names.iter().position(|x| *x == name).unwrap();
        assert_eq!(s.doc.lettering(i).unwrap().text, "$5");
        s.run_tool("undo", &json!({}));
        // Not atomic: the first step stays.
        failed(s.run_tool(
            "batch",
            &json!({ "atomic": false, "steps": [
                { "tool": "add_svg", "arguments": { "svg": BADGE } },
                { "tool": "set_object", "arguments": { "object": "nope" } }
            ] }),
        ));
        assert_eq!(s.doc.shapes.len(), n * 2);
    }

    #[test]
    fn a_trace_keeps_to_the_designs_threads_and_says_what_it_costs() {
        let dir = std::env::temp_dir().join(format!("tl-palette-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("mark.png");
        // A red a shade off the design's, and a blue the design hasn't got.
        let img = vectorcraft_trace::Raster::from_fn(120, 60, |x, y| {
            if (10..50).contains(&x) && (10..50).contains(&y) {
                [215, 30, 40, 255]
            } else if (70..110).contains(&x) && (10..50).contains(&y) {
                [20, 50, 190, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        std::fs::write(&png, img.encode_png()).unwrap();
        let mut s = Studio::with_file(None, None);
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><rect id="Red" width="20" height="20" fill="#c8141e"/></svg>"##;
        ok(s.run_tool(
            "add_svg",
            &json!({ "svg": svg, "replace": true, "x_mm": -30 }),
        ));
        let r = ok(s.run_tool(
            "trace_image",
            &json!({ "path": png.to_str().unwrap(), "width_mm": 40.0, "x_mm": 20, "palette": "design" }),
        ));
        assert!(
            r["colors"].as_array().unwrap().contains(&json!("#c8141e")),
            "{r}"
        );
        assert_eq!(
            r["new_colors"].as_array().unwrap().len(),
            1,
            "only the blue: {r}"
        );
        assert!(r["trims"]["after"].as_u64().unwrap() >= r["trims"]["before"].as_u64().unwrap());
        // The short answer keeps what it costs.
        s.run_tool("undo", &json!({}));
        let r = ok(s.run_tool(
            "trace_image",
            &json!({ "path": png.to_str().unwrap(), "width_mm": 40.0, "palette": ["#c8141e", "#1432be"], "terse": true }),
        ));
        assert_eq!(r["new_colors"], json!([]), "{r}");
        assert!(r["trims"].is_object(), "{r}");
        assert!(
            s.run_tool(
                "trace_image",
                &json!({ "path": png.to_str().unwrap(), "palette": ["red"] })
            )
            .is_error
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trace_image_and_round_corners() {
        let dir = std::env::temp_dir().join(format!("tl-tools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("mark.png");
        let img = vectorcraft_trace::Raster::from_fn(120, 60, |x, y| {
            if (10..50).contains(&x) && (10..50).contains(&y) {
                [200, 20, 30, 255]
            } else if (70..110).contains(&x) && (10..50).contains(&y) {
                [20, 50, 190, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        std::fs::write(&png, img.encode_png()).unwrap();
        let mut s = Studio::with_file(None, None);
        let r = ok(s.run_tool(
            "trace_image",
            &json!({ "path": png.to_str().unwrap(), "width_mm": 60.0, "threads": "brother" }),
        ));
        assert_eq!(r["added"].as_array().unwrap().len(), 2, "{r}");
        assert_eq!(r["traced_as"], "flat logo");
        let names: Vec<String> = r["added"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["name"].as_str().unwrap().into())
            .collect();
        let r = s.run_tool(
            "round_corners",
            &json!({ "objects": names, "radius_mm": 2.0 }),
        );
        assert!(!r.is_error, "{}", text(&r));
        assert!(text(&r).contains("Rounded 8 corners"), "{}", text(&r));
        let p = s.doc.edit_path(0).unwrap();
        assert!(zigzag::bezier::path_corners(&p).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_outline_gives_a_shape_an_outline_to_set_up_on_its_own() {
        let mut s = Studio::with_file(None, None);
        ok(s.run_tool("add_svg", &json!({ "svg": BADGE })));
        let r = ok(s.run_tool(
            "add_outline",
            &json!({ "objects": ["bar", "stitchline"], "kind": "satin", "width_mm": 1.5, "color": "#ffb703" }),
        ));
        let added = r["added"].as_array().unwrap();
        assert_eq!(added.len(), 1, "{r}");
        assert_eq!(added[0]["name"], "bar (outline)");
        assert_eq!(added[0]["kind"], "satin");
        assert_eq!(added[0]["color"], "#ffb703");
        assert_eq!(added[0]["source"], "stroke");
        // Its thread and stitch type are its own.
        let r = ok(s.run_tool(
            "set_object",
            &json!({ "object": "bar (outline)", "kind": "run", "color": "#000000" }),
        ));
        assert_eq!(r["kind"], "run");
        let bar = s
            .doc
            .shapes
            .iter()
            .find(|o| o.id.starts_with("bar"))
            .unwrap();
        assert_eq!(bar.part, zigzag::svg::Part::Fill);
        assert_eq!(
            (bar.params.kind, bar.color),
            (Some(Kind::Fill), [0xe6, 0x39, 0x46])
        );
        // One undo step takes the outline away again.
        let n = s.doc.shapes.len();
        ok(s.run_tool("undo", &json!({})));
        ok(s.run_tool("undo", &json!({})));
        assert_eq!(s.doc.shapes.len(), n - 1);

        // The menu's Add Outline does the same for the selection.
        let patch = s
            .doc
            .shapes
            .iter()
            .position(|o| o.id.starts_with("patch"))
            .unwrap();
        s.select_index(Some(patch));
        s.run_action(crate::commands::Action::AddOutline);
        assert!(s.status.starts_with("Added an outline"), "{}", s.status);
        assert_eq!(s.sel_indices(), [patch], "the shape stays selected");
        assert_eq!(s.doc.shapes[patch + 1].part, zigzag::svg::Part::Stroke);
        assert_eq!(s.doc.shapes[patch + 1].params.kind, Some(Kind::Run));
        assert_eq!(s.outline_index(patch), Some(patch + 1));
        assert_eq!(
            s.outline_index(patch + 1),
            None,
            "an outline has none itself"
        );
        assert!(s.outline_on(patch) && !s.can_outline(patch));
        s.run_action(crate::commands::Action::AddOutline);
        assert!(
            s.status.starts_with("Couldn't add an outline"),
            "{}",
            s.status
        );

        // Deleting the shape (Delete, or an agent's delete_object) takes
        // its outline with it; one undo brings both back.
        let n = s.doc.shapes.len();
        s.select_index(Some(patch));
        s.delete_selection();
        assert_eq!(s.doc.shapes.len(), n - 2);
        assert!(s.status.starts_with("Deleted patch"), "{}", s.status);
        ok(s.run_tool("undo", &json!({})));
        assert_eq!(s.doc.shapes.len(), n);
        ok(s.run_tool("add_outline", &json!({ "objects": "bar" })));
        let n = s.doc.shapes.len();
        ok(s.run_tool("delete_object", &json!({ "object": "bar" })));
        assert_eq!(s.doc.shapes.len(), n - 2);
        assert!(s.doc.shapes.iter().all(|o| !o.id.starts_with("bar")));

        let r = s.run_tool("add_outline", &json!({ "objects": "stitchline" }));
        assert!(
            r.is_error && text(&r).contains("filled shape"),
            "{}",
            text(&r)
        );
        let r = s.run_tool("add_outline", &json!({ "objects": "ring", "width_mm": 0 }));
        assert!(r.is_error && text(&r).contains("width_mm"), "{}", text(&r));
    }

    #[test]
    fn tools_say_which_only_read() {
        let tools = tool_specs();
        let get = |n: &str| {
            tools
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == n)
                .unwrap()
                .clone()
        };
        assert_eq!(get("get_design")["annotations"]["readOnlyHint"], true);
        assert_eq!(get("render_preview")["annotations"]["readOnlyHint"], true);
        assert_eq!(get("delete_object")["annotations"]["destructiveHint"], true);
        assert_eq!(get("set_object")["annotations"]["readOnlyHint"], false);
    }

    #[test]
    fn rpc_runs_tool_calls_through_any_runner() {
        // The headless (stdio) server and the HTTP one share this.
        let mut s = Studio::with_file(None, None);
        let mut call = |name: String, args: Value| s.run_tool(&name, &args);
        let tools = tool_specs();
        let r = crate::mcp::handle_rpc(
            &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "add_svg", "arguments": { "svg": BADGE } } }),
            &tools,
            INSTRUCTIONS,
            &mut call,
        )
        .unwrap();
        assert_eq!(r["id"], 7);
        assert_eq!(r["result"]["isError"], false);
        assert_eq!(s.doc.shapes.len(), 4);
    }

    /// A panic inside a tool is a tool error, not a crash: the same guard
    /// the stitch planner has, on the agent boundary too.
    #[test]
    fn a_panicking_tool_is_an_error_not_a_crash() {
        let mut s = Studio::with_file(None, None);
        let r = s.run_tool_caught("__panic__", &json!({}));
        assert!(r.is_error, "{r:?}");
        assert!(text_of(&r).contains("panicked"), "{}", text_of(&r));
        // And through the dispatch agents actually use.
        let r = s.run_mcp_call("__panic__", &json!({}));
        assert!(r.is_error, "{r:?}");
        assert!(text_of(&r).contains("panicked"), "{}", text_of(&r));
    }

    /// What a panicking tool changed is rolled back, like an atomic
    /// batch's: the agent is told the call failed, so nothing it did
    /// should be left behind.
    #[test]
    fn a_panicking_tool_rolls_back_what_it_changed() {
        let mut s = Studio::with_file(None, None);
        s.run_tool("add_svg", &json!({ "svg": BADGE }));
        s.replan();
        let (name0, undo0, redo0, sel0, modified0) = (
            s.doc.name.clone(),
            s.undo.len(),
            s.redo.len(),
            s.selected,
            s.modified,
        );
        let r = s.run_tool_caught("__panic__", &json!({ "mutate": true }));
        assert!(r.is_error, "{r:?}");
        assert_eq!(s.doc.name, name0);
        assert_eq!(s.undo.len(), undo0);
        assert_eq!(s.redo.len(), redo0);
        assert_eq!(s.selected, sel0);
        assert_eq!(s.modified, modified0);
        // And the plan was rebuilt from the restored document.
        assert_eq!(s.names.len(), s.doc.shapes.len());
    }

    /// A panic can't leave `batching` set: it's put back to what it was,
    /// so a panicking batch doesn't break every later replan check.
    #[test]
    fn a_panicking_tool_restores_batching() {
        let mut s = Studio::with_file(None, None);
        s.batching = true;
        let _ = s.run_tool_caught("__panic__", &json!({}));
        assert!(s.batching, "restored to the pre-call value");
        s.batching = false;
        let _ = s.run_tool_caught("__panic__", &json!({}));
        assert!(!s.batching);
    }

    /// A tool that switched tabs before panicking can't be rolled back:
    /// the snapshot's document belongs to the other tab. Only `batching`
    /// is put back, and each tab keeps its own file.
    #[test]
    fn a_panicking_tab_switch_is_not_rolled_onto_the_wrong_doc() {
        let mut s = Studio::with_file(None, None);
        s.run_tool("add_svg", &json!({ "svg": BADGE }));
        s.replan();
        let r = s.run_tool_caught("__panic__", &json!({ "switch_tab": true }));
        assert!(r.is_error, "{r:?}");
        // The new tab is in front with its own empty document; the badge
        // stayed in the first tab's husk — the snapshot wasn't copied in.
        assert_eq!(s.tab_count(), 2);
        assert_eq!(s.active_tab(), 1);
        assert!(s.doc.shapes.is_empty(), "live doc is the new tab's");
        assert_eq!(s.tabs.list[0].doc.shapes.len(), 4);
    }
}
