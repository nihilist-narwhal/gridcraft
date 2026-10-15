//! Renders the whole GridCraft window offscreen (wgpu, no window) to a PNG.
//!
//! `cargo run --release -p gridcraft-ui-egui --example snapshot -- [--sample sales] [--in file.xlsx]
//!  [--size 1440x900] [--scale 2] [--cmd 'id={json}']... [--tab Insert] [--dark] [--language en|zh|ja|ko|ru|pt|cs]
//!  [--dialog 'name={json}'] [--hf-edit header|footer] [--embedded] [--host-theme] [--host-tab] out.png`
//!
//! `--embedded` draws the sheet as one panel of a sample host window (a navigation panel on the
//! left; no title bar, File view or status bar), `--host-theme` applies a sample host palette, and
//! `--host-tab` adds and selects a sample host ribbon tab (see `docs/embedding.md`).

use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::embed::{EmbedOptions, HostCommand};
use gridcraft_ui_egui::host_ribbon::{RibbonButton, RibbonGroup, RibbonTab};
use gridcraft_ui_egui::icons::Icon;
use gridcraft_ui_egui::theme::{HostTheme, Palette};
use serde_json::json;

/// A sample host design system: cool greys with an indigo accent.
fn host_theme() -> HostTheme {
    let c = egui::Color32::from_rgb;
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
    HostTheme::from_palettes(&light, &dark)
}

/// A sample host tab: two host commands and two built-in ones, plus host-drawn content.
fn add_host_tab(app: &mut SheetApp) {
    let commands = [
        HostCommand::new("contoso.refresh", "Refresh Data", |app, _| {
            app.run("cell.set", json!({"cell": "A1", "input": "Refreshed"}))?;
            Ok(json!({"refreshed": true}))
        }),
        HostCommand::new("contoso.publish", "Publish", |_, _| Ok(json!({"published": true}))),
    ];
    for command in commands {
        if let Err(e) = app.register_command(command) {
            eprintln!("{e}");
        }
    }
    let tab = RibbonTab::new("Contoso")
        .keytip("Y")
        .group(
            RibbonGroup::new("Data")
                .button(RibbonButton::new("Refresh\nData", "contoso.refresh").icon(Icon::Calc).keytip("R"))
                .button(RibbonButton::new("Publish", "contoso.publish").icon(Icon::Share).keytip("P")),
        )
        .group(
            RibbonGroup::new("Cells")
                .button(RibbonButton::new("Bold", "home.bold").icon(Icon::Bold).small().keytip("B"))
                .button(RibbonButton::new("Clear", "edit.clearContents").icon(Icon::Eraser).small())
                .button(RibbonButton::new("Sort A to Z", "data.sortAscending").icon(Icon::SortAsc).small()),
        )
        .draw(|_, ui| {
            ui.vertical(|ui| {
                ui.label("Connected to");
                ui.strong("contoso-demo");
            });
        });
    if let Err(e) = app.add_ribbon_tab(tab) {
        eprintln!("{e}");
    }
    app.ui.ribbon_tab = "Contoso".into();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut sample = None;
    let mut input = None;
    let mut size = (1440.0f32, 900.0f32);
    let mut scale = 2.0f32;
    let mut cmds: Vec<String> = vec![];
    let mut tab = None;
    let mut dark = false;
    let mut pane: Option<String> = None;
    let mut chart: Option<u32> = None;
    let mut dialog: Option<String> = None;
    let mut language: Option<String> = None;
    let mut hf_edit: Option<String> = None;
    let (mut embedded, mut themed, mut host_tab) = (false, false, false);
    let mut out = "snapshot.png".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--sample" => {
                i += 1;
                sample = args.get(i).cloned();
            }
            "--in" => {
                i += 1;
                input = args.get(i).cloned();
            }
            "--size" => {
                i += 1;
                if let Some((w, h)) = args.get(i).and_then(|s| s.split_once('x')) {
                    size = (w.parse().unwrap_or(1440.0), h.parse().unwrap_or(900.0));
                }
            }
            "--scale" => {
                i += 1;
                scale = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(2.0);
            }
            "--cmd" => {
                i += 1;
                if let Some(c) = args.get(i) {
                    cmds.push(c.clone());
                }
            }
            "--tab" => {
                i += 1;
                tab = args.get(i).cloned();
            }
            "--dark" => dark = true,
            "--embedded" => embedded = true,
            "--host-theme" => themed = true,
            "--host-tab" => host_tab = true,
            "--pane" => {
                i += 1;
                pane = args.get(i).cloned();
            }
            "--chart" => {
                i += 1;
                chart = args.get(i).and_then(|s| s.parse().ok());
            }
            "--dialog" => {
                i += 1;
                dialog = args.get(i).cloned();
            }
            "--hf-edit" => {
                i += 1;
                hf_edit = args.get(i).cloned();
            }
            "--language" => {
                i += 1;
                language = args.get(i).cloned();
            }
            other => out = other.to_string(),
        }
        i += 1;
    }
    let mut session = Session::new();
    if let Some(p) = &input {
        session.execute("file.open", json!({"path": p})).expect("open");
    } else if let Some(s) = &sample {
        session.execute("file.new", json!({"sample": s})).expect("sample");
    } else {
        session.new_workbook();
    }
    for c in &cmds {
        let (id, p) = c.split_once('=').map(|(a, b)| (a.to_string(), serde_json::from_str(b).unwrap_or(json!({})))).unwrap_or((c.clone(), json!({})));
        if let Err(e) = session.execute(&id, p) {
            eprintln!("{id}: {e}");
        }
    }
    let mut harness =
        egui_kittest::Harness::builder().with_size(egui::vec2(size.0, size.1)).with_pixels_per_point(scale).with_max_steps(12).wgpu().build_ui_state(
            move |ui, app: &mut SheetApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                if embedded {
                    // The host's own window: its light/dark, a navigation panel, the sheet beside it.
                    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
                    let t = gridcraft_ui_egui::theme::Tokens::get(&ctx);
                    egui::Panel::left("host_navigation").exact_size(190.0).frame(egui::Frame::NONE.fill(t.window).inner_margin(14)).show(ui, |ui| {
                        ui.label(egui::RichText::new("CONTOSO").strong().size(16.0).color(t.accent));
                        ui.add_space(12.0);
                        for item in ["Overview", "Workbooks", "Reports", "Settings"] {
                            ui.label(egui::RichText::new(item).color(if item == "Workbooks" { t.text } else { t.text_dim }));
                            ui.add_space(6.0);
                        }
                    });
                    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| app.ui(ui));
                } else {
                    app.ui(ui);
                }
            },
            {
                let mut app = SheetApp::new(session, Default::default());
                app.ui.dark = dark;
                if embedded {
                    app.embed = EmbedOptions::embedded();
                }
                if themed {
                    app.set_host_theme(Some(host_theme()));
                }
                if host_tab {
                    add_host_tab(&mut app);
                }
                if let Some(code) = &language
                    && let Some(l) = gridcraft_ui_egui::i18n::Language::parse(code)
                {
                    app.ui.language = l;
                }
                if let Some(t) = tab {
                    app.ui.ribbon_tab = t;
                }
                app.grid.pane = pane;
                app.selected_chart = chart;
                if let Some(d) = &dialog {
                    let (name, p) =
                        d.split_once('=').map(|(a, b)| (a, serde_json::from_str(b).unwrap_or(json!({})))).unwrap_or((d.as_str(), json!({})));
                    app.open_dialog(name, p);
                }
                // Page Layout view: edit the first page's centre header (or footer) section.
                let active = app.session.active().and_then(|d| d.wb.active().cloned().map(|s| (d.wb.active_sheet, s)));
                if let (Some(part), Some((si, sh))) = (&hf_edit, active) {
                    let zone = gridcraft_engine::paper::HfZone { footer: part == "footer", section: 1 };
                    gridcraft_ui_egui::paper_view::begin(&mut app, si, &sh, (0, 0), zone);
                }
                app
            },
        );
    harness.run_steps(8);
    let img = harness.render().expect("render");
    img.save(&out).expect("save png");
    eprintln!("wrote {out} ({}x{})", img.width(), img.height());
}
