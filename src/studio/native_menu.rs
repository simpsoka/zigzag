//! The real macOS menu bar, built from the shared command list
//! (`commands::menus`). ⌘-shortcuts are menu key equivalents, so they show in
//! the menus and work like any Mac app. While a text field has focus the Edit
//! commands are disabled, which lets ⌘C/⌘V/⌘Z reach the text field instead.
//! Elsewhere this compiles to a stub and the menus are drawn in the window.

use crate::commands::Action;

#[cfg(any(target_os = "macos", feature = "menu-check"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod imp {
    use super::Action;
    use crate::app::Studio;
    use crate::commands::{Entry, Sc, menus};
    use eframe::egui::{self, Key};
    use muda::accelerator::{Accelerator, CMD_OR_CTRL, Code, Modifiers};
    use muda::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
    use std::collections::HashMap;
    use std::sync::{Mutex, mpsc};

    enum Handle {
        Plain(MenuItem),
        Check(CheckMenuItem),
    }

    pub struct NativeMenu {
        _menu: Menu,
        items: Vec<(Action, Handle)>,
        by_id: HashMap<MenuId, Action>,
        rx: mpsc::Receiver<MenuId>,
        state: HashMap<Action, (bool, Option<bool>)>,
        /// File › Open Recent, and the files it lists now.
        recent: Option<Submenu>,
        recent_shown: Vec<std::path::PathBuf>,
    }

    fn code(k: Key) -> Option<Code> {
        Some(match k {
            Key::A => Code::KeyA,
            Key::B => Code::KeyB,
            Key::C => Code::KeyC,
            Key::D => Code::KeyD,
            Key::E => Code::KeyE,
            Key::F => Code::KeyF,
            Key::G => Code::KeyG,
            Key::H => Code::KeyH,
            Key::I => Code::KeyI,
            Key::J => Code::KeyJ,
            Key::K => Code::KeyK,
            Key::L => Code::KeyL,
            Key::M => Code::KeyM,
            Key::N => Code::KeyN,
            Key::O => Code::KeyO,
            Key::P => Code::KeyP,
            Key::Q => Code::KeyQ,
            Key::R => Code::KeyR,
            Key::S => Code::KeyS,
            Key::T => Code::KeyT,
            Key::U => Code::KeyU,
            Key::V => Code::KeyV,
            Key::W => Code::KeyW,
            Key::X => Code::KeyX,
            Key::Y => Code::KeyY,
            Key::Z => Code::KeyZ,
            Key::Equals => Code::Equal,
            Key::Minus => Code::Minus,
            Key::Num0 => Code::Digit0,
            Key::Num1 => Code::Digit1,
            Key::Num2 => Code::Digit2,
            Key::Num3 => Code::Digit3,
            Key::Num4 => Code::Digit4,
            Key::Num5 => Code::Digit5,
            Key::Num6 => Code::Digit6,
            Key::Num7 => Code::Digit7,
            Key::Num8 => Code::Digit8,
            Key::Num9 => Code::Digit9,
            Key::OpenBracket => Code::BracketLeft,
            Key::CloseBracket => Code::BracketRight,
            Key::Quote => Code::Quote,
            Key::Comma => Code::Comma,
            _ => return None,
        })
    }

    /// Only ⌘ shortcuts become key equivalents; single-letter tool keys stay
    /// in the canvas so they never steal typing.
    fn accel(sc: Option<Sc>) -> Option<Accelerator> {
        let sc = sc.filter(|s| s.cmd)?;
        let mut m = CMD_OR_CTRL;
        if sc.shift {
            m |= Modifiers::SHIFT;
        }
        Some(Accelerator::new(m, code(sc.key)?))
    }

    fn id(a: Action) -> MenuId {
        MenuId::new(format!("{a:?}"))
    }

    impl NativeMenu {
        fn fill(&mut self, sub: &Submenu, entries: &[Entry], studio: &Studio) -> muda::Result<()> {
            for e in entries {
                match e {
                    Entry::Sep => sub.append(&PredefinedMenuItem::separator())?,
                    Entry::Sub(title, v) => {
                        let s = Submenu::new(*title, true);
                        self.fill(&s, v, studio)?;
                        sub.append(&s)?;
                    }
                    Entry::Recent => {
                        let s = Submenu::new(crate::commands::RECENT_TITLE, true);
                        sub.append(&s)?;
                        self.recent = Some(s);
                        self.fill_recent(studio);
                    }
                    Entry::Item(a, label, sc) => {
                        let enabled = studio.action_enabled(*a);
                        let checked = studio.action_checked(*a);
                        self.by_id.insert(id(*a), *a);
                        let h = match checked {
                            Some(c) => {
                                let item =
                                    CheckMenuItem::with_id(id(*a), *label, enabled, c, accel(*sc));
                                sub.append(&item)?;
                                Handle::Check(item)
                            }
                            None => {
                                let item = MenuItem::with_id(id(*a), *label, enabled, accel(*sc));
                                sub.append(&item)?;
                                Handle::Plain(item)
                            }
                        };
                        self.state.insert(*a, (enabled, checked));
                        self.items.push((*a, h));
                    }
                }
            }
            Ok(())
        }

        /// Open Recent's items, made again from the list as it is now.
        fn fill_recent(&mut self, studio: &Studio) {
            let Some(sub) = &self.recent else { return };
            while sub.remove_at(0).is_some() {}
            let files = studio.recent.files();
            for (i, label) in studio.recent.labels().into_iter().enumerate() {
                let a = Action::OpenRecent(i);
                self.by_id.insert(id(a), a);
                // (An & in a file's name is the name's, not a mnemonic.)
                let label = label.replace('&', "&&");
                let _ = sub.append(&MenuItem::with_id(id(a), label, true, None));
            }
            if !files.is_empty() {
                let _ = sub.append(&PredefinedMenuItem::separator());
            }
            let a = Action::ClearRecent;
            self.by_id.insert(id(a), a);
            let _ = sub.append(&MenuItem::with_id(
                id(a),
                crate::commands::CLEAR_RECENT,
                !files.is_empty(),
                None,
            ));
            self.recent_shown = files.to_vec();
        }

        pub fn new(ctx: egui::Context, studio: &Studio) -> muda::Result<NativeMenu> {
            let (tx, rx) = mpsc::channel();
            let tx = Mutex::new(tx);
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send(e.id().clone());
                }
                ctx.request_repaint();
            }));
            let mut me = NativeMenu {
                _menu: Menu::new(),
                items: Vec::new(),
                by_id: HashMap::new(),
                rx,
                state: HashMap::new(),
                recent: None,
                recent_shown: Vec::new(),
            };
            let app = Submenu::new("ZigZag", true);
            // About ZigZag: ZigZag's own window (the system's About panel
            // can't carry the links).
            me.fill(&app, &[crate::commands::ABOUT_ITEM], studio)?;
            // Check for Updates…, under About as Mac apps have it.
            me.fill(&app, &[crate::commands::UPDATES_ITEM], studio)?;
            app.append_items(&[&PredefinedMenuItem::separator()])?;
            // Settings…, where a Mac app has it.
            me.fill(&app, &[crate::commands::SETTINGS_ITEM], studio)?;
            app.append_items(&[
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &PredefinedMenuItem::separator(),
            ])?;
            // Quit ZigZag: ZigZag's own, so that documents with unsaved
            // changes are asked about first (the system's ends the app
            // there and then).
            me.fill(&app, &[crate::commands::QUIT_ITEM], studio)?;
            me._menu.append(&app)?;
            for (title, entries) in menus() {
                let sub = Submenu::new(title, true);
                if title == "Window" {
                    sub.append_items(&[
                        &PredefinedMenuItem::minimize(None),
                        &PredefinedMenuItem::maximize(None),
                        &PredefinedMenuItem::fullscreen(None),
                        &PredefinedMenuItem::separator(),
                    ])?;
                }
                me.fill(&sub, &entries, studio)?;
                // (File › Close, ⌘W, closes the tab in front, or the window
                // once the last tab is empty.)
                if title == "Window" {
                    sub.append_items(&[
                        &PredefinedMenuItem::separator(),
                        &PredefinedMenuItem::bring_all_to_front(None),
                    ])?;
                    #[cfg(target_os = "macos")]
                    sub.set_as_windows_menu_for_nsapp();
                }
                // (The Mac puts its search of the menus at the top of Help.)
                #[cfg(target_os = "macos")]
                if title == "Help" {
                    sub.set_as_help_menu_for_nsapp();
                }
                me._menu.append(&sub)?;
            }
            #[cfg(target_os = "macos")]
            me._menu.init_for_nsapp();
            Ok(me)
        }

        /// Keep enabled / checked states current (only touching what changed).
        pub fn sync(&mut self, studio: &Studio, typing: bool) {
            if self.recent_shown != studio.recent.files() {
                self.fill_recent(studio);
            }
            for (a, h) in &self.items {
                let edit = matches!(
                    a,
                    Action::Undo
                        | Action::Redo
                        | Action::Copy
                        | Action::Paste
                        | Action::Duplicate
                        | Action::Delete
                        | Action::SelectAll
                        | Action::Deselect
                );
                let enabled = studio.action_enabled(*a) && !(typing && edit);
                let checked = studio.action_checked(*a);
                if self.state.get(a) == Some(&(enabled, checked)) {
                    continue;
                }
                match h {
                    Handle::Plain(i) => i.set_enabled(enabled),
                    Handle::Check(i) => {
                        i.set_enabled(enabled);
                        i.set_checked(checked.unwrap_or(false));
                    }
                }
                self.state.insert(*a, (enabled, checked));
            }
        }

        pub fn poll(&self) -> Vec<Action> {
            self.rx
                .try_iter()
                .filter_map(|id| self.by_id.get(&id).copied())
                .collect()
        }
    }

    #[cfg(test)]
    mod tests {
        /// With the native menu, a ⌘ shortcut is the menu's to take: one
        /// whose key it can't name would do nothing at all.
        #[test]
        fn every_command_shortcut_has_a_key_the_menu_knows() {
            for (a, label, sc) in crate::commands::all_actions() {
                if let Some(sc) = sc.filter(|s| s.cmd) {
                    assert!(super::accel(Some(sc)).is_some(), "{label} ({a:?}): {:?}", sc.key);
                }
            }
        }
    }
}

#[cfg(any(target_os = "macos", feature = "menu-check"))]
pub use imp::NativeMenu;

/// No native menu bar on this platform.
#[cfg(not(any(target_os = "macos", feature = "menu-check")))]
pub struct NativeMenu;

#[cfg(not(any(target_os = "macos", feature = "menu-check")))]
impl NativeMenu {
    pub fn sync(&mut self, _studio: &crate::app::Studio, _typing: bool) {}
    pub fn poll(&self) -> Vec<Action> {
        Vec::new()
    }
}
