//! The sheet as one panel of a host application's window: chrome, keyboard, host colours, host
//! ribbon tabs and commands, and workbooks the host stores as bytes.
//!
//! The rendered checks need a GPU adapter (wgpu); without one they are skipped. Set
//! `GRIDCRAFT_EMBED_SNAPSHOTS=<dir>` to write the renders there and look at them.

use std::cell::{Cell, RefCell};
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Color32, Event, Key, KeyboardShortcut, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::kittest::Queryable as _;
use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, Value};
use gridcraft_ui_egui::embed::{EmbedOptions, HostCommand, KeyboardScope};
use gridcraft_ui_egui::host_ribbon::{RibbonButton, RibbonGroup, RibbonTab};
use gridcraft_ui_egui::icons::Icon;
use gridcraft_ui_egui::theme::{self, HostTheme, Palette, Tokens};
use gridcraft_ui_egui::{Services, SheetApp, control};
use serde_json::{Value as Json, json};

/// A host window: its own navigation panel with a text field, and the sheet beside it.
struct Host {
    app: SheetApp,
    note: String,
    note_rect: Rect,
    /// Cmd/Ctrl+S presses the host handled itself, after the sheet.
    host_saves: usize,
    /// egui's quit shortcut (Cmd/Ctrl+Q), as the host window sees it after the sheet.
    host_quits: usize,
    frame: u32,
    /// Frames after whose sheet the host installs a font set of its own.
    host_fonts_on: Vec<u32>,
}

type Harness = egui_kittest::Harness<'static, Host>;

const HOST_PANEL: f32 = 220.0;

fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}

fn harness(app: SheetApp) -> Harness {
    let host = Host { app, note: String::new(), note_rect: Rect::NOTHING, host_saves: 0, host_quits: 0, frame: 0, host_fonts_on: Vec::new() };
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1280.0, 800.0)).build_ui_state(
        |ui, host: &mut Host| {
            let ctx = ui.ctx().clone();
            host.frame += 1;
            host.app.logic(&ctx);
            egui::Panel::left("host_panel").exact_size(HOST_PANEL).show(ui, |ui| {
                ui.label("Host navigation");
                host.note_rect = ui.add(egui::TextEdit::singleline(&mut host.note).id(egui::Id::new("host.note"))).rect;
            });
            egui::CentralPanel::default().show(ui, |ui| host.app.ui(ui));
            // The host's own shortcut, handled after the sheet drew.
            if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S))) {
                host.host_saves += 1;
            }
            if ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::Key { key: Key::Q, pressed: true, modifiers, .. } if modifiers.command))) {
                host.host_quits += 1;
            }
            if host.host_fonts_on.contains(&host.frame) {
                ctx.set_fonts(host_fonts());
            }
        },
        host,
    );
    // The host picks its own appearance (egui's fallback would be dark).
    h.ctx.set_theme(egui::Theme::Light);
    h.run_steps(4);
    h
}

/// A host's own font set: egui's, with the proportional faces in another order.
fn host_fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(chain) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        chain.reverse();
    }
    fonts
}

fn embedded(services: Services) -> SheetApp {
    SheetApp::embedded(session(), services)
}

fn has_label(h: &Harness, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

fn click_at(h: &mut Harness, pos: Pos2) {
    h.input_mut().events.extend([
        Event::PointerMoved(pos),
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
    ]);
    h.run_steps(3);
}

fn key(h: &mut Harness, key: Key, modifiers: Modifiers) {
    h.input_mut().events.extend([
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers },
        Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers },
    ]);
    h.run_steps(2);
}

fn type_text(h: &mut Harness, text: &str) {
    h.input_mut().events.push(Event::Text(text.into()));
    h.run_steps(2);
}

/// A keytip letter, as a keyboard sends it: the key and its text.
fn letter(h: &mut Harness, k: Key) {
    h.input_mut().events.extend([
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE },
        Event::Text(k.name().to_ascii_lowercase()),
        Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE },
    ]);
    h.run_steps(2);
}

fn grid_point(h: &Harness) -> Pos2 {
    let r = h.state().app.grid.rect.expect("the grid is drawn");
    r.min + vec2(120.0, 80.0)
}

fn value(app: &SheetApp, cell: &str) -> Value {
    app.session.active().unwrap().wb.active().unwrap().value(CellRef::parse(cell).unwrap())
}

fn palette() -> (Palette, Palette) {
    let c = Color32::from_rgb;
    let light = Palette {
        accent: c(0x4F, 0x46, 0xE5),
        selection: c(0x4F, 0x46, 0xE5),
        surface: c(0xEE, 0xF0, 0xF6),
        raised: c(0xFF, 0xFF, 0xFF),
        border: c(0xD5, 0xD9, 0xE4),
        text: c(0x1E, 0x22, 0x33),
        text_muted: c(0x5B, 0x61, 0x78),
    };
    let dark = Palette {
        accent: c(0x8B, 0x85, 0xFF),
        selection: c(0x8B, 0x85, 0xFF),
        surface: c(0x1B, 0x1E, 0x2B),
        raised: c(0x26, 0x2A, 0x3A),
        border: c(0x3A, 0x3F, 0x55),
        text: c(0xE8, 0xEA, 0xF2),
        text_muted: c(0xA3, 0xA8, 0xBF),
    };
    (light, dark)
}

/// Renders the harness when a GPU adapter is available (saving the PNG when asked to). Without
/// one, egui_kittest's renderer panics while it starts: that is caught, and the rendered checks
/// of every test are skipped from then on.
fn render(h: &mut Harness, name: &str) -> Option<image::RgbaImage> {
    static NO_RENDERER: AtomicBool = AtomicBool::new(false);
    if NO_RENDERER.load(Ordering::Relaxed) {
        return None;
    }
    match std::panic::catch_unwind(AssertUnwindSafe(|| h.render())) {
        Ok(Ok(img)) => {
            if let Ok(dir) = std::env::var("GRIDCRAFT_EMBED_SNAPSHOTS") {
                let _ = std::fs::create_dir_all(&dir);
                img.save(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
            }
            Some(img)
        }
        Ok(Err(e)) => {
            eprintln!("no GPU renderer ({e}); skipping the rendered checks for {name}");
            NO_RENDERER.store(true, Ordering::Relaxed);
            None
        }
        Err(_) => {
            eprintln!("no GPU adapter; skipping the rendered checks for {name}");
            NO_RENDERER.store(true, Ordering::Relaxed);
            None
        }
    }
}

/// Pixels of `color` (within a small rounding tolerance) inside `area` (points; the harness
/// renders at 1 pixel per point).
fn count(img: &image::RgbaImage, area: Rect, color: Color32) -> usize {
    let near = |a: u8, b: u8| a.abs_diff(b) <= 2;
    img.enumerate_pixels()
        .filter(|(x, y, _)| area.contains(pos2(*x as f32 + 0.5, *y as f32 + 0.5)))
        .filter(|(_, _, p)| near(p[0], color.r()) && near(p[1], color.g()) && near(p[2], color.b()))
        .count()
}

#[test]
fn embedded_sheet_leaves_the_window_chrome_to_the_host() {
    let mut standalone = harness(SheetApp::new(session(), Services::default()));
    let mut embedded = harness(embedded(Services::default()));
    for label in ["AutoSave", "File", "Ready"] {
        assert!(has_label(&standalone, label), "the standalone window shows {label}");
        assert!(!has_label(&embedded, label), "the embedded sheet has no {label}");
    }
    // The ribbon, formula bar, grid and sheet tabs stay; the grid moves up by the title bar.
    let (top_standalone, top_embedded) = (standalone.state().app.grid.rect.unwrap().top(), embedded.state().app.grid.rect.unwrap().top());
    assert!(top_embedded < top_standalone - 20.0, "{top_embedded} vs {top_standalone}");
    assert!(embedded.state().app.grid.rect.unwrap().left() >= HOST_PANEL, "the sheet stays in its panel");
    if let (Some(a), Some(b)) = (render(&mut standalone, "standalone"), render(&mut embedded, "embedded")) {
        // Embedded, the ribbon tabs (the active one underlined in the accent) sit where the
        // standalone title bar is.
        let top = Rect::from_min_size(pos2(HOST_PANEL, 0.0), vec2(1280.0 - HOST_PANEL, 52.0));
        assert_eq!(count(&a, top, Tokens::light().accent), 0, "the standalone title bar has no accent");
        assert!(count(&b, top, Tokens::light().accent) > 10, "the embedded ribbon starts at the top");
    }
    let ctx = embedded.ctx.clone();
    let outcome = |h: &mut Harness, method: &str, params: Json| match control::handle(&mut h.state_mut().app, &ctx, method, &params) {
        control::Outcome::Done(v) => v,
        control::Outcome::Screenshot { .. } => panic!("no screenshot was asked for"),
    };
    // What belongs to the host's window is refused, not done to the host.
    for (method, params) in [
        ("engine.execute", json!({"command": "ui.backstage", "params": {"page": "info"}})),
        ("engine.execute", json!({"command": "view.theme", "params": {"mode": "dark"}})),
        ("engine.execute", json!({"command": "view.interfaceScale", "params": {"percent": 150}})),
        ("app.quit", json!({})),
        ("ui.resize", json!({"width": 800, "height": 600})),
    ] {
        assert_eq!(outcome(&mut embedded, method, params)["ok"], false, "{method}");
    }
    assert!(embedded.state().app.backstage.is_none());
    // File › Options (the command, a keytip) says there is no File view instead of doing nothing.
    embedded.state_mut().app.run_or_alert("file.options", json!({}));
    assert!(embedded.state().app.backstage.is_none());
    assert!(embedded.state().app.toast.as_ref().is_some_and(|(m, _)| m.contains("File view")));
    // The host's global style, zoom and fonts stay the host's.
    assert_eq!(embedded.ctx.global_style().visuals.panel_fill, egui::Visuals::light().panel_fill);
    assert_eq!(embedded.ctx.zoom_factor(), 1.0);
    let proportional = egui::FontDefinitions::default().families.get(&egui::FontFamily::Proportional).cloned();
    let fonts = embedded.ctx.fonts(|f| f.definitions().clone());
    assert_eq!(fonts.families.get(&egui::FontFamily::Proportional).cloned(), proportional, "the host's default font stays");
    assert!(theme::fonts_installed(&embedded.ctx), "GridCraft's families are added");
    // A host that replaces the whole font set gets GridCraft's families back, without a crash.
    embedded.ctx.set_fonts(egui::FontDefinitions::default());
    embedded.run_steps(4);
    assert!(theme::fonts_installed(&embedded.ctx));
    // The standalone window keeps owning everything.
    assert_eq!(outcome(&mut standalone, "engine.execute", json!({"command": "ui.backstage", "params": {"page": "info"}}))["ok"], true);
    standalone.run_steps(2);
    assert!(standalone.state().app.backstage.is_some());
}

#[test]
fn keyboard_belongs_to_the_panel_the_user_clicked() {
    let saved: Rc<RefCell<Vec<(String, usize, bool)>>> = Rc::default();
    let sink = saved.clone();
    let mut h = harness(embedded(Services {
        save_workbook: Some(Box::new(move |s| {
            sink.borrow_mut().push((s.name.to_string(), s.bytes.len(), s.save_as));
            Ok(())
        })),
        keep_key: Some(Box::new(|e| matches!(e, Event::Key { key: Key::F2, .. }))),
        ..Default::default()
    }));
    assert!(!h.state().app.has_keyboard(), "the host decides when the sheet gets the keyboard");
    type_text(&mut h, "x");
    assert!(h.state().app.editor.is_none(), "typing without the keyboard doesn't edit a cell");

    // The host's own field gets its typing, the sheet nothing.
    let note = h.state().note_rect.center();
    click_at(&mut h, note);
    type_text(&mut h, "hello");
    assert_eq!(h.state().note, "hello");
    assert!(h.state().app.editor.is_none() && !h.state().app.has_keyboard());
    key(&mut h, Key::F10, Modifiers::NONE);
    letter(&mut h, Key::H);
    assert!(h.state().app.keytips.prefix().is_none(), "keytips stay off without the keyboard");
    assert_eq!(h.state().note, "helloh", "the letter went to the host's field");

    // A click into the grid gives the sheet the keyboard; typing edits the active cell.
    let grid = grid_point(&h);
    click_at(&mut h, grid);
    assert!(h.state().app.has_keyboard());
    type_text(&mut h, "7");
    assert!(h.state().app.editor.is_some(), "typing starts an edit");
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().app.editor.is_none());
    assert_eq!(h.state().note, "helloh", "the host field didn't get the sheet's typing");

    // The host's filter keeps F2 for itself.
    key(&mut h, Key::F2, Modifiers::NONE);
    assert!(h.state().app.editor.is_none(), "F2 went to the host");

    // Save with no file goes to the host's store, which marks the workbook saved.
    assert!(h.state().app.session.active().unwrap().is_dirty());
    key(&mut h, Key::S, Modifiers::COMMAND);
    assert_eq!(saved.borrow().as_slice(), &[(String::from("Book1.xlsx"), saved.borrow()[0].1, false)]);
    assert!(saved.borrow()[0].1 > 0);
    assert!(!h.state().app.session.active().unwrap().is_dirty());
    assert_eq!(h.state().host_saves, 0, "the sheet handled Cmd/Ctrl+S");

    // A reserved shortcut is the host's even while the sheet has the keyboard.
    h.state_mut().app.embed.reserved_shortcuts.push(KeyboardShortcut::new(Modifiers::COMMAND, Key::S));
    key(&mut h, Key::S, Modifiers::COMMAND);
    assert_eq!(h.state().host_saves, 1);
    assert_eq!(saved.borrow().len(), 1, "the sheet never saw it");

    // Clicking back into the host panel gives the keyboard back.
    click_at(&mut h, note);
    assert!(!h.state().app.has_keyboard());
    type_text(&mut h, "!");
    assert_eq!(h.state().note, "helloh!");
    assert!(h.state().app.editor.is_none());
    // The host can hand the keyboard over itself.
    h.state_mut().app.focus_keyboard();
    assert!(h.state().app.has_keyboard());
    h.state_mut().app.release_keyboard();
    assert!(!h.state().app.has_keyboard());
}

#[test]
fn keys_stay_with_the_panel_that_owns_them() {
    let mut app = embedded(Services::default());
    app.embed.reserved_shortcuts.extend([KeyboardShortcut::new(Modifiers::NONE, Key::Slash), KeyboardShortcut::new(Modifiers::ALT, Key::Num1)]);
    let mut h = harness(app);
    // Alt alone opens keytips on Windows and Linux.
    h.ctx.set_os(egui::os::OperatingSystem::Windows);
    let grid = grid_point(&h);
    click_at(&mut h, grid);
    assert!(h.state().app.has_keyboard());

    // Tab and the arrows move the selection; the keyboard stays with the sheet.
    let before = h.state().app.session.active().unwrap().selection.active;
    key(&mut h, Key::Tab, Modifiers::NONE);
    key(&mut h, Key::ArrowDown, Modifiers::NONE);
    let after = h.state().app.session.active().unwrap().selection.active;
    assert_eq!((after.row, after.col), (before.row + 1, before.col + 1));
    assert!(h.state().app.has_keyboard());

    // egui's quit shortcut is the host window's, even while the sheet has the keyboard.
    key(&mut h, Key::Q, Modifiers::COMMAND);
    assert_eq!(h.state().host_quits, 1);

    // A reserved key keeps the text it types: "/" doesn't start an edit.
    h.input_mut().events.extend([
        Event::Key { key: Key::Slash, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE },
        Event::Text("/".into()),
        Event::Key { key: Key::Slash, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE },
    ]);
    h.run_steps(2);
    assert!(h.state().app.editor.is_none(), "the reserved key's text went to the host");

    // A reserved Alt+1: releasing Alt afterwards is not a keytip tap.
    for (k, pressed, modifiers) in [
        (Key::AltLeft, true, Modifiers::ALT),
        (Key::Num1, true, Modifiers::ALT),
        (Key::Num1, false, Modifiers::ALT),
        (Key::AltLeft, false, Modifiers::NONE),
    ] {
        h.input_mut().events.push(Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers });
        h.run_steps(1);
    }
    h.run_steps(1);
    assert!(h.state().app.keytips.prefix().is_none(), "the host's Alt+1 doesn't open the sheet's keytips");
    // The same in one frame.
    h.input_mut().events.extend(
        [(Key::AltLeft, true, Modifiers::ALT), (Key::Num1, true, Modifiers::ALT), (Key::AltLeft, false, Modifiers::NONE)]
            .map(|(k, pressed, modifiers)| Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers }),
    );
    h.run_steps(2);
    assert!(h.state().app.keytips.prefix().is_none());
    // A plain Alt tap still opens them.
    for (pressed, modifiers) in [(true, Modifiers::ALT), (false, Modifiers::NONE)] {
        h.input_mut().events.push(Event::Key { key: Key::AltLeft, physical_key: None, pressed, repeat: false, modifiers });
        h.run_steps(1);
    }
    assert_eq!(h.state().app.keytips.prefix(), Some(""));
    key(&mut h, Key::Escape, Modifiers::NONE);

    // Shift+Tab between the fields of the sheet's own dialog keeps the keyboard with the sheet
    // (egui moves that focus between frames).
    h.state_mut().app.open_dialog("find", json!({"replace": true}));
    h.run_steps(3);
    let fields: Vec<Rect> = h.query_all_by_role(egui::accesskit::Role::TextInput).map(|n| n.rect()).collect();
    let (find_what, replace_with) = (fields[fields.len() - 2], fields[fields.len() - 1]);
    assert!(find_what.left() > HOST_PANEL && replace_with.top() > find_what.top(), "the dialog's two fields");
    click_at(&mut h, find_what.center());
    type_text(&mut h, "a");
    key(&mut h, Key::Tab, Modifiers::NONE);
    type_text(&mut h, "b");
    key(&mut h, Key::Tab, Modifiers::SHIFT);
    assert!(h.state().app.has_keyboard(), "Shift+Tab inside the dialog");
    type_text(&mut h, "c");
    let d = h.state().app.dialog.as_ref().unwrap();
    assert_eq!((&d.values["what"], &d.values["with"]), (&json!("ac"), &json!("b")));
    assert_eq!(h.state().note, "", "the host's field got nothing");
}

#[test]
fn host_font_changes_never_crash_the_sheet() {
    // The host installs its own font set after the sheet drew, in the frame the sheet installed
    // its families, and again for a few frames.
    let mut h = harness(embedded(Services::default()));
    let frame = h.state().frame;
    h.state_mut().host_fonts_on = vec![frame + 1, frame + 3, frame + 4, frame + 5];
    h.run_steps(10);
    assert!(theme::fonts_installed(&h.ctx), "the sheet's families came back");
    let mut fresh = egui_kittest::Harness::builder().with_size(vec2(1280.0, 800.0)).build_ui_state(
        |ui, host: &mut Host| {
            host.frame += 1;
            host.app.logic(ui.ctx());
            egui::CentralPanel::default().show(ui, |ui| host.app.ui(ui));
            if host.frame == 1 {
                // The host's own start-up, after the sheet's first frame.
                ui.ctx().set_fonts(host_fonts());
            }
        },
        Host {
            app: embedded(Services::default()),
            note: String::new(),
            note_rect: Rect::NOTHING,
            host_saves: 0,
            host_quits: 0,
            frame: 0,
            host_fonts_on: Vec::new(),
        },
    );
    fresh.run_steps(8);
    assert!(theme::fonts_installed(&fresh.ctx));
    assert!(fresh.state().app.grid.rect.is_some(), "the sheet draws");

    // A `fonts` service that drops a family the sheet draws with gets it put back.
    let services = Services {
        fonts: Some(Box::new(|f: &mut egui::FontDefinitions| {
            f.families.remove(&egui::FontFamily::Name(theme::MONO.into()));
            f.families.remove(&egui::FontFamily::Name(theme::UI.into()));
        })),
        ..Default::default()
    };
    for app in [
        embedded(services),
        SheetApp::new(session(), Services { fonts: Some(Box::new(|f: &mut egui::FontDefinitions| f.families.clear())), ..Default::default() }),
    ] {
        let mut h = harness(app);
        h.state_mut().app.run("cell.set", json!({"cell": "A1", "input": "=1+1"})).unwrap();
        h.state_mut().app.run("formulas.showFormulas", json!({})).ok();
        h.run_steps(4);
        assert!(theme::fonts_installed(&h.ctx));
    }
}

#[test]
fn a_running_host_command_can_unregister_itself() {
    let mut app = embedded(Services::default());
    app.register_command(HostCommand::new("contoso.once", "Once", |app, _| {
        assert!(app.has_host_command("contoso.once"));
        assert!(app.register_command(HostCommand::new("contoso.once", "Twice", |_, _| Ok(Json::Null))).is_err(), "still registered");
        assert_eq!(app.run("contoso.once", json!({})), Err("command `contoso.once` is already running".into()));
        assert!(app.unregister_command("contoso.once"));
        assert!(!app.has_host_command("contoso.once"));
        Ok(json!(1))
    }))
    .unwrap();
    assert_eq!(app.run("contoso.once", json!({})), Ok(json!(1)));
    assert!(!app.has_host_command("contoso.once"), "it stays unregistered");
    assert!(app.run("contoso.once", json!({})).is_err());
}

#[test]
fn standalone_keyboard_is_unchanged() {
    let app = SheetApp::new(session(), Services::default());
    assert_eq!(app.embed, EmbedOptions::default());
    assert_eq!(app.embed.keyboard, KeyboardScope::Window);
    let mut h = harness(app);
    assert!(h.state().app.has_keyboard());
    type_text(&mut h, "5");
    assert!(h.state().app.editor.is_some(), "the standalone sheet takes every key");
}

#[test]
fn host_theme_maps_onto_the_tokens() {
    let (light, dark) = palette();
    let t = light.tokens(false);
    assert_eq!(
        (t.accent, t.sel_border, t.window, t.ribbon, t.text, t.text_dim),
        (light.accent, light.selection, light.surface, light.raised, light.text, light.text_muted)
    );
    assert_eq!((t.tab_bar, t.status_bar, t.menu_bg, t.input_border), (light.surface, light.surface, light.raised, light.border));
    assert!(!t.dark && dark.tokens(true).dark);
    // The cells keep the workbook's colours.
    assert_eq!((t.grid_bg, t.gridline, t.cell_text), (Tokens::light().grid_bg, Tokens::light().gridline, Tokens::light().cell_text));
    assert_eq!(t.sel_fill.a(), Tokens::light().sel_fill.a());

    let host_theme = HostTheme::from_palettes(&light, &dark);
    let mut app = embedded(Services::default());
    app.set_host_theme(Some(host_theme));
    let mut h = harness(app);
    assert_eq!(Tokens::get(&h.ctx), host_theme.light);
    // Light or dark is the host's choice.
    h.ctx.set_theme(egui::Theme::Dark);
    h.run_steps(3);
    assert_eq!(Tokens::get(&h.ctx), host_theme.dark);
    assert!(Tokens::get(&h.ctx).dark);
    h.ctx.set_theme(egui::Theme::Light);
    h.run_steps(3);
    // The host's own global visuals are not touched by the sheet's colours.
    assert_eq!(h.ctx.global_style().visuals.panel_fill, egui::Visuals::light().panel_fill);
    let themed = render(&mut h, "host-theme");
    h.state_mut().app.set_host_theme(None);
    h.run_steps(3);
    assert_eq!(Tokens::get(&h.ctx), Tokens::light());
    let plain = render(&mut h, "embedded-default-theme");
    if let (Some(themed), Some(plain)) = (themed, plain) {
        let sheet = Rect::from_min_max(pos2(HOST_PANEL, 0.0), pos2(1280.0, 800.0));
        let green = Tokens::light().accent;
        assert!(count(&themed, sheet, light.accent) > 20, "the host accent marks the active tab and the selection");
        assert_eq!(count(&themed, sheet, green), 0, "no GridCraft green is left");
        assert!(count(&plain, sheet, green) > 20);
        assert_eq!(count(&plain, sheet, light.accent), 0);
        assert!(count(&themed, sheet, light.surface) > 10_000, "the host surface fills the chrome");
    }

    // A sheet that owns the global style builds the context's visuals from the host theme.
    let mut app = SheetApp::new(session(), Services::default());
    app.set_host_theme(Some(host_theme));
    let h = harness(app);
    assert_eq!(h.ctx.global_style().visuals.panel_fill, light.surface);
    assert_eq!(h.ctx.global_style().visuals.selection.stroke.color, light.accent);
}

/// A host tab with a host command (stamps A1), a built-in command and a command that is never
/// available; the tab's keytip is Y.
fn host_tab_app(runs: Rc<Cell<u32>>) -> SheetApp {
    let mut app = embedded(Services::default());
    app.register_command(
        HostCommand::new("contoso.stamp", "Stamp Cells", move |app, params| {
            runs.set(runs.get() + 1);
            let text = params.get("text").and_then(Json::as_str).unwrap_or("Stamped").to_string();
            app.run("cell.set", json!({"cell": "A1", "input": text}))?;
            Ok(json!({"stamped": true}))
        })
        .params("{text?: string}"),
    )
    .unwrap();
    app.register_command(HostCommand::new("contoso.never", "Never", |_, _| Ok(Json::Null)).enabled_when(|_| false)).unwrap();
    assert!(app.register_command(HostCommand::new("contoso.stamp", "Again", |_, _| Ok(Json::Null))).is_err(), "ids are unique");
    assert!(app.register_command(HostCommand::new("home.bold", "Bold", |_, _| Ok(Json::Null))).is_err(), "built-in ids are GridCraft's");
    app.add_ribbon_tab(
        RibbonTab::new("Contoso")
            .keytip("Y")
            .group(
                RibbonGroup::new("Cells")
                    .button(RibbonButton::new("Stamp", "contoso.stamp").icon(Icon::Check).keytip("S"))
                    .button(RibbonButton::new("Never", "contoso.never").small())
                    .button(RibbonButton::new("Bold A1", "home.bold").params(json!({"range": "A1"})).small().keytip("B")),
            )
            .draw(|app, ui| {
                ui.label(format!("{} sheet(s)", app.session.active().map(|d| d.wb.sheets.len()).unwrap_or(0)));
            }),
    )
    .unwrap();
    app
}

#[test]
fn host_tab_buttons_run_commands() {
    let runs = Rc::new(Cell::new(0));
    let mut h = harness(host_tab_app(runs.clone()));
    assert!(!has_label(&h, "Stamp"), "Home is showing");
    h.get_by_label("Contoso").click();
    h.run_steps(3);
    assert_eq!(h.state().app.ui.ribbon_tab, "Contoso");
    assert!(has_label(&h, "1 sheet(s)"), "the host's draw callback ran");
    let tab = render(&mut h, "host-tab");
    h.get_by_label("Stamp").click();
    h.run_steps(3);
    assert_eq!(runs.get(), 1);
    assert_eq!(value(&h.state().app, "A1"), Value::Text("Stamped".into()));
    // A command that isn't available leaves its button inert.
    h.get_by_label("Never").click();
    h.run_steps(3);
    assert!(h.state().app.message.is_none() && h.state().app.toast.is_none());

    // Collapsed, the tab's buttons hide; clicking the tab brings them back.
    h.state_mut().app.run("view.collapseRibbon", json!({})).unwrap();
    h.run_steps(3);
    assert!(!has_label(&h, "Stamp"));
    h.get_by_label("Contoso").click();
    h.run_steps(3);
    assert!(has_label(&h, "Stamp") && !h.state().app.ui.ribbon_collapsed);

    // Keytips: F10, Y (the tab), S (the button) — also from another tab and a collapsed ribbon.
    let grid = grid_point(&h);
    click_at(&mut h, grid);
    h.state_mut().app.run("ui.ribbonTab", json!({"tab": "Home"})).unwrap();
    h.state_mut().app.run("view.collapseRibbon", json!({})).unwrap();
    h.run_steps(2);
    key(&mut h, Key::F10, Modifiers::NONE);
    render(&mut h, "host-tab-keytip");
    letter(&mut h, Key::Y);
    assert_eq!(h.state().app.ui.ribbon_tab, "Contoso");
    assert!(!h.state().app.ui.ribbon_collapsed);
    assert_eq!(h.state().app.keytips.prefix(), Some("Y"));
    render(&mut h, "host-button-keytips");
    letter(&mut h, Key::S);
    assert_eq!(runs.get(), 2);
    assert!(h.state().app.keytips.prefix().is_none());
    key(&mut h, Key::F10, Modifiers::NONE);
    letter(&mut h, Key::Y);
    letter(&mut h, Key::B);
    let bold = {
        let d = h.state().app.session.active().unwrap();
        let sh = d.wb.active().unwrap();
        d.wb.styles.get(sh.style_id(CellRef::parse("A1").unwrap())).font.bold
    };
    assert!(bold, "a host button can run a built-in command with params");

    // Everything is a command: the control channel lists and runs the host's commands.
    let ctx = h.ctx.clone();
    let control::Outcome::Done(list) = control::handle(&mut h.state_mut().app, &ctx, "engine.commands", &json!({"search": "contoso"})) else {
        panic!("engine.commands answers")
    };
    let ids: Vec<&str> = list["result"].as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    assert_eq!(ids, ["contoso.stamp", "contoso.never"]);
    assert_eq!(list["result"][0]["menu"], json!(["Contoso", "Cells"]));
    assert_eq!(list["result"][1]["enabled"], false);
    let control::Outcome::Done(r) =
        control::handle(&mut h.state_mut().app, &ctx, "engine.execute", &json!({"command": "contoso.stamp", "params": {"text": "Agent"}}))
    else {
        panic!("engine.execute answers")
    };
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(value(&h.state().app, "A1"), Value::Text("Agent".into()));
    let control::Outcome::Done(r) = control::handle(&mut h.state_mut().app, &ctx, "contoso.never", &json!({})) else { panic!() };
    assert_eq!(r["ok"], false, "an unavailable host command reports it");
    assert_eq!(runs.get(), 3);

    // The command search finds host commands under their ribbon tab, and runs them.
    h.state_mut().app.open_dialog("commandSearch", json!({}));
    h.state_mut().app.dialog.as_mut().unwrap().search = "stamp".into();
    h.run_steps(3);
    h.get_by_label_contains("Stamp Cells").click();
    h.run_steps(3);
    assert_eq!(runs.get(), 4);
    assert!(h.state().app.dialog.is_none());

    if let Some(img) = tab {
        // The active host tab's underline is the accent, as for built-in tabs.
        let tabs_row = Rect::from_min_size(pos2(HOST_PANEL, 0.0), vec2(1280.0 - HOST_PANEL, 60.0));
        assert!(count(&img, tabs_row, Tokens::light().accent) > 10);
    }
}

#[test]
fn host_stores_the_workbook_as_bytes() {
    let mut source = session();
    source.execute("cell.set", json!({"cell": "A1", "input": "41"})).unwrap();
    source.execute("cell.set", json!({"cell": "A2", "input": "=A1+1"})).unwrap();
    let b64 = source.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap()["base64"].as_str().unwrap().to_string();
    let bytes = gridcraft_engine::io::base64_decode(&b64).unwrap();

    let saved: Rc<RefCell<Vec<(String, Vec<u8>, bool)>>> = Rc::default();
    let refuse = Rc::new(Cell::new(false));
    let (sink, refusing) = (saved.clone(), refuse.clone());
    let mut app = embedded(Services {
        save_workbook: Some(Box::new(move |s| {
            if refusing.get() {
                return Err("the store is offline".into());
            }
            sink.borrow_mut().push((s.name.to_string(), s.bytes.to_vec(), s.save_as));
            Ok(())
        })),
        ..Default::default()
    });
    app.open_workbook_bytes("Plan.xlsx", &bytes, None).unwrap();
    assert_eq!(app.session.documents().len(), 1, "the blank Book1 was replaced");
    assert_eq!(app.session.active().unwrap().display_title(), "Plan.xlsx");
    assert_eq!(value(&app, "A2"), Value::Number(42.0));
    assert!(!app.session.active().unwrap().is_dirty());

    app.run("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    let (name, out) = app.workbook_bytes("xlsx").unwrap();
    assert_eq!(name, "Plan.xlsx");
    assert!(app.session.active().unwrap().is_dirty(), "the host marks it saved once stored");
    let mut check = Session::new();
    check.execute("file.open", json!({"name": name, "base64": gridcraft_engine::io::base64_encode(&out)})).unwrap();
    assert_eq!(check.active().unwrap().wb.active().unwrap().value(CellRef::parse("A2").unwrap()), Value::Number(2.0));
    let (csv_name, csv) = app.workbook_bytes("csv").unwrap();
    assert_eq!(csv_name, "Plan.csv");
    assert!(String::from_utf8_lossy(&csv).starts_with('1'));
    app.mark_saved();
    assert!(!app.session.active().unwrap().is_dirty());

    // Save and Save As hand the bytes to the host; a refusal keeps the workbook dirty.
    app.run("cell.set", json!({"cell": "B1", "input": "x"})).unwrap();
    app.run("file.save", json!({})).unwrap();
    app.run("file.saveAs", json!({})).unwrap();
    assert_eq!(saved.borrow().iter().map(|(n, _, a)| (n.as_str(), *a)).collect::<Vec<_>>(), [("Plan.xlsx", false), ("Plan.xlsx", true)]);
    assert!(!app.session.active().unwrap().is_dirty());
    refuse.set(true);
    app.run("cell.set", json!({"cell": "B2", "input": "y"})).unwrap();
    assert_eq!(app.run("file.save", json!({})), Err("the store is offline".into()));
    assert!(app.session.active().unwrap().is_dirty());
    assert!(app.dialog.is_none(), "no file dialog");

    // Hostile bytes never crash: a broken workbook is an error and the open one stays.
    assert!(app.open_workbook_bytes("broken.xlsx", b"PK\x03\x04garbage", None).is_err());
    assert_eq!(app.session.active().unwrap().display_title(), "Plan.xlsx");
    for junk in [&b""[..], &[0xFFu8; 64][..], &[0u8; 3][..]] {
        let _ = app.open_workbook_bytes("junk.xlsx", junk, None);
    }
}
