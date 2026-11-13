//! MCP tools: what agents can do with the live studio document.

use crate::app::{HOOPS, SPM, Studio, shape_bounds};
use crate::mcp::{Content, ToolResult};
use crate::widgets::{allowed_kinds, kind_name};
use serde_json::{Value, json};
use std::path::PathBuf;
use zigzag::checks::{Issue, Level};
use zigzag::document::{Document, Placement};
use zigzag::formats::preview::{PreviewOptions, render_png};
use zigzag::geom::P;
use zigzag::params::{FABRICS, Kind, Params, parse_hex, underlay};
use zigzag::svg::{Geometry, Part};

pub const INSTRUCTIONS: &str = "ZigZag is a machine-embroidery editor showing live documents on a hoop, one per tab; the tools work on the one in front (list_documents, switch_document, new_document). \
Coordinates are millimetres from the hoop center (+x right, +y down). The person watching sees every change, and each tool call is one undo step. \
Typical flow: get_design → add_svg (any SVG artwork) and add_text (lettering) → transform_objects to size and place → set_object to choose stitch types → render_preview to look at the stitches → check_design → save, or export (machine file or html worksheet). \
Stitch types: fill (tatami rows; areas wider than ~7 mm), satin (glossy zigzag columns 1–7 mm wide; works on strokes, and on narrow filled shapes like letters, bars and rings), run (thin running stitch for details), \
contour (rows following the outline inward), blanket (E-stitch edge), applique (placement line, stop, tack-down, stop, satin cover) and manual (every path point is a needle point). \
Settings can also be preset in SVG element ids, e.g. id=\"logo_fill_a=45\" or id=\"border_satin_w=2\". \
An object is named for its id without the settings (id=\"Face_mask_fill\" is \"Face mask\"); name it to a tool by that name, by the id, or by its index. \
list_machines and set_document's machine set a document up for a machine: its hoop, the file it reads, its shortest stitch and its lock stitches. \
Hat designs are small (about 50 × 50 mm on the front of a cap): keep satin columns under 7 mm wide, letters at least 5 mm tall, and details at least 1.5 mm. \
open_file also opens machine files (pes, pec, dst, exp, jef) as manual stitches. \
trace_image turns a logo (PNG, JPEG, WebP, GIF, BMP or TIFF) into shapes (snap colors to Brother or Janome threads with `threads`). round_corners softens sharp corners, which stitch more cleanly; simplify takes extra points out of outlines (traced ones especially) and clean_up removes stray points, objects too small to sew and extra points across the design. \
Use batch to make several changes in one call: it is all or nothing and one undo step, each step is {\"tool\": name, \"arguments\": {…}}, and later steps can use earlier results (\"$0.added.0.name\"). \
Tools that change the design return the whole object or document; pass terse: true (on a tool, or on a batch for all its steps) for a short answer instead: ok, the names of objects added, and new_problems, the design problems that weren't there before.";

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
            "description": "Describe the current document: every object (index, name, stitch type, color, position and size in mm, effective settings, stitch count, problems), design stats, fabric, hoop and options.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "add_svg",
            "description": "Add SVG artwork to the canvas (or replace the document). Every filled shape and stroke becomes an object; fills default to fill stitch, strokes to satin (≥1 mm) or run. Settings in element ids apply (e.g. id=\"leaf_satin\"). A satin can be given as its two edges: one stroked path (any stroke width, id with satin) whose two longest subpaths are the column's rails, with any shorter subpaths crossing both as rungs that keep the rails in step and set which way the stitches run, e.g. d=\"M4 10 C12 2 28 2 36 10 M4 10 C12 18 28 18 36 10 M14 5 L18 15\". By default the artwork keeps its own size (viewBox units scaled by width/height attributes, else 1 unit = 1 px) and is centered on the hoop.",
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
                    "kind": { "type": "string", "enum": ["fill", "satin", "run", "contour", "blanket", "applique", "meander", "ripple", "cross", "tartan", "manual", "skip"] },
                    "angle": { "type": ["number", "null"], "description": "Fill row direction in degrees (0 = horizontal, 90 = vertical)." },
                    "spacing_mm": { "type": ["number", "null"], "description": "Fill/contour row spacing, satin zigzag spacing, or blanket prong spacing." },
                    "length_mm": { "type": ["number", "null"], "description": "Max stitch length for fill and run." },
                    "pull_mm": { "type": ["number", "null"], "description": "Pull compensation per side." },
                    "width_mm": { "type": ["number", "null"], "description": "Satin width (strokes and borders), appliqué cover width, or blanket prong length." },
                    "split_mm": { "type": ["number", "null"], "description": "Satin auto-split: split stitches longer than this (0 = off; default 7)." },
                    "lean": { "type": ["number", "null"], "description": "Satin: how far the stitches lean from square across the column, degrees (−60 to 60; default 0). A few degrees keeps stitches from sinking between a knit's rows; more shows a slant." },
                    "border": { "type": ["boolean", "null"], "description": "Satin on a filled shape: true = along its outline, false = across it as a column." },
                    "spiral": { "type": ["boolean", "null"], "description": "Contour fill as one spiral out from the center (or the start point) instead of rings." },
                    "underlay": { "type": ["boolean", "null"] },
                    "satin_underlay": { "description": "Satin underlay passes, or \"auto\".", "oneOf": [{ "type": "array", "items": { "type": "string", "enum": ["center", "contour", "zigzag"] } }, { "type": "string", "enum": ["auto"] }] },
                    "short_stitches": { "type": ["boolean", "null"] },
                    "repeats": { "type": ["integer", "null"], "description": "Running stitch passes over the line, 1 to 9: 1 is a single run, 3 a triple run (bolder). An even number ends back where it started." },
                    "color": { "type": ["string", "null"], "description": "Thread color #rrggbb, or null for the SVG's color." },
                    "start": { "type": ["number", "null"], "description": "Where sewing starts: 0–1 along the object's outline (fills: the outer ring from its first point; lines: from their start). Null: the planner chooses." },
                    "end": { "type": ["number", "null"], "description": "Where sewing ends, 0–1 along the outline (the thread goes on from there to the next object)." },
                    "trim": { "type": ["boolean", "null"], "description": "Cut the thread after this object even if the next one is close." },
                    "stop": { "type": ["boolean", "null"], "description": "Pause the machine after this object (to lay foam, or trim appliqué fabric)." },
                    "hidden": { "type": ["boolean", "null"], "description": "Hide the object: it isn't shown or sewn, and keeps its stitch settings for when it's shown again." },
                    "locked": { "type": ["boolean", "null"], "description": "Lock the object: the person can't select or move it on the canvas (a background, so dragging over it selects what's on top). It sews as usual, and these tools still change it." }
                },
                "required": ["object"]
            }
        }),
        json!({
            "name": "delete_object",
            "description": "Delete an object.",
            "inputSchema": { "type": "object", "properties": { "object": obj_schema() }, "required": ["object"] },
            "annotations": { "destructiveHint": true }
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
            "description": "Open a file in a new tab (or the empty tab in front), which becomes the document the other tools work on: an SVG, or a machine file (pes, pec, dst, exp, jef) which opens as manual stitches you can move, scale and recolor. A file that's already open comes to the front instead, keeping its unsaved changes.",
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
                    "preview_color": { "type": "string", "description": "Fabric color for previews, #rrggbb (e.g. the hat's color)." }
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
            "description": "Render the stitches as they'll be sewn (PNG), on the preview fabric color with the hoop outline.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "width_px": { "type": "integer", "description": "Image width, 200–2000. Default 900." },
                    "show_jumps": { "type": "boolean", "description": "Draw jump stitches. Default true." },
                    "fit": { "type": "string", "enum": ["design", "hoop"], "description": "Frame the design closely or show the whole hoop. Default design." }
                }
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "check_design",
            "description": "List problems an experienced digitizer would flag (sizes, satin widths, hoop fit, lettering size).",
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
            "description": "Trace a raster logo (png, jpg, webp, gif, bmp, tiff) into stitchable shapes and add them (or start a new document with them). Flat logos trace exactly (edges, corners, circles); photos are posterized. Use `threads` to snap every color to a thread chart.",
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
            "name": "batch",
            "description": "Run several tool calls in one go, in order. Atomic by default: if any step fails nothing changes, and the whole batch is one undo step. A string argument \"$N.field\" (e.g. \"$0.added.0.name\") is replaced by that field of step N's JSON result; write \"$$\" for a literal \"$\". A step is {\"tool\": name, \"arguments\": {…}}. open_file, new_document, switch_document and undo can't run in an atomic batch, nor a batch in a batch. The stitches are planned once, at the end (and before any step that reads them: get_design, render_preview, check_design, export), so stitch counts in other steps' results are from before the batch.",
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
fn only_reads(name: &str) -> bool {
    matches!(
        name,
        "get_design"
            | "list_fonts"
            | "render_preview"
            | "check_design"
            | "get_svg"
            | "list_documents"
            | "list_machines"
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

/// The names of the objects a result says were added, if it says.
fn added_names(result: &Value) -> Vec<Value> {
    result["added"]
        .as_array()
        .into_iter()
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
            for key in path.split('.').filter(|k| !k.is_empty()) {
                cur = match key.parse::<usize>() {
                    Ok(i) if cur.is_array() => cur.get(i),
                    _ => cur.get(key),
                }
                .ok_or(format!("{s}: step {n}'s result has no `{key}`"))?;
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
                    [] => Err(format!(
                        "no object named '{s}'. Objects: {}",
                        self.names.join(", ")
                    )),
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
                let ul = p.satin_underlay_for(w);
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
            "kind": kind.name(),
            "allowed_kinds": allowed_kinds(s).iter().map(|k| k.name()).collect::<Vec<_>>(),
            "lettering": self.doc.lettering(i).map(|t| json!({ "text": t.text, "font": t.font, "size_mm": t.size_mm, "spacing_mm": t.spacing_mm, "arc_mm": t.arc_mm, "bold": t.bold, "italic": t.italic, "leading": t.leading, "align": t.align.name(), "h_scale": t.h_scale, "v_scale": t.v_scale, "case": t.case.name(), "on_path": t.path.is_some(), "path_offset": t.path_offset, "path_flip": t.path_flip, "kern": t.kern.iter().map(|(k, v)| json!({ "at": k, "mm": v })).collect::<Vec<_>>() })),
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

    fn design_json(&self) -> Value {
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
                "minutes": (total as f64 / SPM).ceil(),
            },
            "objects": (0..self.doc.shapes.len()).map(|i| self.object_json(i)).collect::<Vec<_>>(),
            "design_problems": self.issues.iter().filter(|x| x.shape.is_none()).map(|x| issue_json(x, &self.names)).collect::<Vec<_>>(),
        })
    }

    /// Run one MCP tool against the live document.
    pub fn run_tool(&mut self, name: &str, args: &Value) -> ToolResult {
        if let Err(e) = check_args(name, args) {
            return ToolResult::error(e);
        }
        // Inside a batch, only a step that reads the stitches waits for them.
        let reads = matches!(
            name,
            "get_design" | "render_preview" | "check_design" | "export" | "send_to_skitch"
        );
        if reads {
            self.plan_now();
        }
        if (reads || !self.batching) && self.dirty_needs_plan() {
            self.replan();
        }
        // A short answer was asked for: what's wrong now, to tell what's new.
        let terse = args.get("terse").and_then(Value::as_bool) == Some(true);
        let before = (terse && name != "batch").then(|| self.problems());
        let result = match name {
            "batch" => return self.tool_batch(args),
            "trace_image" => self.tool_trace_image(args),
            "round_corners" => self.tool_round_corners(args),
            "simplify" => self.tool_simplify(args),
            "clean_up" => self.tool_clean_up(args),
            "get_design" => Ok(ToolResult::json(&self.design_json())),
            "add_svg" => self.tool_add_svg(args),
            "set_object" => self.tool_set_object(args),
            "delete_object" => self.resolve(args.get("object")).and_then(|i| {
                let name = self.names[i].clone();
                self.structural(|d| d.delete(i))
                    .map_err(|e| e.to_string())?;
                self.selected = self
                    .selected
                    .filter(|&u| self.doc.shapes.iter().any(|s| s.uid == u));
                Ok(ToolResult::text(format!("Deleted {name}.")))
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
                    if let Some((src, rel, c)) = on {
                        spec.path = Some(rel);
                        (x, y) = (c.x / 10.0, c.y / 10.0);
                        self.doc.shapes[src].params.kind = Some(Kind::Skip);
                    }
                    let i = self
                        .add_lettering(&spec, (x, y))
                        .map_err(|e| format!("{e:#}"))?;
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
                if args.get("on_path").is_some_and(Value::is_null) {
                    spec.path = None;
                } else if let Some((src, rel, _)) = self.path_for_text(args.get("on_path"))? {
                    spec.path = Some(rel);
                    self.doc.shapes[src].params.kind = Some(Kind::Skip);
                }
                let uid = self.doc.shapes[i].uid;
                self.update_lettering(i, &spec)
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
                    self.selected = None;
                    Ok(ToolResult::text("Selection cleared."))
                }
                v => self.resolve(v).map(|i| {
                    self.select_index(Some(i));
                    ToolResult::text(format!("Selected {}.", self.names[i]))
                }),
            },
            "undo" => {
                if self.apply_history(crate::app::History::Undo) {
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
                self.save(path.as_deref());
                if self.modified {
                    Err(self.status.clone())
                } else {
                    Ok(ToolResult::text(self.status.clone()))
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
                match path {
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
    fn problems(&self) -> Vec<(Option<String>, String)> {
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
    fn new_problems(&self, before: &[(Option<String>, String)]) -> Vec<Value> {
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
            self.redo.clone(),
            self.selected,
            self.modified,
        );
        let mut results: Vec<Value> = Vec::new();
        let mut report = Vec::new();
        let mut images = Vec::new();
        for (k, step) in steps.iter().enumerate() {
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
                        "undo" | "open_file" | "switch_document" | "new_document"
                    ))
            {
                Err(format!("`{tool}` can't run inside an atomic batch."))
            } else {
                resolve_refs(step.get("arguments").unwrap_or(&json!({})), &results)
                    .map(|a| self.run_tool(tool, &a))
            };
            let r = outcome.unwrap_or_else(ToolResult::error);
            if r.is_error {
                let msg = failed(text_of(&r));
                if atomic {
                    self.doc = doc0;
                    self.undo.truncate(undo0);
                    self.redo = redo0;
                    self.stable = self.doc.clone();
                    self.selected = sel0;
                    self.modified = modified0;
                    self.dirty = true;
                    self.replan();
                }
                let mut r = ToolResult::json(&json!({ "error": msg, "results": report }));
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
        let objs: Vec<Value> = added.iter().map(|&i| self.object_json(i)).collect();
        Ok(ToolResult::json(&json!({
            "traced_as": t.method,
            "colors": t.colors.iter().map(|&c| hex(c)).collect::<Vec<_>>(),
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
        let redo = self.redo.clone();
        self.structural(|d| {
            removed = d.simplify_paths(&idx, tol)?;
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))?;
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
        let redo = self.redo.clone();
        self.structural(|d| {
            r = d.clean_up(&opt, false)?;
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))?;
        if r.total() == 0 {
            self.undo.pop();
            self.redo = redo;
        } else {
            self.replan_soon();
        }
        Ok(ToolResult::text(crate::path_tools::cleanup_summary(&r)))
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
        let redo = self.redo.clone();
        self.structural(|d| {
            n = d.round_corners(&idx, r.min(50.0) * 10.0, kind, &[])?;
            Ok(())
        })
        .map_err(|e| format!("{e:#}"))?;
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
            p.spacing = v.map(|x| x.clamp(0.1, 2.0));
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
        if let Some(v) = num("split_mm")? {
            p.split = v.map(|x| x.max(0.0));
        }
        if let Some(v) = num("lean")? {
            p.lean = v
                .map(|x| x.clamp(-zigzag::params::MAX_LEAN, zigzag::params::MAX_LEAN))
                .filter(|x| *x != 0.0);
        }
        if let Some(v) = num("repeats")? {
            p.repeats = v.map(|x| x.clamp(1.0, 9.0) as u32);
        }
        if let Some(v) = num("start")? {
            p.start = v.map(|x| x.rem_euclid(1.0));
        }
        if let Some(v) = num("end")? {
            p.end = v.map(|x| x.rem_euclid(1.0));
        }
        if let Some(v) = flag("trim")? {
            p.trim = v.filter(|&x| x);
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

    pub(crate) fn resolve_many(&self, v: Option<&Value>) -> Result<Vec<usize>, String> {
        match v {
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
        if let Some(v) = args.get("preview_color") {
            s.cloth = v
                .as_str()
                .and_then(parse_hex)
                .ok_or("preview_color must look like #rrggbb")?;
        }
        self.structural(|d| {
            d.settings = s;
            Ok(())
        })
        .map_err(|e| e.to_string())?;
        self.replan_soon();
        Ok(ToolResult::json(&self.design_json()["document"]))
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
        let opt = PreviewOptions {
            hoop_mm: fit_hoop.then(|| self.hoop_dims()),
            background: self.doc.settings.cloth,
            show_jumps,
            thread_mm: 0.38,
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
        let text = format!(
            "{:.1} × {:.1} mm, {} stitches, {} colors. Sewing order: {}.",
            b.width() / 10.0,
            b.height() / 10.0,
            self.design.stitch_count(),
            self.design.blocks.len(),
            legend.join(" → ")
        );
        Ok(ToolResult {
            content: vec![Content::Image(png), Content::Text(text)],
            is_error: false,
        })
    }

    pub fn fit_view(&mut self) {
        self.request_fit();
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
        ok(s.run_tool("export", &json!({ "path": at("x.vp3"), "format": "vp3" })));
        no(
            s.run_tool("open_file", &json!({ "path": at("x.vp3") })),
            "isn't a file ZigZag can open",
        );
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
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            body.to_string(),
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
            studio.handle_mcp();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        client.join().unwrap();
        assert_eq!(
            studio.doc.shapes.len(),
            4,
            "the agent's artwork is on the canvas"
        );
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
}
