//! ZigZag studio: visual stitch editor (egui, OpenGL via glow) with an
//! MCP server so agents can work on the same canvas.

mod agent;
mod app;
mod app_icon;
mod brush;
mod canvas;
mod capture;
mod chrome;
mod commands;
mod cursors;
mod density;
mod dialog;
mod dock;
mod flyout;
mod gpu;
mod icons;
mod kits;
mod layout;
mod lettering;
mod machine;
mod mcp;
mod mcp_chip;
mod native_menu;
mod palettes;
mod path_edit;
mod path_tools;
mod pen;
mod perf_bench;
mod planner;
mod profile;
mod session;
mod sew;
mod shapes;
mod snap;
mod swatches;
mod tabs;
mod taskbar;
mod theme;
mod threads;
mod tool_args;
mod tools;
mod trace_ui;
mod units;
mod view_tools;
mod widgets;

const USAGE: &str = "usage: zigzag-studio [design.svg] [--select N] [--mcp-port PORT | --no-mcp | --mcp-stdio]\n\n  --mcp-stdio  no window: serve the MCP tools over stdin/stdout (headless, for agents)";

fn main() -> eframe::Result {
    let mut path = None;
    let mut select = None;
    let mut mcp_port = std::env::var("ZIGZAG_MCP_PORT")
        .or_else(|_| std::env::var("THREADLINE_MCP_PORT"))
        .ok()
        .and_then(|v| v.parse().ok())
        .or(Some(mcp::DEFAULT_PORT));
    let mut headless = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--mcp-stdio") => headless = true,
            Some("--select") => {
                select = args.next().and_then(|v| v.to_str()?.parse::<usize>().ok())
            }
            Some("--mcp-port") => mcp_port = args.next().and_then(|v| v.to_str()?.parse().ok()),
            Some("--no-mcp") => mcp_port = None,
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return Ok(());
            }
            // Finder can pass a process serial number when launching the .app.
            Some(s) if s.starts_with("-psn_") => {}
            _ => path = Some(std::path::PathBuf::from(a)),
        }
    }
    if headless {
        // No window: the same tools, on a document only the agent sees.
        let mut studio = app::Studio::with_file(path, select);
        let instructions = format!(
            "{} Headless: no one is watching this document and nothing is kept unless you save or export it.",
            tools::INSTRUCTIONS
        );
        mcp::serve_stdio(&tools::tool_specs(), &instructions, |name, args| {
            studio.run_tool(name, args)
        });
        return Ok(());
    }
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: {
            let v = eframe::egui::ViewportBuilder::default()
                .with_title("ZigZag")
                .with_app_id("zigzag-studio")
                .with_inner_size([1360.0, 860.0])
                .with_min_inner_size([1000.0, 620.0])
                .with_drag_and_drop(true);
            // macOS: content runs under a transparent title bar, so the toolbar
            // and traffic lights share one strip (a unified toolbar).
            if cfg!(target_os = "macos") {
                v.with_fullsize_content_view(true)
                    .with_titlebar_shown(false)
                    .with_title_shown(false)
            } else {
                v
            }
        },
        ..Default::default()
    };
    eframe::run_native(
        "ZigZag",
        options,
        Box::new(move |cc| Ok(Box::new(app::Studio::new(cc, path, select, mcp_port)))),
    )
}
