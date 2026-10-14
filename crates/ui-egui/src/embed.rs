//! Embedding the sheet in another egui application.
//!
//! [`SheetApp::ui`] draws the whole spreadsheet into any egui region. A host that shows it as one
//! panel among its own sets [`EmbedOptions`] on the app (or starts from [`SheetApp::embedded`]):
//! the host then owns the window chrome (title bar, window title and size, the File view), the
//! global egui style (light/dark, interface zoom) and the keyboard outside the sheet, while the
//! ribbon, formula bar, grid and sheet tabs stay. On top of that a host can
//!
//! - supply its design system's colours ([`SheetApp::set_host_theme`], [`crate::theme::Palette`]);
//! - add its own commands ([`SheetApp::register_command`]), reachable like built-in ones from the
//!   ribbon, keytips, the command search, the control channel and [`SheetApp::run`];
//! - add ribbon tabs after the built-in ones ([`SheetApp::add_ribbon_tab`], [`crate::host_ribbon`]);
//! - own the workbook's storage: [`SheetApp::open_workbook_bytes`], [`SheetApp::workbook_bytes`],
//!   [`SheetApp::mark_saved`] and the [`crate::Services::save_workbook`] hook.
//!
//! The defaults ([`EmbedOptions::default`]) are the standalone app, unchanged.

use egui::{Event, Key, KeyboardShortcut, Modifiers};
use serde_json::{Value as Json, json};

use crate::SheetApp;
use crate::host_ribbon::RibbonTab;
use crate::theme::{self, HostTheme};

/// Which parts of the window the sheet draws and which context-wide settings it owns.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbedOptions {
    /// The title bar (document title, Search, Share, Comments, the light/dark switch). The host
    /// draws its own window chrome when this is off.
    pub title_bar: bool,
    /// The Quick Access Toolbar in the title bar (AutoSave, Home, Save, Undo, Redo, More). It
    /// lives in the title bar, so it shows only with it.
    pub quick_access_toolbar: bool,
    /// The File tab and its Backstage view (New, Open, Save As, Print, Options…). Off, the File
    /// tab is gone and `ui.backstage` reports that the host has no File view.
    pub backstage: bool,
    /// The status bar under the sheet tabs (mode, selection statistics, view buttons, zoom).
    /// The user's View setting still applies when this is on.
    pub status_bar: bool,
    /// Whether the sheet may drive the native window: set its title, start a drag or maximize
    /// from the title bar, and resize, focus or close it from the control channel.
    pub window_controls: bool,
    /// Whether the sheet owns the egui context's global style: light/dark (`view.theme`), the
    /// interface size (`view.interfaceScale`), the visuals and the spacing. Off, the host's
    /// light/dark applies, GridCraft's look is scoped to the sheet's own region, and GridCraft's
    /// fonts are merged into the host's font set instead of replacing it.
    pub global_style: bool,
    /// Who gets the keyboard.
    pub keyboard: KeyboardScope,
    /// Shortcuts the host keeps for itself: the sheet never sees them, even while it has the
    /// keyboard (e.g. the host's own Cmd/Ctrl+S). Matched exactly (Shift and Alt must match;
    /// `COMMAND` is Cmd on macOS and Ctrl elsewhere). Cmd/Ctrl+C, X and V also keep the copy,
    /// cut and paste events. For decisions per event, see [`crate::Services::keep_key`].
    pub reserved_shortcuts: Vec<KeyboardShortcut>,
}

/// Who gets keyboard input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardScope {
    /// The sheet is the window: it handles every key (the standalone app).
    Window,
    /// The sheet is one panel among others: it handles keys only while it has the keyboard, which
    /// it takes when the user clicks into it (or the host calls [`SheetApp::focus_keyboard`]) and
    /// gives up when the user clicks elsewhere (or the host calls [`SheetApp::release_keyboard`]).
    /// Without the keyboard it never grabs egui's focus and its shortcuts and keytips stay off.
    Focus,
}

impl Default for EmbedOptions {
    /// The standalone app: everything on, the sheet owns the window and the keyboard.
    fn default() -> Self {
        EmbedOptions {
            title_bar: true,
            quick_access_toolbar: true,
            backstage: true,
            status_bar: true,
            window_controls: true,
            global_style: true,
            keyboard: KeyboardScope::Window,
            reserved_shortcuts: Vec::new(),
        }
    }
}

impl EmbedOptions {
    /// The sheet as a panel in a host's window: no title bar, Quick Access Toolbar, File view,
    /// status bar or window control; the host's global style; the keyboard only while focused.
    pub fn embedded() -> Self {
        EmbedOptions {
            title_bar: false,
            quick_access_toolbar: false,
            backstage: false,
            status_bar: false,
            window_controls: false,
            global_style: false,
            keyboard: KeyboardScope::Focus,
            reserved_shortcuts: Vec::new(),
        }
    }
}

/// The run function of a [`HostCommand`].
pub type HostRun = Box<dyn FnMut(&mut SheetApp, &Json) -> Result<Json, String>>;
/// The enablement test of a [`HostCommand`].
pub type HostEnabled = Box<dyn Fn(&SheetApp) -> bool>;

/// A command the embedding application adds. It runs through [`SheetApp::run`] like a built-in
/// command, so ribbon buttons, keytips, the command search and the control channel reach it.
pub struct HostCommand {
    /// Namespaced id, e.g. `contoso.refresh`: letters, digits, `.`, `_` and `-`, with at least one
    /// dot. It can't reuse a built-in id or the `ui.`/`app.` namespaces.
    pub id: String,
    pub label: String,
    /// Documentation of the params, like the built-in commands' (`{range?: A1}`).
    pub params: String,
    run: HostRun,
    enabled: Option<HostEnabled>,
}

impl HostCommand {
    pub fn new(id: impl Into<String>, label: impl Into<String>, run: impl FnMut(&mut SheetApp, &Json) -> Result<Json, String> + 'static) -> Self {
        HostCommand { id: id.into(), label: label.into(), params: String::new(), run: Box::new(run), enabled: None }
    }

    /// Documents the params (shown by the control channel's `engine.commands`).
    pub fn params(mut self, doc: impl Into<String>) -> Self {
        self.params = doc.into();
        self
    }

    /// Only available while `enabled` says so (its ribbon buttons are greyed out otherwise).
    pub fn enabled_when(mut self, enabled: impl Fn(&SheetApp) -> bool + 'static) -> Self {
        self.enabled = Some(Box::new(enabled));
        self
    }
}

/// What [`crate::Services::save_workbook`] receives: the workbook encoded as .xlsx.
pub struct HostSave<'a> {
    /// The workbook's display name with an `.xlsx` extension.
    pub name: &'a str,
    pub bytes: &'a [u8],
    /// Save As (the host may ask for a new name) rather than Save.
    pub save_as: bool,
}

/// What the embedding application added to the app.
#[derive(Default)]
pub(crate) struct Host {
    pub(crate) commands: Vec<HostCommand>,
    pub(crate) tabs: Vec<RibbonTab>,
    pub(crate) theme: Option<HostTheme>,
    /// The host theme the context's global visuals were last built from (owned global style).
    pub(crate) theme_applied: Option<HostTheme>,
    /// The sheet has the keyboard (with [`KeyboardScope::Focus`]).
    pub(crate) keyboard: bool,
    /// egui's focused widget when the sheet last finished drawing with the keyboard: focus that
    /// moves anywhere else between frames went to a host widget.
    pub(crate) focus_seen: Option<egui::Id>,
    /// The host handed the keyboard over ([`SheetApp::focus_keyboard`]): its own widget lets go
    /// of egui's focus at the next frame.
    pub(crate) focus_requested: bool,
}

/// The longest command id a host may register.
const MAX_COMMAND_ID: usize = 128;

/// UI-level command ids handled by [`SheetApp::run`] before the engine (kept in step with
/// `run_ui_command`): a host command can't take them over.
pub(crate) const UI_COMMANDS: &[&str] = &[
    "ui.printSettings",
    "ui.printSubmit",
    "ui.ribbonTab",
    "view.collapseRibbon",
    "view.darkMode",
    "view.theme",
    "view.animations",
    "options.autoComplete",
    "view.interfaceScale",
    "app.language.set",
    "app.language.english",
    "app.language.japanese",
    "ui.backstage",
    "ui.dialog",
    "ui.message.dismiss",
    "ui.notice",
    "ui.notice.dismiss",
];

fn check_command_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > MAX_COMMAND_ID {
        return Err(format!("a command id has 1 to {MAX_COMMAND_ID} characters"));
    }
    if !id.contains('.') || id.starts_with('.') || id.ends_with('.') {
        return Err(format!("command id {id:?} needs a namespace, like \"contoso.refresh\""));
    }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err(format!("command id {id:?} may use letters, digits, '.', '_' and '-' only"));
    }
    if id.starts_with("ui.") || id.starts_with("app.") || UI_COMMANDS.contains(&id) || gridcraft_engine::find_command(id).is_some() {
        return Err(format!("command id {id:?} belongs to GridCraft"));
    }
    Ok(())
}

/// Keyboard events: what [`KeyboardScope::Focus`] and reserved shortcuts hold back.
fn is_keyboard(e: &Event) -> bool {
    matches!(e, Event::Key { .. } | Event::Text(_) | Event::Copy | Event::Cut | Event::Paste(_) | Event::Ime(_))
}

/// Whether `e` is one of the host's reserved shortcuts.
fn is_reserved(reserved: &[KeyboardShortcut], e: &Event) -> bool {
    let command_letter = |key: Key| reserved.iter().any(|s| s.logical_key == key && Modifiers::COMMAND.matches_exact(s.modifiers));
    match e {
        Event::Key { key, modifiers, .. } => reserved.iter().any(|s| s.logical_key == *key && modifiers.matches_exact(s.modifiers)),
        Event::Copy => command_letter(Key::C),
        Event::Cut => command_letter(Key::X),
        Event::Paste(_) => command_letter(Key::V),
        _ => false,
    }
}

impl SheetApp {
    /// A sheet for a host's window: [`EmbedOptions::embedded`].
    pub fn embedded(session: gridcraft_engine::Session, services: crate::Services) -> SheetApp {
        let mut app = SheetApp::new(session, services);
        app.embed = EmbedOptions::embedded();
        app
    }

    /// Use the host's colours (or GridCraft's own again with `None`) from the next frame on.
    /// Light or dark follows the egui context: the host's when the sheet doesn't own the global
    /// style, the sheet's own `view.theme` otherwise.
    pub fn set_host_theme(&mut self, theme: Option<HostTheme>) {
        self.host.theme = theme;
    }

    pub fn host_theme(&self) -> Option<HostTheme> {
        self.host.theme
    }

    /// Keeps the context's host theme in step with this app's, re-building the global visuals
    /// when the sheet owns them.
    pub(crate) fn sync_host_theme(&mut self, ctx: &egui::Context) {
        if theme::host_theme(ctx) != self.host.theme {
            theme::set_host_theme(ctx, self.host.theme);
        }
        if self.embed.global_style && self.host.theme != self.host.theme_applied {
            theme::apply_visuals(ctx);
            self.host.theme_applied = self.host.theme;
        }
    }

    /// Adds a command. Its id must be new (see [`HostCommand::id`]).
    pub fn register_command(&mut self, command: HostCommand) -> Result<(), String> {
        check_command_id(&command.id)?;
        if self.has_host_command(&command.id) {
            return Err(format!("command {:?} is already registered", command.id));
        }
        self.host.commands.push(command);
        Ok(())
    }

    /// Removes a host command; `false` when there was none with this id.
    pub fn unregister_command(&mut self, id: &str) -> bool {
        let before = self.host.commands.len();
        self.host.commands.retain(|c| c.id != id);
        self.host.commands.len() != before
    }

    pub fn has_host_command(&self, id: &str) -> bool {
        self.host.commands.iter().any(|c| c.id == id)
    }

    /// The host commands as the control channel lists commands (`engine.commands`).
    pub(crate) fn host_command_list(&self) -> Vec<Json> {
        self.host
            .commands
            .iter()
            .map(|c| {
                let enabled = c.enabled.as_ref().is_none_or(|f| f(self));
                json!({"id": c.id, "label": c.label, "params": c.params, "enabled": enabled, "host": true, "menu": self.host_menu_path(&c.id)})
            })
            .collect()
    }

    /// Where a host command sits on the ribbon: its tab and group, when a button runs it.
    pub(crate) fn host_menu_path(&self, id: &str) -> Vec<String> {
        for tab in &self.host.tabs {
            for group in &tab.groups {
                if group.buttons.iter().any(|b| b.command == id) {
                    return [tab.label.clone(), group.label.clone()].into_iter().filter(|s| !s.is_empty()).collect();
                }
            }
        }
        Vec::new()
    }

    /// Whether a command can run now: a host command's own test, a built-in command's
    /// enablement, and `true` for the UI-level commands. Unknown ids are not available.
    pub fn command_enabled(&self, id: &str) -> bool {
        if let Some(c) = self.host.commands.iter().find(|c| c.id == id) {
            return c.enabled.as_ref().is_none_or(|f| f(self));
        }
        if UI_COMMANDS.contains(&id) {
            return true;
        }
        gridcraft_engine::find_command(id).is_some_and(|spec| (spec.enabled)(&self.session).is_ok())
    }

    /// Runs a host command, if `id` names one. The command is taken out while it runs, so a
    /// command that runs itself again gets an error instead of recursing.
    pub(crate) fn run_host_command(&mut self, id: &str, params: &Json) -> Option<Result<Json, String>> {
        let index = self.host.commands.iter().position(|c| c.id == id)?;
        if !self.host.commands.get(index).is_some_and(|c| c.enabled.as_ref().is_none_or(|f| f(self))) {
            return Some(Err(format!("command `{id}` is not available right now")));
        }
        let mut command = self.host.commands.remove(index);
        let result = (command.run)(self, params);
        // The command may have registered or removed commands meanwhile; keep its slot unless
        // something else took the id.
        if !self.has_host_command(&command.id) {
            let at = index.min(self.host.commands.len());
            self.host.commands.insert(at, command);
        }
        Some(result)
    }

    /// The sheet has the keyboard: always with [`KeyboardScope::Window`], while focused otherwise.
    pub fn has_keyboard(&self) -> bool {
        self.embed.keyboard == KeyboardScope::Window || self.host.keyboard
    }

    /// Give the sheet the keyboard (e.g. when the host shows it, or on a host shortcut). A host
    /// widget holding egui's focus lets go of it at the next frame.
    pub fn focus_keyboard(&mut self) {
        self.host.keyboard = true;
        self.host.focus_requested = true;
    }

    /// Take the keyboard back for the host. Keytips close; the cell being edited stays open.
    pub fn release_keyboard(&mut self) {
        self.host.keyboard = false;
        self.host.focus_requested = false;
        self.keytips.clear();
    }

    /// A press decides who has the keyboard ([`KeyboardScope::Focus`]): inside the sheet's region
    /// takes it, elsewhere in the host gives it up. A press on a window or menu above the page
    /// goes to the sheet when one of its dialogs is open, and leaves the keyboard where it was
    /// for menus (a menu opens from wherever the keyboard already is). A host widget that took
    /// egui's focus since the last frame (Tab, or the host's `request_focus`) takes the keyboard.
    pub(crate) fn track_keyboard(&mut self, ui: &egui::Ui) {
        if self.embed.keyboard == KeyboardScope::Window {
            return;
        }
        let ctx = ui.ctx();
        if std::mem::take(&mut self.host.focus_requested) {
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            self.host.focus_seen = None;
        } else if self.host.keyboard {
            let focused = ctx.memory(|m| m.focused());
            if focused.is_some() && focused != self.host.focus_seen {
                self.release_keyboard();
            }
        }
        let Some(pos) = ctx.input(|i| if i.pointer.any_pressed() { i.pointer.press_origin().or(i.pointer.interact_pos()) } else { None }) else {
            return;
        };
        let own = ui.layer_id();
        let hit = ctx.layer_id_at(pos);
        let keyboard = if hit == Some(own) || (hit.is_none() && own.order == egui::Order::Background) {
            ui.max_rect().intersect(ui.clip_rect()).contains(pos)
        } else if self.dialog.is_some() || self.message.is_some() {
            true
        } else if hit.is_some_and(|layer| matches!(layer.order, egui::Order::Foreground | egui::Order::Tooltip | egui::Order::Debug)) {
            self.host.keyboard
        } else {
            false
        };
        if keyboard != self.host.keyboard {
            if keyboard {
                // The click focuses whatever it lands on (the grid, the formula bar…) itself.
                self.host.keyboard = true;
            } else {
                self.release_keyboard();
            }
        }
    }

    /// Takes the keyboard events the sheet must not see out of this frame's input: all of them
    /// without the keyboard, the host's reserved ones with it. [`Self::restore_keys`] gives them
    /// back after the sheet has drawn, so host widgets drawn later still get them.
    pub(crate) fn withhold_keys(&mut self, ctx: &egui::Context) -> Vec<Event> {
        let all = !self.has_keyboard();
        if !all && self.embed.reserved_shortcuts.is_empty() && self.services.keep_key.is_none() {
            return Vec::new();
        }
        let events = ctx.input_mut(|i| std::mem::take(&mut i.events));
        let reserved = &self.embed.reserved_shortcuts;
        let mut keep = self.services.keep_key.take();
        let (withheld, rest): (Vec<Event>, Vec<Event>) =
            events.into_iter().partition(|e| is_keyboard(e) && (all || is_reserved(reserved, e) || keep.as_mut().is_some_and(|f| f(e))));
        self.services.keep_key = keep;
        ctx.input_mut(|i| i.events = rest);
        withheld
    }

    /// After the sheet drew: with [`KeyboardScope::Focus`] and the keyboard, the keyboard events
    /// it saw are used up (a host handling shortcuts after [`SheetApp::ui`] doesn't act on them
    /// twice); the withheld ones go back for the host.
    pub(crate) fn finish_keys(&mut self, ctx: &egui::Context, withheld: Vec<Event>, had_keyboard: bool) {
        if self.embed.keyboard == KeyboardScope::Focus && had_keyboard {
            ctx.input_mut(|i| i.events.retain(|e| !is_keyboard(e)));
            self.host.focus_seen = ctx.memory(|m| m.focused());
        }
        if !withheld.is_empty() {
            ctx.input_mut(|i| i.events.extend(withheld));
        }
    }

    /// Opens a workbook the host holds (any format `file.open` reads, by `name`'s extension or
    /// content) and shows it, without file dialogs. A cell still being edited is committed when
    /// it is valid and dropped otherwise. A password-protected workbook needs `password`; without
    /// it the error contains [`gridcraft_engine::io::PASSWORD_REQUIRED`].
    pub fn open_workbook_bytes(&mut self, name: &str, bytes: &[u8], password: Option<&str>) -> Result<Json, String> {
        if !self.commit_edit(0, 0, false, false) {
            self.cancel_edit();
            self.message = None;
        }
        let mut params = json!({"name": name, "base64": gridcraft_engine::io::base64_encode(bytes)});
        if let (Some(password), Some(map)) = (password, params.as_object_mut()) {
            map.insert("password".into(), json!(password));
        }
        let r = self.run("file.open", params);
        if let Ok(r) = &r {
            self.after_open(None, r);
        }
        self.after_engine();
        r
    }

    /// The active workbook encoded for the host: `(name, bytes)`, where `format` is `xlsx`, `csv`,
    /// `tsv`, `json` or `html` and the name carries its extension. A cell still being edited is
    /// committed first (an invalid entry is an error, shown to the user as when saving). The
    /// workbook stays dirty until the host calls [`Self::mark_saved`].
    pub fn workbook_bytes(&mut self, format: &str) -> Result<(String, Vec<u8>), String> {
        if !self.commit_edit(0, 0, false, false) {
            return Err(self.message.as_ref().map(|(_, m)| m.clone()).unwrap_or_else(|| "the cell being edited can't be committed".into()));
        }
        self.commit_text_box_edit()?;
        self.store_view_state();
        let r = self.session.run("file.saveBytes", json!({"format": format}))?;
        let bytes =
            r.get("base64").and_then(Json::as_str).and_then(gridcraft_engine::io::base64_decode).ok_or("the workbook could not be encoded")?;
        let name = r.get("name").and_then(Json::as_str).unwrap_or("Book1.xlsx").to_string();
        Ok((name, bytes))
    }

    /// The host stored the workbook: it is no longer dirty.
    pub fn mark_saved(&mut self) {
        if let Some(d) = self.session.active_mut() {
            d.saved = d.wb.clone();
        }
    }

    /// Save / Save As without a file when the host stores workbooks ([`crate::Services::save_workbook`]):
    /// hand it the .xlsx bytes and mark the workbook saved when it took them.
    pub(crate) fn host_save(&mut self, id: &str, params: &Json) -> Option<Result<Json, String>> {
        let save_as = match id {
            "file.save" => false,
            "file.saveAs" => true,
            _ => return None,
        };
        let has_path = params.get("path").and_then(Json::as_str).is_some_and(|p| !p.is_empty());
        let has_file = self.session.active().is_some_and(|d| d.path.is_some());
        if self.services.save_workbook.is_none() || has_path || (has_file && !save_as) {
            return None;
        }
        let saved = self.workbook_bytes("xlsx").and_then(|(name, bytes)| {
            let save = self.services.save_workbook.as_mut().ok_or("the host can't store workbooks")?;
            save(&HostSave { name: &name, bytes: &bytes, save_as })?;
            Ok(json!({"name": name, "bytes": bytes.len(), "host": true}))
        });
        if saved.is_ok() {
            self.mark_saved();
        }
        Some(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_defaults_draw_everything() {
        let o = EmbedOptions::default();
        assert!(o.title_bar && o.quick_access_toolbar && o.backstage && o.status_bar && o.window_controls && o.global_style);
        assert_eq!(o.keyboard, KeyboardScope::Window);
        assert!(o.reserved_shortcuts.is_empty());
        let e = EmbedOptions::embedded();
        assert!(!e.title_bar && !e.quick_access_toolbar && !e.backstage && !e.window_controls && !e.global_style);
        assert_eq!(e.keyboard, KeyboardScope::Focus);
    }

    #[test]
    fn host_command_ids_are_namespaced_and_new() {
        assert!(check_command_id("contoso.refresh").is_ok());
        assert!(check_command_id("contoso.data-sync_2").is_ok());
        for bad in ["", "refresh", ".refresh", "contoso.", "contoso refresh", "contoso.é", "home.bold", "ui.anything", "app.quit", "view.theme"] {
            assert!(check_command_id(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(check_command_id(&format!("contoso.{}", "x".repeat(MAX_COMMAND_ID))).is_err());
    }

    #[test]
    fn reserved_shortcuts_match_exactly_and_cover_clipboard_events() {
        let reserved = [KeyboardShortcut::new(Modifiers::COMMAND, Key::S), KeyboardShortcut::new(Modifiers::COMMAND, Key::C)];
        let key = |key, modifiers| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        assert!(is_reserved(&reserved, &key(Key::S, Modifiers::COMMAND)));
        assert!(is_reserved(&reserved, &key(Key::S, Modifiers::MAC_CMD | Modifiers::COMMAND)));
        assert!(!is_reserved(&reserved, &key(Key::S, Modifiers::COMMAND | Modifiers::SHIFT)), "Save As stays with the sheet");
        assert!(!is_reserved(&reserved, &key(Key::S, Modifiers::NONE)));
        assert!(is_reserved(&reserved, &Event::Copy));
        assert!(!is_reserved(&reserved, &Event::Cut));
        assert!(!is_reserved(&reserved, &Event::Text("s".into())));
    }
}
