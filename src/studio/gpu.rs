//! Stitches drawn by the GPU.
//!
//! Each color block's thread segments go into a vertex buffer once, in
//! design units, and stay there until the block changes (a new plan).
//! Every frame is then a handful of uniforms per block: panning, zooming,
//! dragging and scaling never touch the stitch data, and the stitch player
//! just draws fewer instances. The shader turns each segment into a quad
//! of the thread's width and shades it like a round thread (or a thin
//! contrasting line, in the Lines view), with antialiased edges.
//!
//! The painter's [`crate::canvas::draw_design_cached`] remains for the
//! tests (no OpenGL there) and for a context without the needed GL.

use crate::canvas::{DrawOptions, StitchView, View, color32, contrasting};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke};
use eframe::glow::{self, HasContext};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use zigzag::design::Design;
use zigzag::geom::P;

/// Frames a block's buffers may go unused before they're freed.
const KEEP_FRAMES: u64 = 240;

/// The GPU side of the stitch renderer: owned by the studio, shared with
/// each frame's paint callback.
pub struct StitchGpu {
    state: Arc<Mutex<GlState>>,
    /// Per slot (the design, the design as sent to the machine): each
    /// block's content hash, for the plan generation it was hashed from.
    keys: Vec<Option<(u64, Vec<u64>)>>,
    frame: u64,
}

struct GlState {
    gl: Arc<glow::Context>,
    vao: glow::VertexArray,
    /// The four corners of a quad, (end, side): the one per-vertex
    /// attribute, which every instance shares. (Deriving the corner from
    /// `gl_VertexID` with only instanced attributes enabled isn't
    /// reliable on every driver, Apple's among them.)
    corners: glow::Buffer,
    segs: Program,
    pts: Program,
    blocks: HashMap<u64, GlBlock>,
    /// Uploaded on the GL thread at the next paint.
    pending: Vec<(u64, Vec<f32>, Vec<f32>)>,
}

struct GlBlock {
    segs: glow::Buffer,
    n_segs: i32,
    pts: glow::Buffer,
    n_pts: i32,
    used: u64,
}

struct Program {
    id: glow::Program,
    /// `a_corner` (per vertex), `a_a`, and `a_b` for the segment shader
    /// (per instance).
    attr_corner: u32,
    attr: u32,
    attr_b: u32,
    u_lin: Option<glow::UniformLocation>,
    u_origin: Option<glow::UniformLocation>,
    u_size: Option<glow::UniformLocation>,
    u_half: Option<glow::UniformLocation>,
    u_aa: Option<glow::UniformLocation>,
    u_px: Option<glow::UniformLocation>,
    u_shade: Option<glow::UniformLocation>,
    u_color: Option<glow::UniformLocation>,
}

/// One block's draw for a frame.
struct Cmd {
    key: u64,
    /// Segments (and points) to draw: fewer than the block has when the
    /// player is part way through it.
    segs: i32,
    pts: i32,
    color: [f32; 4],
    /// Design → canvas points: linear part (column major) and origin.
    lin: [f32; 4],
    origin: [f32; 2],
}

struct FrameDraw {
    cmds: Vec<Cmd>,
    size: [f32; 2],
    ppp: f32,
    /// Half the thread width, points.
    half: f32,
    /// Round-thread shading (else a flat line).
    shade: bool,
    /// Needle points' radius (points), in the Lines view when zoomed in.
    point_r: Option<f32>,
}

const SEG_VERT: &str = r#"
uniform mat2 u_lin;
uniform vec2 u_origin;
uniform vec2 u_size;
uniform float u_half;
uniform float u_px;
uniform float u_shade;
in vec2 a_corner;
in vec2 a_a;
in vec2 a_b;
out float v_t;
out float v_s;
flat out float v_len;
flat out float v_hi;
void main() {
    // The quad's corner: which end of the segment (0 or 1), which side (±1).
    float end = a_corner.x;
    float side = a_corner.y;
    vec2 sa = u_lin * a_a + u_origin;
    vec2 sb = u_lin * a_b + u_origin;
    vec2 d = sb - sa;
    float len = length(d);
    vec2 dir = len > 1e-5 ? d / len : vec2(1.0, 0.0);
    vec2 n = vec2(-dir.y, dir.x);
    // Each stitch a little shorter than its span, so the needle points
    // show between them (thread only); half a pixel past that for the
    // feathered ends.
    float inset = u_shade * min((u_half - 0.5 * u_px) * 0.6, len * 0.2);
    float along = end * 2.0 - 1.0;
    vec2 p = mix(sa, sb, end) + dir * (along * (0.5 * u_px - inset)) + n * (u_half * side);
    gl_Position = vec4(p.x / u_size.x * 2.0 - 1.0, 1.0 - p.y / u_size.y * 2.0, 0.0, 1.0);
    v_t = side;
    v_len = len - 2.0 * inset;
    v_s = end * (v_len + u_px) - 0.5 * u_px;
    // Light from the top left: the highlight sits on that side.
    v_hi = (n.x + n.y > 0.0) ? -0.28 : 0.28;
}
"#;

const SEG_FRAG: &str = r#"
uniform vec4 u_color;
uniform float u_aa;
uniform float u_px;
uniform float u_half;
uniform float u_shade;
in float v_t;
in float v_s;
flat in float v_len;
flat in float v_hi;
out vec4 f_color;
void main() {
    float t = abs(v_t);
    float a = (1.0 - smoothstep(1.0 - u_aa, 1.0, t)) * u_color.a;
    // Feathered ends, one pixel wide, centered on the stitch's end.
    a *= smoothstep(-0.5 * u_px, 0.5 * u_px, v_s) * smoothstep(-0.5 * u_px, 0.5 * u_px, v_len - v_s);
    vec3 c = u_color.rgb;
    // The painter's thread: a darker line at full width, the color at
    // 72% of it, and a lighter line 22% wide (at least 0.8 pt), offset
    // toward the light, over the middle 70% of the stitch.
    float edge = smoothstep(0.72 - u_aa, 0.72 + u_aa, t);
    c = mix(c, c * 0.5, edge * u_shade);
    float hw = max(0.22, 0.4 / u_half);
    float hi = 1.0 - smoothstep(hw - u_aa, hw + u_aa, abs(v_t - v_hi));
    float along = v_s / max(v_len, 1e-3);
    hi *= smoothstep(0.15 - u_px / v_len, 0.15, along) * (1.0 - smoothstep(0.85, 0.85 + u_px / v_len, along));
    c = mix(c, mix(u_color.rgb, vec3(1.0), 0.45), hi * u_shade * (1.0 - edge));
    f_color = vec4(c * a, a);
}
"#;

const PT_VERT: &str = r#"
uniform mat2 u_lin;
uniform vec2 u_origin;
uniform vec2 u_size;
uniform float u_half;
in vec2 a_corner;
in vec2 a_a;
out vec2 v_uv;
void main() {
    vec2 corner = vec2(a_corner.x * 2.0 - 1.0, a_corner.y);
    vec2 p = u_lin * a_a + u_origin + corner * u_half;
    gl_Position = vec4(p.x / u_size.x * 2.0 - 1.0, 1.0 - p.y / u_size.y * 2.0, 0.0, 1.0);
    v_uv = corner;
}
"#;

const PT_FRAG: &str = r#"
uniform vec4 u_color;
uniform float u_aa;
in vec2 v_uv;
out vec4 f_color;
void main() {
    float d = length(v_uv);
    float a = (1.0 - smoothstep(1.0 - u_aa, 1.0, d)) * u_color.a;
    f_color = vec4(u_color.rgb * a, a);
}
"#;

unsafe fn compile(gl: &glow::Context, vert: &str, frag: &str) -> Result<Program, String> {
    let version = eframe::egui_glow::ShaderVersion::get(gl);
    if !version.is_new_shader_interface() {
        return Err(format!("needs GLSL 1.40 or ES 3.00, have {version:?}"));
    }
    let prelude = match version {
        eframe::egui_glow::ShaderVersion::Es300 => "#version 300 es\nprecision highp float;\n",
        _ => "#version 140\n",
    };
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, src) in [(glow::VERTEX_SHADER, vert), (glow::FRAGMENT_SHADER, frag)] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, &format!("{prelude}{src}"));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let log = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                gl.delete_program(program);
                return Err(format!("shader: {log}"));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for s in shaders {
            gl.detach_shader(program, s);
            gl.delete_shader(s);
        }
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("link: {log}"));
        }
        let attr = gl
            .get_attrib_location(program, "a_a")
            .ok_or("no a_a attribute")?;
        let attr_b = gl.get_attrib_location(program, "a_b").unwrap_or(attr);
        let attr_corner = gl
            .get_attrib_location(program, "a_corner")
            .ok_or("no a_corner attribute")?;
        let u = |name: &str| gl.get_uniform_location(program, name);
        Ok(Program {
            id: program,
            attr_corner,
            attr,
            attr_b,
            u_lin: u("u_lin"),
            u_origin: u("u_origin"),
            u_size: u("u_size"),
            u_half: u("u_half"),
            u_aa: u("u_aa"),
            u_px: u("u_px"),
            u_shade: u("u_shade"),
            u_color: u("u_color"),
        })
    }
}

impl StitchGpu {
    /// Set up on the GL context (at app start). Err: the context can't
    /// do it, and the painter draws the stitches instead.
    pub fn new(gl: Arc<glow::Context>) -> Result<Self, String> {
        unsafe {
            let vao = gl.create_vertex_array()?;
            let corners = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(corners));
            let quad: [f32; 8] = [0.0, -1.0, 0.0, 1.0, 1.0, -1.0, 1.0, 1.0];
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&quad), glow::STATIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            let segs = compile(&gl, SEG_VERT, SEG_FRAG)?;
            let pts = match compile(&gl, PT_VERT, PT_FRAG) {
                Ok(p) => p,
                Err(e) => {
                    gl.delete_program(segs.id);
                    gl.delete_vertex_array(vao);
                    return Err(e);
                }
            };
            Ok(StitchGpu {
                state: Arc::new(Mutex::new(GlState {
                    gl,
                    vao,
                    corners,
                    segs,
                    pts,
                    blocks: HashMap::new(),
                    pending: Vec::new(),
                })),
                keys: vec![None, None],
                frame: 0,
            })
        }
    }

    /// Free everything on the GL context (app exit).
    pub fn destroy(&mut self) {
        if let Ok(mut s) = self.state.lock() {
            unsafe {
                for b in s.blocks.values() {
                    s.gl.delete_buffer(b.segs);
                    s.gl.delete_buffer(b.pts);
                }
                s.blocks.clear();
                s.gl.delete_program(s.segs.id);
                s.gl.delete_program(s.pts.id);
                s.gl.delete_buffer(s.corners);
                s.gl.delete_vertex_array(s.vao);
            }
        }
    }

    /// Draw the design's stitches into `rect` (the painter's clip
    /// applies). `slot` tells designs apart (0: the document's plan, 1:
    /// the plan sent to the machine) and `generation` changes whenever
    /// that design does. Returns where the last stitch drawn is, for
    /// the needle.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        view: &View,
        d: &Design,
        opt: &DrawOptions,
        slot: usize,
        generation: u64,
    ) -> Option<Pos2> {
        self.frame += 1;
        if self.keys.len() <= slot {
            self.keys.resize(slot + 1, None);
        }
        let fresh = matches!(&self.keys[slot], Some((g, k)) if *g == generation && k.len() == d.blocks.len());
        if !fresh {
            self.keys[slot] = Some((generation, d.blocks.iter().map(block_key).collect()));
        }
        let keys = self.keys[slot]
            .as_ref()
            .map(|(_, k)| k.clone())
            .unwrap_or_default();

        let s = view.scale as f64 / 10.0;
        let w = (opt.thread_mm * view.scale).max(1.0);
        let lines = opt.view == StitchView::Lines;
        let ppp = painter.ctx().pixels_per_point();
        let point_r = (lines && view.scale > 10.0).then(|| (view.scale * 0.06).clamp(1.2, 2.2));
        let mut remaining = opt.limit;
        let mut cmds = Vec::with_capacity(d.blocks.len());
        let mut last: Option<Pos2> = None;
        let mut jumps: Vec<egui::Shape> = Vec::new();
        let jump_stroke = Stroke::new(1.0, Color32::from_gray(140));

        let mut state = self.state.lock().ok()?;
        let frame = self.frame;
        for (k, blk) in d.blocks.iter().enumerate() {
            if remaining == 0 {
                break;
            }
            if opt.hides_block(d, k) {
                remaining = remaining.saturating_sub(blk.runs.iter().map(|r| r.pts.len()).sum());
                last = None;
                continue;
            }
            // The gesture's transform, when this block is being moved.
            let m = match &opt.moving {
                Some((on, m)) if on.get(k).copied().unwrap_or(false) => *m,
                _ => zigzag::geom::IDENTITY,
            };
            let at = |p: P| view.to_screen(rect, zigzag::geom::affine_apply(m, p));
            let key = keys.get(k).copied().unwrap_or(0);
            if let Some(b) = state.blocks.get_mut(&key) {
                b.used = frame;
            } else if !state.pending.iter().any(|(pk, ..)| *pk == key) {
                let (segs, pts) = block_data(blk);
                state.pending.push((key, segs, pts));
            }
            // How much of the block the player shows, and the jumps
            // between its runs (thin dashed lines, drawn by the painter).
            let mut n_segs = 0i32;
            let mut n_pts = 0i32;
            for run in &blk.runs {
                if remaining == 0 {
                    break;
                }
                let n = run.pts.len().min(remaining);
                remaining -= n;
                n_segs += n.saturating_sub(1) as i32;
                n_pts += n as i32;
                if let (Some(a), Some(&b)) = (last, run.pts.first())
                    && opt.show_jumps
                {
                    let b = at(b);
                    if a.distance(b) > 1.0 {
                        jumps.extend(egui::Shape::dashed_line(&[a, b], jump_stroke, 4.0, 3.0));
                    }
                }
                last = n
                    .checked_sub(1)
                    .and_then(|i| run.pts.get(i))
                    .map(|&p| at(p))
                    .or(last);
            }
            let color = if lines {
                contrasting(blk.color, opt.background)
            } else {
                color32(blk.color)
            };
            let c = color.to_normalized_gamma_f32();
            // Design p → canvas point: C + s·(A·p + t − c), relative to rect.min.
            let cx = (rect.center().x - rect.min.x) as f64;
            let cy = (rect.center().y - rect.min.y) as f64;
            let (vc, t) = (view.center, (m[4], m[5]));
            cmds.push(Cmd {
                key,
                segs: n_segs,
                pts: n_pts,
                color: c,
                lin: [
                    (s * m[0]) as f32,
                    (s * m[1]) as f32,
                    (s * m[2]) as f32,
                    (s * m[3]) as f32,
                ],
                origin: [
                    (cx + s * (t.0 - vc.x)) as f32,
                    (cy + s * (t.1 - vc.y)) as f32,
                ],
            });
        }
        drop(state);

        let draw = FrameDraw {
            cmds,
            size: [rect.width(), rect.height()],
            ppp,
            // (Half a device pixel wider than nominal, as the painter's
            // feathered strokes are.)
            half: if lines {
                0.75 / ppp.max(0.5)
            } else {
                w / 2.0 + 0.5 / ppp.max(0.5)
            },
            shade: !lines && w >= 2.2,
            point_r,
        };
        let state = self.state.clone();
        let callback = eframe::egui_glow::CallbackFn::new(move |_info, _painter| {
            if let Ok(mut s) = state.lock() {
                unsafe { s.paint(&draw, frame) };
            }
        });
        painter.add(egui::Shape::Callback(egui::PaintCallback {
            rect,
            callback: Arc::new(callback),
        }));
        painter.extend(jumps);
        last
    }
}

/// A block's stitches, hashed (so its buffers can be kept while they're
/// the same).
fn block_key(blk: &zigzag::design::Block) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    blk.color.hash(&mut h);
    blk.runs.len().hash(&mut h);
    for r in &blk.runs {
        r.pts.len().hash(&mut h);
        for p in &r.pts {
            p.x.to_bits().hash(&mut h);
            p.y.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

/// A block's segments (a, b per segment, in sewing order) and its points.
fn block_data(blk: &zigzag::design::Block) -> (Vec<f32>, Vec<f32>) {
    let n: usize = blk.runs.iter().map(|r| r.pts.len()).sum();
    let mut segs = Vec::with_capacity(n * 4);
    let mut pts = Vec::with_capacity(n * 2);
    for r in &blk.runs {
        for s in r.pts.windows(2) {
            segs.extend_from_slice(&[s[0].x as f32, s[0].y as f32, s[1].x as f32, s[1].y as f32]);
        }
        for p in &r.pts {
            pts.extend_from_slice(&[p.x as f32, p.y as f32]);
        }
    }
    (segs, pts)
}

fn bytes(v: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding or invalid bit patterns; the slice is
    // read, never written, for the upload.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

impl GlState {
    unsafe fn upload(&self, data: &[f32]) -> Result<glow::Buffer, String> {
        unsafe {
            let buf = self.gl.create_buffer()?;
            self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(buf));
            self.gl
                .buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(data), glow::STATIC_DRAW);
            Ok(buf)
        }
    }

    /// On the GL thread, inside egui's paint: take in new blocks, free
    /// forgotten ones, draw this frame's.
    unsafe fn paint(&mut self, draw: &FrameDraw, frame: u64) {
        unsafe {
            let gl = self.gl.clone();
            gl.bind_vertex_array(Some(self.vao));
            for (key, segs, pts) in std::mem::take(&mut self.pending) {
                let (Ok(sb), Ok(pb)) = (self.upload(&segs), self.upload(&pts)) else {
                    continue;
                };
                self.blocks.insert(
                    key,
                    GlBlock {
                        segs: sb,
                        n_segs: (segs.len() / 4) as i32,
                        pts: pb,
                        n_pts: (pts.len() / 2) as i32,
                        used: frame,
                    },
                );
            }
            let stale: Vec<u64> = self
                .blocks
                .iter()
                .filter(|(_, b)| b.used + KEEP_FRAMES < frame)
                .map(|(k, _)| *k)
                .collect();
            for k in stale {
                if let Some(b) = self.blocks.remove(&k) {
                    gl.delete_buffer(b.segs);
                    gl.delete_buffer(b.pts);
                }
            }

            // Antialiasing: one device pixel, as a fraction of the half width.
            let px = 1.0 / draw.ppp.max(0.5);
            let aa = (px / draw.half).clamp(0.05, 1.0);
            let p = &self.segs;
            gl.use_program(Some(p.id));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.corners));
            gl.enable_vertex_attrib_array(p.attr_corner);
            gl.vertex_attrib_divisor(p.attr_corner, 0);
            gl.vertex_attrib_pointer_f32(p.attr_corner, 2, glow::FLOAT, false, 8, 0);
            gl.uniform_2_f32(p.u_size.as_ref(), draw.size[0], draw.size[1]);
            gl.uniform_1_f32(p.u_half.as_ref(), draw.half);
            gl.uniform_1_f32(p.u_aa.as_ref(), aa);
            gl.uniform_1_f32(p.u_px.as_ref(), px);
            gl.uniform_1_f32(p.u_shade.as_ref(), if draw.shade { 1.0 } else { 0.0 });
            gl.enable_vertex_attrib_array(p.attr);
            gl.enable_vertex_attrib_array(p.attr_b);
            gl.vertex_attrib_divisor(p.attr, 1);
            gl.vertex_attrib_divisor(p.attr_b, 1);
            for c in &draw.cmds {
                let Some(b) = self.blocks.get(&c.key) else {
                    continue;
                };
                if c.segs <= 0 {
                    continue;
                }
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(b.segs));
                gl.vertex_attrib_pointer_f32(p.attr, 2, glow::FLOAT, false, 16, 0);
                gl.vertex_attrib_pointer_f32(p.attr_b, 2, glow::FLOAT, false, 16, 8);
                gl.uniform_matrix_2_f32_slice(p.u_lin.as_ref(), false, &c.lin);
                gl.uniform_2_f32(p.u_origin.as_ref(), c.origin[0], c.origin[1]);
                gl.uniform_4_f32(
                    p.u_color.as_ref(),
                    c.color[0],
                    c.color[1],
                    c.color[2],
                    c.color[3],
                );
                gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, c.segs.min(b.n_segs));
            }
            gl.vertex_attrib_divisor(p.attr, 0);
            gl.vertex_attrib_divisor(p.attr_b, 0);
            gl.disable_vertex_attrib_array(p.attr);
            gl.disable_vertex_attrib_array(p.attr_b);
            gl.disable_vertex_attrib_array(p.attr_corner);

            if let Some(r) = draw.point_r {
                let p = &self.pts;
                gl.use_program(Some(p.id));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.corners));
                gl.enable_vertex_attrib_array(p.attr_corner);
                gl.vertex_attrib_divisor(p.attr_corner, 0);
                gl.vertex_attrib_pointer_f32(p.attr_corner, 2, glow::FLOAT, false, 8, 0);
                gl.uniform_2_f32(p.u_size.as_ref(), draw.size[0], draw.size[1]);
                gl.uniform_1_f32(p.u_half.as_ref(), r);
                gl.uniform_1_f32(p.u_aa.as_ref(), (px / r).clamp(0.05, 1.0));
                gl.enable_vertex_attrib_array(p.attr);
                gl.vertex_attrib_divisor(p.attr, 1);
                for c in &draw.cmds {
                    let Some(b) = self.blocks.get(&c.key) else {
                        continue;
                    };
                    if c.pts <= 0 {
                        continue;
                    }
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(b.pts));
                    gl.vertex_attrib_pointer_f32(p.attr, 2, glow::FLOAT, false, 8, 0);
                    gl.uniform_matrix_2_f32_slice(p.u_lin.as_ref(), false, &c.lin);
                    gl.uniform_2_f32(p.u_origin.as_ref(), c.origin[0], c.origin[1]);
                    gl.uniform_4_f32(
                        p.u_color.as_ref(),
                        c.color[0],
                        c.color[1],
                        c.color[2],
                        c.color[3],
                    );
                    gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, c.pts.min(b.n_pts));
                }
                gl.vertex_attrib_divisor(p.attr, 0);
                gl.disable_vertex_attrib_array(p.attr);
                gl.disable_vertex_attrib_array(p.attr_corner);
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_vertex_array(None);
        }
    }
}
