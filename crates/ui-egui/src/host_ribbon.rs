//! Ribbon tabs an embedding application adds after the built-in ones.
//!
//! A [`RibbonTab`] holds groups of buttons ([`RibbonGroup`], [`RibbonButton`]) that GridCraft lays
//! out like its own tabs, and/or a draw callback that gets the app and the ribbon body's `Ui`.
//! Buttons run commands by id, built-in or the host's own ([`crate::embed::HostCommand`]), so a
//! button does exactly what the control channel's `engine.execute` does. Host tabs collapse and
//! expand with the ribbon, answer keytips (the tab's `keytip`, then each button's) and work on the
//! web like everything else.

use std::sync::Arc;

use egui::{Align2, Color32, Rect, Response, Sense, Ui, pos2, vec2};
use serde_json::Value as Json;

use crate::SheetApp;
use crate::icons::{self, Icon};
use crate::keytips::{HostEntry, keytip_table};
use crate::theme::{self, Tokens};

/// Paints a host icon into `rect` with the colour the button wants (text or disabled).
pub type PaintIcon = Arc<dyn Fn(&egui::Painter, Rect, Color32)>;
/// Draws a host tab's content (after its groups); gets the app and the ribbon body.
pub type DrawTab = Box<dyn FnMut(&mut SheetApp, &mut Ui)>;

/// A ribbon tab added by the embedding application.
pub struct RibbonTab {
    /// The tab's name on the ribbon, shown as given. It is also its identity: `ui.ribbonTab
    /// {"tab": label}` selects it. It can't be a built-in tab's name or another host tab's.
    pub label: String,
    /// The access keys after Alt or F10: one to three letters or digits ("Y", "Y2"). They can't
    /// clash with the built-in keytips or another host tab's.
    pub keytip: Option<String>,
    /// Groups of buttons, separated like the built-in groups.
    pub groups: Vec<RibbonGroup>,
    /// Immediate-mode content after the groups (or instead of them).
    pub draw: Option<DrawTab>,
}

impl RibbonTab {
    pub fn new(label: impl Into<String>) -> Self {
        RibbonTab { label: label.into(), keytip: None, groups: Vec::new(), draw: None }
    }

    pub fn keytip(mut self, keys: impl Into<String>) -> Self {
        self.keytip = Some(keys.into());
        self
    }

    pub fn group(mut self, group: RibbonGroup) -> Self {
        self.groups.push(group);
        self
    }

    /// Draw (part of) the tab yourself, e.g. with [`crate::widgets::big_button`].
    pub fn draw(mut self, draw: impl FnMut(&mut SheetApp, &mut Ui) + 'static) -> Self {
        self.draw = Some(Box::new(draw));
        self
    }
}

/// Buttons that belong together; groups are separated by a rule, as on the built-in tabs.
#[derive(Clone, Default)]
pub struct RibbonGroup {
    /// Names the group in the command search ("Tab › Group") and for assistive technology.
    pub label: String,
    pub buttons: Vec<RibbonButton>,
}

impl RibbonGroup {
    pub fn new(label: impl Into<String>) -> Self {
        RibbonGroup { label: label.into(), buttons: Vec::new() }
    }

    pub fn button(mut self, button: RibbonButton) -> Self {
        self.buttons.push(button);
        self
    }
}

/// Large buttons stand alone (icon over a one- or two-line label); small ones stack three high
/// (icon beside the label).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonSize {
    Large,
    Small,
}

#[derive(Clone, Default)]
pub enum ButtonIcon {
    #[default]
    None,
    /// One of GridCraft's own icons.
    Builtin(Icon),
    /// The host's icon, painted in code.
    Paint(PaintIcon),
}

/// A button that runs a command when clicked or when its keytip is typed.
#[derive(Clone)]
pub struct RibbonButton {
    /// The label; `\n` breaks a large button's label into two lines.
    pub label: String,
    /// Hover text (the label when empty).
    pub tooltip: String,
    /// The command to run: a host command id or any built-in one.
    pub command: String,
    pub params: Json,
    pub icon: ButtonIcon,
    pub size: ButtonSize,
    /// One or two letters or digits typed after the tab's keytip.
    pub keytip: Option<String>,
}

impl RibbonButton {
    pub fn new(label: impl Into<String>, command: impl Into<String>) -> Self {
        RibbonButton {
            label: label.into(),
            tooltip: String::new(),
            command: command.into(),
            params: Json::Object(Default::default()),
            icon: ButtonIcon::None,
            size: ButtonSize::Large,
            keytip: None,
        }
    }

    pub fn params(mut self, params: Json) -> Self {
        self.params = params;
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = tooltip.into();
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = ButtonIcon::Builtin(icon);
        self
    }

    pub fn paint_icon(mut self, paint: impl Fn(&egui::Painter, Rect, Color32) + 'static) -> Self {
        self.icon = ButtonIcon::Paint(Arc::new(paint));
        self
    }

    pub fn small(mut self) -> Self {
        self.size = ButtonSize::Small;
        self
    }

    pub fn keytip(mut self, keys: impl Into<String>) -> Self {
        self.keytip = Some(keys.into());
        self
    }
}

/// The built-in tab names a host tab can't take (contextual tabs and File included).
fn reserved_label(label: &str) -> bool {
    crate::ribbon::TABS.iter().chain(["File", "Table Design", "Chart Design", crate::paper_view::TAB].iter()).any(|t| t.eq_ignore_ascii_case(label))
}

/// Upper-cases keytip letters and checks their shape: 1..=`max` ASCII letters or digits.
fn normalize_keytip(keys: &str, max: usize) -> Result<String, String> {
    let keys = keys.trim().to_ascii_uppercase();
    if keys.is_empty() || keys.len() > max || !keys.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("keytip {keys:?} must be 1 to {max} letters or digits"));
    }
    Ok(keys)
}

/// Whether two sequences can't both be typed: one is the other or starts it.
fn overlaps(a: &str, b: &str) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

impl SheetApp {
    /// Adds a ribbon tab after the built-in ones (and after host tabs added before it). Checks
    /// the label and the keytips (see [`RibbonTab`]); button commands are looked up when pressed,
    /// so they can be registered later (until then the button is greyed out).
    pub fn add_ribbon_tab(&mut self, mut tab: RibbonTab) -> Result<(), String> {
        tab.label = tab.label.trim().to_string();
        if tab.label.is_empty() {
            return Err("a ribbon tab needs a label".into());
        }
        if reserved_label(&tab.label) || self.host.tabs.iter().any(|t| t.label.eq_ignore_ascii_case(&tab.label)) {
            return Err(format!("there is already a ribbon tab called {:?}", tab.label));
        }
        if let Some(keys) = &tab.keytip {
            let keys = normalize_keytip(keys, 3)?;
            if keytip_table::conflicts(&keys) {
                return Err(format!("keytip {keys:?} is taken by a built-in tab or command"));
            }
            if self.host.tabs.iter().filter_map(|t| t.keytip.as_deref()).any(|other| overlaps(other, &keys)) {
                return Err(format!("keytip {keys:?} overlaps another host tab's"));
            }
            tab.keytip = Some(keys);
        }
        let mut seen: Vec<String> = Vec::new();
        for button in tab.groups.iter_mut().flat_map(|g| g.buttons.iter_mut()) {
            let Some(keys) = &button.keytip else { continue };
            if tab.keytip.is_none() {
                return Err(format!("button {:?} has a keytip but its tab has none", button.label));
            }
            let keys = normalize_keytip(keys, 2)?;
            if seen.iter().any(|other| overlaps(other, &keys)) {
                return Err(format!("button keytip {keys:?} overlaps another button's on this tab"));
            }
            seen.push(keys.clone());
            button.keytip = Some(keys);
        }
        self.host.tabs.push(tab);
        Ok(())
    }

    /// Removes a host tab by label; `false` when there was none. A removed active tab leaves
    /// the ribbon on Home.
    pub fn remove_ribbon_tab(&mut self, label: &str) -> bool {
        let before = self.host.tabs.len();
        self.host.tabs.retain(|t| t.label != label);
        if self.ui.ribbon_tab == label {
            self.ui.ribbon_tab = "Home".into();
        }
        self.host.tabs.len() != before
    }

    /// The host tabs' labels, in ribbon order.
    pub fn ribbon_tab_labels(&self) -> Vec<String> {
        self.host.tabs.iter().map(|t| t.label.clone()).collect()
    }

    /// The keytip sequences of the host tabs and their buttons, for [`crate::keytips::KeyTips`].
    pub(crate) fn host_keytips(&self) -> Vec<(String, HostEntry)> {
        let mut out = Vec::new();
        for (ti, tab) in self.host.tabs.iter().enumerate() {
            let Some(tab_keys) = &tab.keytip else { continue };
            out.push((tab_keys.clone(), HostEntry::Tab(ti)));
            for (gi, group) in tab.groups.iter().enumerate() {
                for (bi, button) in group.buttons.iter().enumerate() {
                    if let Some(keys) = &button.keytip {
                        out.push((format!("{tab_keys}{keys}"), HostEntry::Button(ti, gi, bi)));
                    }
                }
            }
        }
        out
    }

    /// Selects host tab `index` (a keytip or a click), expanding a collapsed ribbon.
    pub(crate) fn select_host_tab(&mut self, index: usize) {
        if let Some(tab) = self.host.tabs.get(index) {
            self.ui.ribbon_tab = tab.label.clone();
            self.ui.ribbon_collapsed = false;
        }
    }

    /// Presses a host button: runs its command as a ribbon click does.
    pub(crate) fn press_host_button(&mut self, tab: usize, group: usize, button: usize) {
        let Some(b) = self.host.tabs.get(tab).and_then(|t| t.groups.get(group)).and_then(|g| g.buttons.get(button)) else { return };
        let (command, params) = (b.command.clone(), b.params.clone());
        if self.command_enabled(&command) {
            self.run_or_alert(&command, params);
        }
    }
}

/// Draws host tab `index` in the ribbon body.
pub(crate) fn show(app: &mut SheetApp, ui: &mut Ui, index: usize) {
    let Some(tab) = app.host.tabs.get(index) else { return };
    let label = tab.label.clone();
    let tab_keys = tab.keytip.clone();
    // A copy of what is drawn, so a press can run its command with the app afterwards.
    let groups: Vec<Vec<RibbonButton>> = tab.groups.iter().map(|g| g.buttons.clone()).collect();
    let mut pressed: Option<(usize, usize)> = None;
    for (gi, buttons) in groups.iter().enumerate() {
        if gi > 0 {
            crate::ribbon::sep(ui);
        }
        let mut bi = 0;
        while let Some(first) = buttons.get(bi) {
            if first.size == ButtonSize::Large {
                if button(app, ui, first, tab_keys.as_deref()).clicked() {
                    pressed = Some((gi, bi));
                }
                bi += 1;
                continue;
            }
            // Up to three small buttons in a column.
            let column: Vec<(usize, &RibbonButton)> =
                buttons.iter().enumerate().skip(bi).take_while(|(_, b)| b.size == ButtonSize::Small).take(3).collect();
            bi += column.len().max(1);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(2.0, 1.0);
                for (i, b) in column {
                    if button(app, ui, b, tab_keys.as_deref()).clicked() {
                        pressed = Some((gi, i));
                    }
                }
            });
        }
    }
    if let Some(mut draw) = app.host.tabs.get_mut(index).and_then(|t| t.draw.take()) {
        if !groups.is_empty() {
            crate::ribbon::sep(ui);
        }
        draw(app, ui);
        // Put the callback back unless the tab went away or got a new one meanwhile.
        if let Some(tab) = app.host.tabs.iter_mut().find(|t| t.label == label)
            && tab.draw.is_none()
        {
            tab.draw = Some(draw);
        }
    }
    if let Some((group, button)) = pressed
        && let Some(tab) = app.host.tabs.iter().position(|t| t.label == label)
    {
        app.press_host_button(tab, group, button);
    }
}

/// One host button, large or small, greyed out when its command can't run now.
fn button(app: &SheetApp, ui: &mut Ui, b: &RibbonButton, tab_keys: Option<&str>) -> Response {
    let t = Tokens::get(ui.ctx());
    let enabled = app.command_enabled(&b.command);
    let color = if enabled { t.text } else { t.text_disabled };
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let resp = match b.size {
        ButtonSize::Large => {
            let font = theme::ui_font(11.5);
            let lines: Vec<_> = b.label.split('\n').map(|line| ui.painter().layout_no_wrap(line.to_string(), font.clone(), color)).collect();
            let text_w = lines.iter().map(|l| l.size().x).fold(0.0, f32::max);
            let (rect, resp) = ui.allocate_exact_size(vec2((text_w + 12.0).max(44.0), 72.0), sense);
            hover_fill(ui, &resp, rect, 5.0, &t);
            paint_icon(ui, &b.icon, Rect::from_center_size(pos2(rect.center().x, rect.top() + 21.0), vec2(30.0, 30.0)), color);
            for (i, line) in lines.into_iter().enumerate() {
                let y = rect.top() + 46.0 + i as f32 * 13.0;
                ui.painter().galley(pos2(rect.center().x - line.size().x / 2.0, y - line.size().y / 2.0), line, color);
            }
            resp
        }
        ButtonSize::Small => {
            let font = theme::ui_font(12.5);
            let text = b.label.replace('\n', " ");
            let tw = ui.painter().layout_no_wrap(text.clone(), font.clone(), color).size().x;
            let icon_w = if matches!(b.icon, ButtonIcon::None) { 6.0 } else { 24.0 };
            let (rect, resp) = ui.allocate_exact_size(vec2(icon_w + tw + 6.0, 23.0), sense);
            hover_fill(ui, &resp, rect, 4.0, &t);
            paint_icon(ui, &b.icon, Rect::from_center_size(pos2(rect.left() + 12.0, rect.center().y), vec2(16.0, 16.0)), color);
            ui.painter().text(pos2(rect.left() + icon_w, rect.center().y), Align2::LEFT_CENTER, text, font, color);
            resp
        }
    };
    let name = b.label.replace('\n', " ");
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &name));
    if let (Some(tab_keys), Some(keys)) = (tab_keys, &b.keytip) {
        crate::ribbon::keytip(app, ui, &format!("{tab_keys}{keys}"), resp.rect.center_bottom());
    }
    let tip = if b.tooltip.is_empty() { name } else { b.tooltip.clone() };
    resp.on_hover_text(tip)
}

fn hover_fill(ui: &Ui, resp: &Response, rect: Rect, radius: f32, t: &Tokens) {
    if resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, radius, t.pressed);
    } else if resp.hovered() && resp.sense.senses_click() {
        ui.painter().rect_filled(rect, radius, t.hover);
    }
}

fn paint_icon(ui: &Ui, icon: &ButtonIcon, rect: Rect, color: Color32) {
    match icon {
        ButtonIcon::None => {}
        ButtonIcon::Builtin(icon) => icons::paint(ui.painter(), rect, *icon, color),
        ButtonIcon::Paint(paint) => paint(ui.painter(), rect, color),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> SheetApp {
        let mut session = gridcraft_engine::Session::new();
        session.new_workbook();
        SheetApp::new(session, Default::default())
    }

    #[test]
    fn tab_labels_and_keytips_are_checked() {
        let mut app = app();
        assert!(app.add_ribbon_tab(RibbonTab::new("  ")).is_err());
        assert!(app.add_ribbon_tab(RibbonTab::new("Home")).is_err(), "a built-in tab");
        assert!(app.add_ribbon_tab(RibbonTab::new("table design")).is_err(), "a contextual tab");
        assert!(app.add_ribbon_tab(RibbonTab::new("Tools").keytip("H")).is_err(), "Home's keytip");
        assert!(app.add_ribbon_tab(RibbonTab::new("Tools").keytip("J")).is_err(), "Draw's implicit level");
        assert!(app.add_ribbon_tab(RibbonTab::new("Tools").keytip("y!")).is_err());
        assert!(app.add_ribbon_tab(RibbonTab::new("Tools").keytip("abcd")).is_err());
        let orphan = RibbonGroup::new("G").button(RibbonButton::new("Go", "contoso.go").keytip("G"));
        assert!(app.add_ribbon_tab(RibbonTab::new("Tools").group(orphan)).is_err(), "a button keytip needs a tab keytip");
        let clash =
            RibbonGroup::new("G").button(RibbonButton::new("A", "contoso.a").keytip("R")).button(RibbonButton::new("B", "contoso.b").keytip("R1"));
        assert!(app.add_ribbon_tab(RibbonTab::new("Tools").keytip("Y").group(clash)).is_err(), "R and R1 overlap");

        let group = RibbonGroup::new("Data").button(RibbonButton::new("Refresh", "contoso.refresh").keytip("r"));
        app.add_ribbon_tab(RibbonTab::new(" Tools ").keytip("y").group(group)).unwrap();
        assert_eq!(app.ribbon_tab_labels(), vec!["Tools".to_string()]);
        assert_eq!(app.host_keytips(), vec![("Y".to_string(), HostEntry::Tab(0)), ("YR".to_string(), HostEntry::Button(0, 0, 0))]);
        assert!(app.add_ribbon_tab(RibbonTab::new("tools")).is_err(), "labels are unique");
        assert!(app.add_ribbon_tab(RibbonTab::new("More").keytip("Y2")).is_err(), "overlaps Tools' Y");
        app.add_ribbon_tab(RibbonTab::new("More").keytip("Z")).unwrap();

        app.ui.ribbon_tab = "Tools".into();
        assert!(app.remove_ribbon_tab("Tools"));
        assert_eq!(app.ui.ribbon_tab, "Home");
        assert!(!app.remove_ribbon_tab("Tools"));
        assert_eq!(app.host_keytips(), vec![("Z".to_string(), HostEntry::Tab(0))]);
    }
}
