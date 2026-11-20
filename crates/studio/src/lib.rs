//! ZigZag studio: visual stitch editor (egui, OpenGL via glow) with an
//! MCP server so agents can work on the same canvas.
//!
//! One library for the two places it runs: the desktop app (the
//! `zigzag-studio` binary, see [`run`]) and the browser (ZZZ, the
//! `zigzag-web` crate, which starts a [`Studio`] in the page). What only
//! the browser needs is in [`platform`].
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

mod about;
mod agent;
mod agent_log;
mod app;
mod app_icon;
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
mod icons;
mod kits;
mod lasso;
mod layout;
mod lettering;
mod lights;
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
mod planner;
mod profile;
mod review;
mod rotate;
mod session;
mod settings;
mod sew;
mod shapes;
mod simplify;
mod snap;
mod swatches;
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

/// What the studio asks of the browser: files, storage, Bluetooth, agents.
#[cfg(target_arch = "wasm32")]
pub mod platform;

/// The desktop app's start: the command line, then the window (or, with
/// `--mcp-stdio`, the agent tools with no window).
#[cfg(not(target_arch = "wasm32"))]
mod desktop;
#[cfg(not(target_arch = "wasm32"))]
pub use desktop::run;

pub use app::Studio;
