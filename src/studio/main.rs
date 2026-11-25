//! ZigZag studio: visual stitch editor (egui, OpenGL via glow) with an
//! MCP server so agents can work on the same canvas.

// A window app on Windows, so opening ZigZag.exe doesn't also open a console
// window. `attach_console` gives a launch from a terminal its output back.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod about;
mod account;
mod agent;
mod agent_setup;
mod agent_log;
mod app;
mod app_icon;
mod attach;
mod brands;
mod backing;
mod brush;
mod build_tools;
mod canvas;
mod capture;
mod chrome;
mod commands;
mod crash;
mod cursors;
mod curvature;
mod density;
mod dialog;
mod dock;
mod eyedropper;
mod file_dialog;
mod flyout;
mod gpu;
mod history;
mod icons;
mod instance;
mod kits;
mod lasso;
mod layout;
mod lettering;
mod lights;
mod link;
mod machine;
mod machines;
mod mcp;
mod mcp_chip;
mod native_menu;
mod palettes;
mod path_edit;
mod path_tools;
mod pen;
mod perf_bench;
mod plan_keep;
mod planner;
mod profile;
mod review;
mod rotate;
mod session;
mod settings;
mod sew;
mod shapes;
mod thumbs;
mod share;
mod simplify;
mod slash;
mod snap;
mod swatches;
mod sync;
mod tabs;
mod taskbar;
mod theme;
mod threads;
mod tools;
mod trace_ui;
mod ui_scale;
mod units;
mod update;
mod view_tools;
mod widgets;

/// Runs `cmd` without a console window of its own. ZigZag is a window app
/// on Windows (above), so a console program it starts (curl, claude, codex)
/// would otherwise open a console window for as long as it runs. Elsewhere
/// it does nothing.
pub(crate) fn no_console(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Windows starts a window app without a console, so `--help`, crash
/// messages and logs printed from a terminal would go nowhere. Use the
/// console of the terminal that started ZigZag, if any (as Alacritty and
/// WezTerm do). Output that is already piped somewhere (an agent's
/// `--mcp-stdio`, a script reading `--help`) stays where it goes.
#[cfg(windows)]
fn attach_console() {
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(pid: u32) -> i32;
    }
    // SAFETY: takes a constant and touches no memory of ours; fails
    // harmlessly when there is no parent console.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_console() {}

const USAGE: &str = "usage: zigzag-studio [design.svg | zigzag://open?url=… ...] [--select N] [--new-window] [--mcp-port PORT | --no-mcp | --mcp-stdio]\n\n  --mcp-stdio   no window: serve the MCP tools over stdin/stdout (headless, for agents)
  --new-window  a window of its own, even with ZigZag running already (also ZIGZAG_NEW_WINDOW=1)

With ZigZag running already, the files open as tabs in its window instead.";

fn main() -> eframe::Result {
    attach_console();
    // A failure that stops ZigZag leaves a log to report (see `crash`).
    crash::install();
    // (To see the crash log and the report offer work: this fails on
    // purpose, as a real failure would.)
    if std::env::var_os("ZIGZAG_CRASH_TEST").is_some() {
        panic!("a crash made on purpose, to test the crash log");
    }
    let mut paths = Vec::new();
    // `zigzag://` links (Windows and Linux; macOS sends an Apple Event).
    let mut links = Vec::new();
    let mut select = None;
    // The agent server's port: as the command line says, else the
    // environment, else Settings (which can also turn it off).
    let mut mcp_port = std::env::var("ZIGZAG_MCP_PORT")
        .or_else(|_| std::env::var("THREADLINE_MCP_PORT"))
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Some)
        .unwrap_or_else(|| settings::Prefs::load().agent_port());
    let mut headless = false;
    let mut new_window = std::env::var_os("ZIGZAG_NEW_WINDOW").is_some();
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--mcp-stdio") => headless = true,
            Some("--new-window") => new_window = true,
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
            Some(s) if link::is_link(s) => links.push(s.to_string()),
            _ => paths.push(std::path::PathBuf::from(a)),
        }
    }
    // Only one window: with ZigZag running already, it opens these files
    // and this launch is done (see `instance`). Not without a window, nor
    // for a capture run (a recording, or a script taking pictures), which
    // wants a window of its own, nor when asked for a window of its own
    // (a dev build next to the installed app, say).
    let capture = [
        "ZIGZAG_CAPTURE",
        "ZIGZAG_CAPTURE_STILL",
        "ZIGZAG_CAPTURE_SHOW",
    ]
    .iter()
    .any(|v| std::env::var_os(v).is_some());
    let mut inbox = None;
    if !headless && !capture && !new_window {
        let request = instance::Request {
            // (The one running may have been started somewhere else.)
            files: paths
                .iter()
                .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()))
                .collect(),
            select,
            links: links.clone(),
        };
        match instance::claim_in_config(&request) {
            instance::Start::First(i) => inbox = Some(i),
            instance::Start::HandedOver => return Ok(()),
            instance::Start::Alone => {}
        }
    }
    // Scan the system fonts in the background: a design without text opens
    // without waiting on them, and they're ready by the first lettering.
    std::thread::spawn(|| {
        zigzag::svg::fontdb();
    });
    if headless {
        // No window: the same tools, on a document only the agent sees.
        let mut studio = app::Studio::with_file(paths.into_iter().next(), select);
        studio.load_machines();
        studio.prefs = settings::Prefs::load();
        studio.apply_agent_log();
        let instructions = format!(
            "{} Headless: no one is watching this document and nothing is kept unless you save or export it.",
            tools::INSTRUCTIONS
        );
        mcp::serve_stdio(&tools::tool_specs(), &instructions, |name, args| {
            let at = std::time::Instant::now();
            let result = studio.run_tool_caught(name, args);
            studio.log_call(name, args, at, &result);
            result
        });
        return Ok(());
    }
    // Links this launch was started with download now, and open when the
    // window is up; on macOS, links arrive as Apple Events from here on.
    link::register_scheme();
    for l in &links {
        link::open(l);
    }
    #[cfg(target_os = "macos")]
    link::listen_for_apple_events();
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: {
            // Room for the layout at the interface size in Settings.
            let (first, min) = ui_scale::first_window(settings::Prefs::load().ui_size);
            let v = eframe::egui::ViewportBuilder::default()
                .with_title("ZigZag")
                .with_app_id("zigzag-studio")
                .with_inner_size(first)
                .with_min_inner_size(min)
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
        Box::new(move |cc| {
            Ok(Box::new(app::Studio::new(
                cc, paths, select, mcp_port, inbox,
            )))
        }),
    )
}
