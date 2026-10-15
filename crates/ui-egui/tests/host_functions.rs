//! Functions of the embedding application in the formula editor: AutoComplete offers them, the
//! argument hint shows their signature, description and the description of the argument being
//! typed (all from the host's catalog), and Insert Function lists them.

use std::sync::Arc;

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use gridcraft_engine::core::{CellError, CellRef, Value};
use gridcraft_engine::{HostArg, HostFunctionInfo, HostFunctions, HostResult, Session};
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::i18n::Language;
use gridcraft_ui_egui::theme::{ui_bold, ui_font};
use serde_json::json;

struct Contoso;

impl HostFunctions for Contoso {
    fn namespace(&self) -> &str {
        "CONTOSO"
    }
    fn catalog(&self) -> Vec<HostFunctionInfo> {
        vec![
            HostFunctionInfo::new("LOOKUP", "Looks a key up in the host's data.")
                .arg("key", "The key to look up.")
                .optional_arg("column", "Which column to return (default 1)."),
            HostFunctionInfo::new("RATE", "The current rate of a key.").arg("key", ""),
        ]
    }
    fn call(&self, name: &str, args: &[HostArg]) -> HostResult {
        let key = args.first().and_then(HostArg::value).map(Value::display).unwrap_or_default();
        let column = args.get(1).and_then(HostArg::value).and_then(|v| v.to_number().ok()).unwrap_or(1.0);
        match name {
            "LOOKUP" => format!("{key}#{column}").as_str().into(),
            "RATE" => (key.len() as f64).into(),
            _ => CellError::Name.into(),
        }
    }
}

fn harness(language: Language) -> Harness<'static, SheetApp> {
    let mut session = Session::new();
    session.register_host_functions(Arc::new(Contoso)).unwrap();
    session.new_workbook();
    let mut app = SheetApp::new(session, Default::default());
    app.ui.language = language;
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn type_text(h: &mut Harness<'static, SheetApp>, text: &str) {
    h.input_mut().events.push(Event::Text(text.into()));
    h.run_steps(4);
}

fn key(h: &mut Harness<'static, SheetApp>, key: Key) {
    h.input_mut().events.extend([
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE },
        Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE },
    ]);
    h.run_steps(4);
}

/// The text painted this frame, one entry per text shape, with its font.
fn painted(h: &Harness<'static, SheetApp>) -> Vec<(String, egui::FontId)> {
    h.output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) => {
                let font = t.galley.job.sections.first().map(|s| s.format.font_id.clone()).unwrap_or_default();
                Some((t.galley.text().to_string(), font))
            }
            _ => None,
        })
        .collect()
}

fn font_of(h: &Harness<'static, SheetApp>, text: &str) -> egui::FontId {
    painted(h).into_iter().find(|(t, _)| t == text).map(|(_, f)| f).unwrap_or_else(|| panic!("{text:?} painted"))
}

fn shows(h: &Harness<'static, SheetApp>, text: &str) -> bool {
    painted(h).iter().any(|(t, _)| t.contains(text))
}

fn editor_text(h: &Harness<'static, SheetApp>) -> String {
    h.state().editor.as_ref().map(|e| e.text.clone()).unwrap_or_default()
}

/// With `GRIDCRAFT_HOST_FUNCTION_SHOTS=<dir>`, saves a render there to look at (needs a GPU).
fn shot(h: &mut Harness<'static, SheetApp>, name: &str) {
    if let Ok(dir) = std::env::var("GRIDCRAFT_HOST_FUNCTION_SHOTS") {
        let _ = std::fs::create_dir_all(&dir);
        h.render().unwrap().save(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
    }
}

fn value(app: &SheetApp, cell: &str) -> Value {
    app.session.active().unwrap().wb.active().unwrap().value(CellRef::parse(cell).unwrap())
}

#[test]
fn autocomplete_and_the_argument_hint_know_host_functions() {
    let mut h = harness(Language::En);
    h.state_mut().begin_edit(Some("=CONTOS".into()), false);
    h.run_steps(4);
    type_text(&mut h, "O");
    assert_eq!(h.state().editor.as_ref().unwrap().autocomplete, ["CONTOSO.LOOKUP", "CONTOSO.RATE"]);
    // The list shows the highlighted function's description from the catalog.
    assert!(shows(&h, "CONTOSO.LOOKUP") && shows(&h, "Looks a key up in the host's data."));
    shot(&mut h, "host-autocomplete");
    // Host functions sort among the built-in ones.
    h.state_mut().cancel_edit();
    h.state_mut().begin_edit(Some("=CO".into()), false);
    h.run_steps(4);
    type_text(&mut h, "N");
    let offered = h.state().editor.as_ref().unwrap().autocomplete.clone();
    assert!(offered.iter().any(|n| n == "CONCAT") && offered.iter().any(|n| n == "CONTOSO.LOOKUP"), "{offered:?}");
    assert!(offered.windows(2).all(|w| w[0] <= w[1]), "sorted: {offered:?}");
    h.state_mut().cancel_edit();

    // Tab takes the highlighted name, then the hint follows the arguments.
    h.state_mut().begin_edit(Some("=CONTOSO".into()), false);
    h.run_steps(4);
    type_text(&mut h, ".L");
    assert_eq!(h.state().editor.as_ref().unwrap().autocomplete, ["CONTOSO.LOOKUP"]);
    key(&mut h, Key::Tab);
    assert_eq!(editor_text(&h), "=CONTOSO.LOOKUP(");
    assert_eq!(h.state().editor.as_ref().unwrap().current_function(), Some(("CONTOSO.LOOKUP".into(), 0)));
    assert!(shows(&h, "CONTOSO.LOOKUP("));
    assert!(shows(&h, "Looks a key up in the host's data."));
    assert!(shows(&h, "key: The key to look up."));
    assert_eq!(font_of(&h, "key"), ui_bold(12.0), "the argument being typed stands out");
    assert_eq!(font_of(&h, "[column]"), ui_font(12.0));
    shot(&mut h, "host-argument-hint");
    type_text(&mut h, "\"EUR\",");
    assert!(shows(&h, "column: Which column to return (default 1)."));
    assert!(!shows(&h, "key: The key to look up."));
    assert_eq!(font_of(&h, "[column]"), ui_bold(12.0), "now the second argument");
    assert_eq!(font_of(&h, "key"), ui_font(12.0));
    type_text(&mut h, "2)");
    key(&mut h, Key::Enter);
    assert!(h.state().editor.is_none());
    assert_eq!(value(h.state(), "A1"), Value::Text("EUR#2".into()));

    // An argument the catalog leaves undescribed adds nothing; past the last argument neither.
    h.state_mut().begin_edit(Some("=CONTOSO.RATE(".into()), false);
    h.run_steps(4);
    assert!(shows(&h, "The current rate of a key."));
    assert!(!painted(&h).iter().any(|(t, _)| t.starts_with("key:")));
    h.state_mut().cancel_edit();

    // Without the host, its functions are no longer offered.
    assert!(h.state_mut().session.unregister_host_functions("CONTOSO"));
    h.state_mut().begin_edit(Some("=CONTOS".into()), false);
    h.run_steps(4);
    type_text(&mut h, "O");
    assert!(h.state().editor.as_ref().unwrap().autocomplete.is_empty());
    type_text(&mut h, ".LOOKUP(");
    assert!(!shows(&h, "key: The key to look up."));
}

#[test]
fn the_hint_uses_the_locale_list_separator_for_host_functions() {
    let mut h = harness(Language::En);
    h.state_mut().run("app.language.set", json!({"language": "es"})).unwrap();
    h.run_steps(2);
    h.state_mut().begin_edit(Some("=CONTOSO.LOOKUP(\"EUR\";".into()), false);
    h.run_steps(4);
    assert_eq!(h.state().editor.as_ref().unwrap().current_function(), Some(("CONTOSO.LOOKUP".into(), 1)));
    assert!(shows(&h, "; "), "Spanish separates arguments with semicolons");
    assert!(shows(&h, "column: Which column to return (default 1)."));
    h.input_mut().events.push(Event::Text("2)".into()));
    h.run_steps(4);
    key(&mut h, Key::Enter);
    assert_eq!(value(h.state(), "A1"), Value::Text("EUR#2".into()));
}

#[test]
fn insert_function_lists_host_functions() {
    let mut h = harness(Language::En);
    h.state_mut().open_dialog("insertFunction", json!({}));
    h.state_mut().dialog.as_mut().unwrap().search = "contoso".into();
    h.run_steps(4);
    assert!(shows(&h, "UserDefined"));
    assert!(shows(&h, "CONTOSO.LOOKUP(key, [column])"), "the signature of the first match");
    assert!(shows(&h, "Looks a key up in the host's data."));
    key(&mut h, Key::Enter);
    assert!(h.state().dialog.is_none());
    assert_eq!(editor_text(&h), "=CONTOSO.LOOKUP(");
    assert!(shows(&h, "key: The key to look up."));
}
