//! Frame-time benchmarks for the gestures that must feel smooth: typing
//! on the canvas and dragging objects, on a traced logo (hundreds of
//! shapes, tens of thousands of stitches). Each figure is a whole frame:
//! the UI pass plus egui's tessellation of what it painted, which is
//! where directly drawn stitches cost.
//!
//! `cargo test --release --features gui --bin zigzag-studio perf_bench -- --ignored --nocapture`

#![cfg(test)]

use crate::app::Studio;
use eframe::egui::{self, Vec2};
use egui_kittest::Harness;
use zigzag::geom::P;

fn studio_with_logo() -> Harness<'static, Studio> {
    let png = std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/agent-badge-preview.png"
    ));
    let mut h = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .build_ui_state(
            |ui, s: &mut Studio| s.frame(ui),
            Studio::with_file(None, None),
        );
    // As the app does it: a slow plan moves planning to the worker.
    h.state_mut().planner.timed = true;
    let doc = zigzag::document::Document::open(&png).unwrap();
    h.state_mut().set_document(doc);
    h.run_steps(5);
    h
}

/// One frame, tessellated: milliseconds.
fn frame_ms(
    h: &mut Harness<'static, Studio>,
    before: impl FnOnce(&mut Harness<'static, Studio>),
) -> f64 {
    before(h);
    let t = std::time::Instant::now();
    h.step();
    let shapes = h.output().shapes.clone();
    let _ = h.ctx.tessellate(shapes, 1.0);
    t.elapsed().as_secs_f64() * 1000.0
}

fn report(what: &str, ms: &[f64]) {
    let mean = ms.iter().sum::<f64>() / ms.len() as f64;
    let max = ms.iter().cloned().fold(0.0, f64::max);
    println!("{what:<34} mean {mean:6.2} ms   max {max:6.2} ms");
}

#[test]
#[ignore]
fn typing_and_dragging_on_a_traced_logo() {
    let mut h = studio_with_logo();
    println!(
        "{} shapes, {} stitches",
        h.state().doc.shapes.len(),
        h.state().design.stitch_count()
    );
    let rect = h.state().canvas_rect;

    // Idle, nothing happening.
    let idle: Vec<f64> = (0..10).map(|_| frame_ms(&mut h, |_| {})).collect();
    report("idle", &idle);

    // Type tool: click on the canvas, type a word.
    h.key_press(egui::Key::T);
    h.run_steps(1);
    let at = h.state().view.to_screen(rect, P::new(0.0, 0.0));
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(2);
    let mut keys = Vec::new();
    for ch in "CAMPFIRE".chars() {
        // The keystroke's frame plus the one after it (the replan lands
        // on the next frame).
        let a = frame_ms(&mut h, |h| h.event(egui::Event::Text(ch.to_string())));
        let b = frame_ms(&mut h, |_| {});
        keys.push(a + b);
    }
    report("keystroke (2 frames)", &keys);
    let idle_typing: Vec<f64> = (0..10).map(|_| frame_ms(&mut h, |_| {})).collect();
    report("idle while typing", &idle_typing);
    // Mouse moving over the canvas while typing (hover repaints).
    let hover_typing: Vec<f64> = (0..10)
        .map(|k| {
            frame_ms(&mut h, |h| {
                h.hover_at(at + Vec2::new(40.0 + k as f32 * 5.0, 60.0))
            })
        })
        .collect();
    report("hover while typing", &hover_typing);
    if std::env::var_os("TL_PROFILE").is_some() {
        eprintln!("-- profiled: a keystroke, the frame after, then idle while typing");
        let _ = frame_ms(&mut h, |h| h.event(egui::Event::Text("S".into())));
        let _ = frame_ms(&mut h, |_| {});
        let _ = frame_ms(&mut h, |_| {});
        let _ = frame_ms(&mut h, |_| {});
    }
    h.key_press(egui::Key::Escape);
    h.run_steps(2);

    // Selection tool: drag the biggest shape.
    h.key_press(egui::Key::V);
    h.run_steps(1);
    let big = (0..h.state().doc.shapes.len())
        .max_by_key(|&i| {
            h.state()
                .doc
                .edit_path(i)
                .map(|p| p[0].anchors.len())
                .unwrap_or(0)
        })
        .unwrap();
    h.state_mut().set_selection(&[big]);
    h.run_steps(2);
    let c = h.state().view.to_screen(
        rect,
        crate::app::shape_bounds(&h.state().doc.shapes[big]).center(),
    );
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    let drag: Vec<f64> = (1..=20)
        .map(|k| {
            frame_ms(&mut h, |h| {
                h.hover_at(c + Vec2::new(k as f32 * 3.0, k as f32))
            })
        })
        .collect();
    report("drag biggest shape", &drag);
    h.drop_at(c + Vec2::new(60.0, 20.0));
    h.run_steps(3);

    // Drag everything.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(2);
    let n = h.state().sel_indices().len();
    let c = h
        .state()
        .view
        .to_screen(rect, h.state().selection_bounds().center());
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    let drag_all: Vec<f64> = (1..=20)
        .map(|k| frame_ms(&mut h, |h| h.hover_at(c + Vec2::new(k as f32 * 3.0, 0.0))))
        .collect();
    report(&format!("drag all ({n} selected)"), &drag_all);
    if std::env::var_os("TL_PROFILE").is_some() {
        eprintln!("-- profiled: dragging everything");
        let _ = frame_ms(&mut h, |h| h.hover_at(c + Vec2::new(70.0, 0.0)));
        let _ = frame_ms(&mut h, |h| h.hover_at(c + Vec2::new(80.0, 0.0)));
    }
    h.drop_at(c + Vec2::new(60.0, 0.0));
    h.run_steps(3);

    // Pan.
    h.key_press(egui::Key::H);
    h.run_steps(1);
    let p0 = rect.center();
    h.hover_at(p0);
    h.run_steps(1);
    h.drag_at(p0);
    h.run_steps(1);
    let pan: Vec<f64> = (1..=10)
        .map(|k| frame_ms(&mut h, |h| h.hover_at(p0 + Vec2::new(k as f32 * 4.0, 0.0))))
        .collect();
    report("pan", &pan);
    h.drop_at(p0 + Vec2::new(40.0, 0.0));
}

/// Frame times of the everyday gestures on designs of your own: every
/// `.svg` in the folder `ZIGZAG_DESIGNS` names. Each figure is the UI pass
/// of a frame (what [`Studio::frame`] takes; the app draws stitches on the
/// GPU, so the painter's tessellation of them here would say nothing).
/// With `TL_PROFILE=1`, each frame's sections go to stderr as well.
///
/// `ZIGZAG_DESIGNS=<folder> cargo test --release --features gui --bin zigzag-studio perf_bench::gestures -- --ignored --nocapture`
#[test]
#[ignore]
fn gestures_on_designs_of_your_own() {
    let dir = std::env::var("ZIGZAG_DESIGNS").expect("ZIGZAG_DESIGNS: a folder of .svg files");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "svg"))
        .collect();
    files.sort();
    for file in files {
        let name = file.file_stem().unwrap().to_string_lossy().into_owned();
        let mut h = Harness::builder()
            .with_size(Vec2::new(1600.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.state_mut().planner.timed = true;
        let doc = zigzag::document::Document::open(&file).unwrap();
        h.state_mut().set_document(doc);
        h.run_steps(5);
        let step = |h: &mut Harness<'static, Studio>,
                    what: &str,
                    n: usize,
                    f: &mut dyn FnMut(&mut Harness<'static, Studio>, usize)| {
            eprintln!("-- {name}: {what}");
            let ms: Vec<f64> = (0..n)
                .map(|k| {
                    f(h, k);
                    let t = std::time::Instant::now();
                    h.step();
                    t.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            report(&format!("  {what}"), &ms);
        };
        // A plan of the whole design, from nothing.
        let t = std::time::Instant::now();
        let fresh = h.state().doc.plan_cached(&mut Default::default());
        println!(
            "{name}: {} objects, {} stitches; a plan from nothing {:.1} ms, the last one {:.1} ms{}",
            h.state().doc.shapes.len(),
            fresh.stitch_count(),
            t.elapsed().as_secs_f64() * 1000.0,
            h.state().plan_ms,
            if h.state().planner.slow {
                " (planned in the background)"
            } else {
                ""
            },
        );
        let rect = h.state().canvas_rect;
        let c = rect.center();
        step(&mut h, "idle", 10, &mut |_, _| {});
        step(&mut h, "pointer moving over the design", 20, &mut |h, k| {
            h.hover_at(c + Vec2::new(k as f32 * 9.0 - 90.0, k as f32 * 5.0 - 50.0))
        });
        // The object with the most corners showing widgets, selected.
        let mut best = (0, 0);
        for i in 0..h.state().doc.shapes.len() {
            h.state_mut().set_selection(&[i]);
            if h.state_mut().editable_path().is_some() {
                let n = h.state().corner_widgets().len();
                if n > best.1 {
                    best = (i, n);
                }
            }
        }
        let (i, n) = best;
        h.state_mut().set_selection(&[i]);
        h.run_steps(2);
        step(
            &mut h,
            &format!("idle, one selected ({n} corner widgets)"),
            10,
            &mut |_, _| {},
        );
        let at = h.state().view.to_screen(
            rect,
            crate::app::shape_bounds(&h.state().doc.shapes[i]).center(),
        );
        step(
            &mut h,
            "pointer moving over the selection",
            20,
            &mut |h, k| h.hover_at(at + Vec2::new(k as f32 * 2.0 - 20.0, k as f32 - 10.0)),
        );
        // Rounding its corners.
        h.state_mut().editable_path();
        if let Some(&(corner, w)) = h.state().corner_widgets().first() {
            let from = h.state().view.to_screen(rect, w);
            let to = h
                .state()
                .view
                .to_screen(rect, w + (corner.u + corner.v).norm() * 30.0);
            h.hover_at(from);
            h.run_steps(1);
            h.drag_at(from);
            h.run_steps(1);
            step(&mut h, "dragging a corner widget", 20, &mut |h, k| {
                h.hover_at(from + (to - from) * ((k + 1) as f32 / 20.0))
            });
            step(&mut h, "letting it go", 1, &mut |h, _| h.drop_at(to));
            h.run_steps(2);
            h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
            h.run_steps(3);
        }
        // Moving it.
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        step(&mut h, "dragging the object", 20, &mut |h, k| {
            h.hover_at(at + Vec2::new(k as f32 * 3.0 + 3.0, k as f32))
        });
        step(&mut h, "letting it go", 1, &mut |h, _| {
            h.drop_at(at + Vec2::new(60.0, 20.0))
        });
        step(&mut h, "the frames after", 3, &mut |_, _| {});
        // A setting changed (a slider's every step re-plans).
        step(&mut h, "a stitch setting, step by step", 10, &mut |h, k| {
            if let Some(i) = h.state().sel_index() {
                h.state_mut().doc.shapes[i].params.spacing = Some(0.3 + k as f64 * 0.01);
                // (Changed between frames, where no frame sees it change:
                // say so, as the frame that changed it would have.)
                h.state_mut().dirty = true;
            }
        });
        // Everything selected, and moved.
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.run_steps(2);
        let n = h.state().sel_indices().len();
        step(
            &mut h,
            &format!("idle, all {n} selected"),
            10,
            &mut |_, _| {},
        );
        let at = h
            .state()
            .view
            .to_screen(rect, h.state().selection_bounds().center());
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        step(&mut h, "dragging everything", 20, &mut |h, k| {
            h.hover_at(at + Vec2::new(k as f32 * 3.0 + 3.0, 0.0))
        });
        step(&mut h, "letting it go", 1, &mut |h, _| {
            h.drop_at(at + Vec2::new(60.0, 0.0))
        });
        step(&mut h, "the frames after", 3, &mut |_, _| {});
        // The recovery copy written (it is after every edit settles).
        let t = std::time::Instant::now();
        let svg = h.state().doc.to_svg().unwrap();
        println!(
            "  writing it out as SVG                {:6.2} ms ({} KB)",
            t.elapsed().as_secs_f64() * 1000.0,
            svg.len() / 1024
        );
    }
}

/// Planning a design of your own from nothing, over and over (to look at
/// with a sampling profiler), and how long each object takes by itself.
///
/// `ZIGZAG_DESIGN=<file.svg> cargo test --release --features gui --bin zigzag-studio perf_bench::planning -- --ignored --nocapture`
#[test]
#[ignore]
fn planning_a_design_of_your_own() {
    let file = std::env::var("ZIGZAG_DESIGN").expect("ZIGZAG_DESIGN: an .svg file");
    let doc = zigzag::document::Document::open(std::path::Path::new(&file)).unwrap();
    let names = zigzag::svg::display_names(&doc.shapes);
    let opt = doc.plan_options();
    for (i, s) in doc.shapes.iter().enumerate() {
        let t = std::time::Instant::now();
        let d = zigzag::design::plan_cached(
            "one",
            std::slice::from_ref(s),
            &opt,
            &mut Default::default(),
        );
        println!(
            "{:>8.2} ms  {:>6} stitches  {} ({:?})",
            t.elapsed().as_secs_f64() * 1000.0,
            d.stitch_count(),
            names[i],
            s.params.kind
        );
    }
    let secs: f64 = std::env::var("ZIGZAG_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let start = std::time::Instant::now();
    let mut n = 0;
    let mut total = 0.0;
    loop {
        let t = std::time::Instant::now();
        let d = doc.plan_cached(&mut Default::default());
        total += t.elapsed().as_secs_f64() * 1000.0;
        n += 1;
        std::hint::black_box(d);
        if start.elapsed().as_secs_f64() >= secs {
            break;
        }
    }
    println!(
        "the whole design from nothing: {:.2} ms (mean of {n})",
        total / n as f64
    );
    let d = doc.plan_cached(&mut Default::default());
    for (k, b) in d.blocks.iter().enumerate() {
        println!(
            "block {k}: thread {:?}, {} runs, objects {:?}",
            b.color,
            b.runs.len(),
            d.block_shapes[k]
                .iter()
                .map(|i| names[*i].as_str())
                .collect::<Vec<_>>()
        );
    }
    let t = std::time::Instant::now();
    let issues = zigzag::checks::check(&doc.shapes, &d, &opt, None);
    println!(
        "checks: {:.2} ms, {} issues",
        t.elapsed().as_secs_f64() * 1000.0,
        issues.len()
    );
}

/// Frame times of making things and of lining them up: drawing a shape
/// with a Shape tool, drawing a path with the Pen, and dragging an object
/// with smart guides on and off; on the bench grid (36 objects, 213,000
/// stitches), a traced logo (hundreds of objects) and, with
/// `ZIGZAG_DESIGNS` naming a folder, every `.svg` in it.
///
/// Each line is the UI pass of a frame (mean and slowest); then, with
/// `TL_PROFILE=quiet`, that less the painter's drawing of the stitches
/// (the app draws those on the GPU); then egui's tessellation of
/// everything painted but the stitches; then the slowest frame's sections.
///
/// `TL_PROFILE=quiet cargo test --release --features gui --bin zigzag-studio perf_bench::making -- --ignored --nocapture`
#[test]
#[ignore]
fn making_and_aligning() {
    let example = |f: &str| {
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples")).join(f)
    };
    let mut files = vec![
        example("badge.svg"),
        example("bench-grid.svg"),
        example("agent-badge-preview.png"),
    ];
    if let Ok(dir) = std::env::var("ZIGZAG_DESIGNS") {
        let mut more: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "svg"))
            .collect();
        more.sort();
        files.extend(more);
    }
    for file in files {
        let name = file.file_stem().unwrap().to_string_lossy().into_owned();
        let mut h = Harness::builder()
            .with_size(Vec2::new(1600.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.state_mut().planner.timed = true;
        let doc = zigzag::document::Document::open(&file).unwrap();
        h.state_mut().set_document(doc);
        h.run_steps(5);
        settle(&mut h);
        println!(
            "{name}: {} objects, {} stitches, {} KB of SVG; a plan takes {:.1} ms{}",
            h.state().doc.shapes.len(),
            h.state().design.stitch_count(),
            h.state().doc.xml.len() / 1024,
            h.state().plan_ms,
            if h.state().planner.slow {
                " (in the background)"
            } else {
                ""
            },
        );
        let rect = h.state().canvas_rect;
        let c = rect.center();

        // A rectangle, drawn across the middle of the design.
        h.key_press(egui::Key::M);
        h.run_steps(2);
        let (from, to) = (c - Vec2::new(90.0, 60.0), c + Vec2::new(90.0, 60.0));
        times(&mut h, "shape tool: hovering", 20, &mut |h, k| {
            h.hover_at(from + Vec2::new(k as f32 * 3.0 - 60.0, k as f32 - 20.0))
        });
        times(&mut h, "shape tool: pressing", 1, &mut |h, _| {
            h.drag_at(from)
        });
        times(&mut h, "shape tool: drawing", 20, &mut |h, k| {
            h.hover_at(from + (to - from) * ((k + 1) as f32 / 20.0))
        });
        times(&mut h, "shape tool: letting go (added)", 1, &mut |h, _| {
            h.drop_at(to)
        });
        times(&mut h, "shape tool: the frames after", 4, &mut |_, _| {});
        settle(&mut h);
        // And a second one, beside it (the first is there to snap to).
        let (from, to) = (c + Vec2::new(120.0, -40.0), c + Vec2::new(200.0, 40.0));
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        for k in 1..=4 {
            h.hover_at(from + (to - from) * (k as f32 / 4.0));
            h.run_steps(1);
        }
        times(&mut h, "a second shape: letting go", 1, &mut |h, _| {
            h.drop_at(to)
        });
        times(
            &mut h,
            "a second shape: the frames after",
            4,
            &mut |_, _| {},
        );
        settle(&mut h);

        // A triangle with the Pen.
        h.key_press(egui::Key::P);
        h.run_steps(2);
        let pts = [
            c + Vec2::new(-200.0, 120.0),
            c + Vec2::new(-120.0, 120.0),
            c + Vec2::new(-160.0, 60.0),
        ];
        times(&mut h, "pen: hovering", 20, &mut |h, k| {
            h.hover_at(pts[0] + Vec2::new(k as f32 * 2.0 - 40.0, 0.0))
        });
        let mut clicks = Vec::new();
        for p in pts {
            h.hover_at(p);
            clicks.push(step_ms(&mut h));
            h.drag_at(p);
            clicks.push(step_ms(&mut h));
            h.drop_at(p);
            clicks.push(step_ms(&mut h));
        }
        report_both("pen: placing three points", &clicks);
        times(&mut h, "pen: Enter (the path is added)", 1, &mut |h, _| {
            h.key_press(egui::Key::Enter)
        });
        times(&mut h, "pen: the frames after", 4, &mut |_, _| {});
        settle(&mut h);

        // The Selection tool: the first rectangle picked and moved, with
        // smart guides and without.
        h.key_press(egui::Key::V);
        h.run_steps(2);
        let n = h.state().doc.shapes.len();
        let i = n - 3;
        for guides in [true, false] {
            h.state_mut().prefs.smart_guides = guides;
            let at = h.state().view.to_screen(
                rect,
                crate::app::shape_bounds(&h.state().doc.shapes[i]).center(),
            );
            let with = if guides { "guides on" } else { "guides off" };
            h.state_mut().set_selection(&[]);
            h.run_steps(2);
            h.hover_at(at);
            h.run_steps(1);
            times(
                &mut h,
                &format!("move, {with}: pressing"),
                1,
                &mut |h, _| h.drag_at(at),
            );
            times(
                &mut h,
                &format!("move, {with}: the first frame"),
                1,
                &mut |h, _| h.hover_at(at + Vec2::new(6.0, 2.0)),
            );
            times(
                &mut h,
                &format!("move, {with}: dragging"),
                30,
                &mut |h, k| h.hover_at(at + Vec2::new(k as f32 * 4.0 + 10.0, k as f32 * 1.5 + 3.0)),
            );
            times(
                &mut h,
                &format!("move, {with}: letting go"),
                1,
                &mut |h, _| h.drop_at(at + Vec2::new(126.0, 46.5)),
            );
            times(
                &mut h,
                &format!("move, {with}: the frames after"),
                4,
                &mut |_, _| {},
            );
            settle(&mut h);
            h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
            h.run_steps(3);
            settle(&mut h);
        }
        // An object of the design itself, the same way (guides on).
        h.state_mut().prefs.smart_guides = true;
        let at = h.state().view.to_screen(
            rect,
            crate::app::shape_bounds(&h.state().doc.shapes[0]).center(),
        );
        h.state_mut().set_selection(&[0]);
        h.run_steps(2);
        h.hover_at(at);
        h.run_steps(1);
        times(&mut h, "move one of its own: pressing", 1, &mut |h, _| {
            h.drag_at(at)
        });
        times(&mut h, "move one of its own: dragging", 30, &mut |h, k| {
            h.hover_at(at + Vec2::new(k as f32 * 4.0 + 10.0, k as f32 * 1.5 + 3.0))
        });
        times(&mut h, "move one of its own: letting go", 1, &mut |h, _| {
            h.drop_at(at + Vec2::new(126.0, 46.5))
        });
        times(
            &mut h,
            "move one of its own: the frames after",
            4,
            &mut |_, _| {},
        );
        settle(&mut h);
        // Idle with the new shape selected, and the pointer over it.
        h.state_mut().set_selection(&[i]);
        h.run_steps(2);
        times(&mut h, "idle, the new shape selected", 10, &mut |_, _| {});
        // Undo and redo of the last thing.
        times(&mut h, "undo", 1, &mut |h, _| {
            h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z)
        });
        times(&mut h, "undo: the frames after", 3, &mut |_, _| {});
    }
}

/// Wait for a plan the worker is making.
fn settle(h: &mut Harness<'static, Studio>) {
    for _ in 0..2000 {
        if !h.state().planner.pending && !h.state().dirty {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        h.run_steps(1);
    }
    h.run_steps(2);
}

/// One frame: its UI pass (ms), that less the painter's drawing of the
/// stitches (with `TL_PROFILE`), the tessellation of what it painted
/// apart from the stitches, and its sections.
fn step_ms(h: &mut Harness<'static, Studio>) -> (f64, f64, f64, String) {
    crate::profile::clear();
    let t = std::time::Instant::now();
    h.step();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let shapes: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter(|s| !matches!(s.shape, egui::Shape::Mesh(_)))
        .cloned()
        .collect();
    let t = std::time::Instant::now();
    let _ = h.ctx.tessellate(shapes, 2.0);
    let paint = t.elapsed().as_secs_f64() * 1000.0;
    let sections = [
        "input",
        "plan",
        "snapshot",
        "d:stitch",
        "d:transform",
        "d:lettering",
        "d:threads",
        "d:document",
        "d:objects",
        "d:agent",
        "chrome+docks",
        "c:pick",
        "c:tools",
        "c:stitches",
        "c:overlays",
        "c:sel_points",
        "c:selected",
        "canvas",
        "typing_box",
        "undo+session",
    ]
    .iter()
    .map(|s| (s, crate::profile::last(s)))
    .filter(|(_, ms)| *ms >= 0.3)
    .map(|(s, ms)| format!("{s} {ms:.1}"))
    .collect::<Vec<_>>()
    .join(", ");
    (ms, ms - crate::profile::last("c:stitches"), paint, sections)
}

fn report_both(what: &str, ms: &[(f64, f64, f64, String)]) {
    let n = ms.len() as f64;
    let mean = ms.iter().map(|m| m.0).sum::<f64>() / n;
    let worst = ms
        .iter()
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .cloned()
        .unwrap_or_default();
    let ui_mean = ms.iter().map(|m| m.1).sum::<f64>() / n;
    let ui_max = ms.iter().map(|m| m.1).fold(0.0, f64::max);
    let paint_mean = ms.iter().map(|m| m.2).sum::<f64>() / n;
    let paint_max = ms.iter().map(|m| m.2).fold(0.0, f64::max);
    println!(
        "  {what:<38} ui {mean:6.2} / {:6.2}   less stitches {ui_mean:6.2} / {ui_max:6.2}   tessellation {paint_mean:5.2} / {paint_max:5.2}   [{}]",
        worst.0, worst.3
    );
}

fn times(
    h: &mut Harness<'static, Studio>,
    what: &str,
    n: usize,
    f: &mut dyn FnMut(&mut Harness<'static, Studio>, usize),
) {
    let ms: Vec<_> = (0..n)
        .map(|k| {
            f(h, k);
            step_ms(h)
        })
        .collect();
    report_both(what, &ms);
}

/// Stepping through the font menu with the arrow keys: what each step
/// costs. First the menu by itself (every installed font listed, one
/// step a frame); then the text taking each font in turn, on an empty
/// canvas or (with `ZIGZAG_DESIGN`) on a design of your own: the switch
/// (the SVG rewritten and read back), the frame after it (the plan, when
/// it's made there), and how long until the stitches are the new font's.
///
/// `cargo test --release --features gui --bin zigzag-studio perf_bench::stepping -- --ignored --nocapture`
#[test]
#[ignore]
fn stepping_through_fonts() {
    let fonts = zigzag::document::font_families();
    println!("{} font families", fonts.len());
    // The menu alone.
    {
        let mut h = Harness::builder()
            .with_size(Vec2::new(300.0, 400.0))
            .build_ui_state(
                {
                    let fonts = fonts.clone();
                    move |ui, s: &mut (String, String)| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            crate::lettering::font_list(ui, &mut s.0, &fonts, &mut s.1);
                        });
                    }
                },
                (fonts[fonts.len() / 3].clone(), String::new()),
            );
        h.run_steps(3);
        let idle: Vec<f64> = (0..10)
            .map(|_| {
                let t = std::time::Instant::now();
                h.step();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        report("the menu, idle", &idle);
        let step: Vec<f64> = (0..20)
            .map(|_| {
                h.key_press(egui::Key::ArrowDown);
                let t = std::time::Instant::now();
                h.step();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        report("the menu, a step down", &step);
    }
    let mut h = Harness::builder()
        .with_size(Vec2::new(1600.0, 1000.0))
        .build_ui_state(
            |ui, s: &mut Studio| s.frame(ui),
            Studio::with_file(None, None),
        );
    h.state_mut().planner.timed = true;
    if let Ok(file) = std::env::var("ZIGZAG_DESIGN") {
        let doc = zigzag::document::Document::open(std::path::Path::new(&file)).unwrap();
        h.state_mut().set_document(doc);
    }
    h.run_steps(5);
    settle(&mut h);
    let text = std::env::var("ZIGZAG_TEXT").unwrap_or_else(|_| "Campfire Club".into());
    let spec = zigzag::document::TextSpec {
        text,
        ..Default::default()
    };
    let i = h.state_mut().add_lettering(&spec, (0.0, 0.0)).unwrap();
    h.run_steps(3);
    settle(&mut h);
    println!(
        "{} objects, {} stitches; the text is {} stitches",
        h.state().doc.shapes.len(),
        h.state().design.stitch_count(),
        h.state().design.per_shape[i],
    );
    let from = fonts.iter().position(|f| *f == spec.font).unwrap_or(0);
    let (mut switch, mut frame, mut landed, mut plans) = (vec![], vec![], vec![], vec![]);
    let mut slowest: (f64, String) = (0.0, String::new());
    let mut on_worker = 0;
    let mut rows: Vec<(f64, String)> = Vec::new();
    let n: usize = std::env::var("ZIGZAG_FONTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    for font in fonts.iter().cycle().skip(from + 1).take(n) {
        let i = h.state().sel_index().unwrap();
        let mut next = h.state().doc.lettering(i).unwrap();
        next.font = font.clone();
        let t = std::time::Instant::now();
        h.state_mut().update_lettering(i, &next).unwrap();
        let a = t.elapsed().as_secs_f64() * 1000.0;
        let (b, _, _, sections) = step_ms(&mut h);
        let worker = h.state().planner.pending;
        if worker {
            on_worker += 1;
        }
        for _ in 0..5000 {
            if !h.state().planner.pending && !h.state().dirty {
                break;
            }
            std::thread::sleep(std::time::Duration::from_micros(500));
            h.step();
        }
        let whole = t.elapsed().as_secs_f64() * 1000.0;
        switch.push(a);
        frame.push(b);
        landed.push(whole);
        plans.push(h.state().plan_ms);
        if whole > slowest.0 {
            slowest = (whole, font.clone());
        }
        rows.push((
            b,
            format!(
                "{font:<34} switch {a:6.1}  frame {b:8.1} [{sections}]  plan {:8.1}{}  stitches {}",
                h.state().plan_ms,
                if worker { " (worker)" } else { "" },
                h.state().design.stitch_count()
            ),
        ));
    }
    rows.sort_by(|x, y| y.0.total_cmp(&x.0));
    for (_, r) in rows.iter().take(12) {
        println!("  {r}");
    }
    println!("  ...");
    for (_, r) in rows.iter().skip(rows.len() / 2).take(3) {
        println!("  {r}");
    }
    times(&mut h, "idle, the text selected", 10, &mut |_, _| {});
    report("the switch (SVG rewritten, read back)", &switch);
    report("the frame after", &frame);
    report("the plan", &plans);
    report("until the stitches are the font's", &landed);
    println!(
        "{on_worker} of {n} planned in the background; the slowest was {} ({:.0} ms)",
        slowest.1, slowest.0
    );
}

/// Scrolling a long chat in the Agent panel, on a traced logo: what each
/// frame costs while the chat scrolls, with its earlier turns folded and
/// shown, beside a frame that only redraws.
///
/// `cargo test --release --features gui --bin zigzag-studio perf_bench::scrolling -- --ignored --nocapture`
#[test]
#[ignore]
fn scrolling_the_agent_chat() {
    use crate::agent::Entry;
    let mut h = studio_with_logo();
    settle(&mut h);
    h.state_mut().dock.reveal(crate::dock::PanelId::Agent);
    {
        let a = &mut h.state_mut().agent;
        a.config = None;
        for k in 0..40 {
            a.log.push(Entry::You(format!("Turn {k}: make the bear more detailed, with shading on the flank and a lighter muzzle")));
            for s in 0..6 {
                a.log.push(Entry::Tool(
                    [
                        "get_design",
                        "render_preview",
                        "trace_image",
                        "batch",
                        "set_object",
                        "check_design",
                    ][s]
                        .into(),
                    "object: Bear body, kind: fill, color: #c8b88e".into(),
                    Some(s != 3),
                ));
            }
            a.log.push(Entry::Said("I'm refining the bear's silhouette and paws, then tracing it back into embroidery shapes at the same size. The tan shading sits on the flank and the far legs, and the muzzle is a lighter cream so the face reads at 35 mm. ".repeat(2)));
            a.log.push(Entry::Done(format!("{} turns · 40 s", k + 1)));
        }
    }
    h.run_steps(6);
    // Where the chat is: just above the prompt.
    let over = {
        use egui_kittest::kittest::Queryable;
        let r = h.get_by_label("Prompt").rect();
        egui::pos2(r.center().x, r.top() - 150.0)
    };
    println!(
        "{} lines in the chat, the pointer at {over:?}",
        h.state().agent.log.len()
    );
    let wheel = |h: &mut Harness<'static, Studio>, dy: f32| {
        h.event(egui::Event::PointerMoved(over));
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, dy),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        });
    };
    times(&mut h, "redraw, nothing scrolling", 30, &mut |h, _| {
        h.event(egui::Event::PointerMoved(over));
    });
    times(
        &mut h,
        "scroll, last turns (rest folded)",
        30,
        &mut |h, k| wheel(h, if k < 15 { 40.0 } else { -40.0 }),
    );
    h.state_mut().agent.show_earlier = true;
    h.run_steps(3);
    times(&mut h, "scroll, whole chat (40 turns)", 30, &mut |h, k| {
        wheel(h, if k < 15 { 60.0 } else { -60.0 })
    });
}

/// Pressing Send in the Agent panel on a traced logo, with a stand-in for
/// Codex that only waits: how long that frame holds the window up, and
/// where the time goes.
///
/// `cargo test --release --features gui --bin zigzag-studio perf_bench::sending -- --ignored --nocapture`
#[cfg(unix)]
#[test]
#[ignore]
fn sending_a_prompt() {
    use egui_kittest::kittest::Queryable;
    let dir = std::env::temp_dir().join(format!("zz-send-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("codex");
    std::fs::write(
        &fake,
        "#!/bin/sh\ncase \"$1\" in debug|mcp) echo '[]'; exit 0;; -c) echo '[]'; exit 0;; esac\nsleep 30\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    unsafe { std::env::set_var("ZIGZAG_CODEX", &fake) };
    let mut h = studio_with_logo();
    settle(&mut h);
    // A server for the agent to reach, as the app has.
    h.state_mut().mcp =
        Some(crate::mcp::start(0, crate::tools::tool_specs(), String::new(), || {}).unwrap());
    h.state_mut().dock.reveal(crate::dock::PanelId::Agent);
    h.state_mut().agent.config = None;
    h.state_mut().agent.kind = crate::agent::AgentKind::Codex;
    h.state_mut().agent.found = [None, None, None];
    h.run_steps(5);
    for round in 0..4 {
        h.state_mut().agent.prompt = format!("make it greener {round}");
        h.run_steps(2);
        let t = std::time::Instant::now();
        h.get_by_label("Send").click();
        h.step();
        let click = t.elapsed().as_secs_f64() * 1000.0;
        let mut frames = 0;
        while !h.state().agent.running() && frames < 50 {
            h.step();
            frames += 1;
        }
        let shown = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "send {round}: click frame {click:.0} ms, running after {frames} more frames ({shown:.0} ms); stop shown: {}",
            h.query_by_label("Stop").is_some()
        );
        h.state_mut().agent.started =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(5));
        h.state_mut().agent.stop();
        h.run_steps(3);
    }
    unsafe { std::env::remove_var("ZIGZAG_CODEX") };
    let _ = std::fs::remove_dir_all(&dir);
}

/// Bringing forward a tab that came back from the last session with
/// unsaved changes: each stage, on every `recovery-*.svg` in the folder
/// `ZIGZAG_RECOVERY` names (copies: the session there is rewritten).
///
/// `ZIGZAG_RECOVERY=<folder> cargo test --profile trial --features gui --bin zigzag-studio perf_bench::reopening -- --ignored --nocapture`
#[test]
#[ignore]
fn reopening_recovered_tabs() {
    let dir = std::path::PathBuf::from(
        std::env::var("ZIGZAG_RECOVERY").expect("ZIGZAG_RECOVERY: a folder of recovery-*.svg"),
    );
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("recovery-") && n.ends_with(".svg"))
        })
        .collect();
    files.sort();
    if let Ok(only) = std::env::var("ZIGZAG_ONLY") {
        files.retain(|f| f.to_string_lossy().contains(&only));
    }
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1000.0;
    let badge = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg");
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        // (`ZIGZAG_COLD`: straight to the studio, as a launch is, nothing
        // planned yet in this process.)
        let cold = std::env::var_os("ZIGZAG_COLD").is_some();
        let t = std::time::Instant::now();
        let xml = std::fs::read_to_string(f).unwrap();
        let read = ms(t);
        let t = std::time::Instant::now();
        let doc = zigzag::document::Document::from_svg(&xml, "untitled", None).unwrap();
        let parse = ms(t);
        if cold {
            println!(
                "{name}: {} objects; read {read:.1} ms, parse {parse:.1} ms",
                doc.shapes.len()
            );
        }
        let t = std::time::Instant::now();
        let d = if cold {
            Default::default()
        } else {
            doc.plan_cached(&mut Default::default())
        };
        let plan = ms(t);
        let t = std::time::Instant::now();
        let _ = doc.plan_cached(&mut Default::default());
        let again = ms(t);
        let t = std::time::Instant::now();
        let issues = zigzag::checks::check(&doc.shapes, &d, &doc.plan_options(), None);
        let check = ms(t);
        if !cold {
            println!(
                "{name}: {} objects, {} stitches; read {read:.1} ms, parse {parse:.1} ms, plan {plan:.1} ms (again {again:.1} ms), checks {check:.1} ms ({} issues)",
                doc.shapes.len(),
                d.stitch_count(),
                issues.len()
            );
        }

        // In the studio: the session has the badge in front and this one
        // behind it, as the app restores it.
        let session = dir.join(format!("session-{name}"));
        let _ = std::fs::remove_dir_all(&session);
        std::fs::create_dir_all(&session).unwrap();
        std::fs::copy(f, session.join("recovery-1.svg")).unwrap();
        let meta = serde_json::json!({
            "active": 0,
            "tabs": [
                {"name": "badge", "path": badge, "recovery": false, "recovery_file": "recovery.svg"},
                {"name": "untitled", "path": null, "recovery": true, "recovery_file": "recovery-1.svg"},
            ],
        });
        // First as it was (nothing kept beside the recovery file), then
        // with the plan that first time kept.
        for kept in [false, true] {
            std::fs::write(session.join("session.json"), meta.to_string()).unwrap();
            let mut studio = Studio::with_file(None, None);
            studio.session = crate::session::SessionState::in_dir(Some(session.clone()));
            let t = std::time::Instant::now();
            assert!(studio.restore_session());
            let restore = ms(t);
            let mut h = Harness::builder()
                .with_size(Vec2::new(1600.0, 1000.0))
                .build_ui_state(|ui, s: &mut Studio| s.frame(ui), studio);
            h.state_mut().planner.timed = true;
            h.run_steps(5);
            settle(&mut h);
            // Click its tab: the frame that brings it forward, then frames
            // until its stitches show, and until they're planned again.
            let t = std::time::Instant::now();
            h.state_mut().tabs.pending = Some(crate::tabs::TabOp::Switch(1));
            h.step();
            let first = ms(t);
            let shown_first = h.state().design.stitch_count() > 0;
            let mut frames = 1;
            let mut slowest: f64 = first;
            while h.state().design.stitch_count() == 0
                || h.state().planner.pending
                || h.state().dirty
            {
                let f = std::time::Instant::now();
                h.step();
                slowest = slowest.max(ms(f));
                frames += 1;
                if frames > 20000 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let all = ms(t);
            println!(
                "    {}: restore (launch) {restore:.1} ms; switch: first frame {first:.1} ms (stitches {}), planned after {all:.1} ms ({frames} frames, slowest {slowest:.1} ms)",
                if kept { "plan kept" } else { "no plan kept" },
                if shown_first { "shown" } else { "not yet" }
            );
            h.state_mut().write_session();
            assert!(session.join("recovery-1.plan").exists());
        }
        let _ = std::fs::remove_dir_all(&session);
    }
}
