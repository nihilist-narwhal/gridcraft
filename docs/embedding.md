# Embedding the sheet in another application

> **Last reviewed:** 2026-10-14 · **Last updated:** 2026-10-14 · **Change:** minor (keyboard and font fixes from review: focus moves inside the sheet, kept keys keep their text, quit shortcuts, host font sets)

`gridcraft-ui-egui`'s `SheetApp::ui(&mut egui::Ui)` draws the whole spreadsheet into any egui
region. An egui application (the *host*) can show it as one panel of its own window, in its own
design system, with its own ribbon tab and its own storage. The API lives in the UI crate
(`embed`, `host_ribbon`, `theme`); nothing below L6 changes. Defaults are the standalone app,
pixel for pixel.

## A host in a few lines

```rust
use gridcraft_ui_egui::{SheetApp, Services, embed::HostCommand, host_ribbon::*, theme::*};

let mut sheet = SheetApp::embedded(session, Services {
    // Save / Save As hand the .xlsx to the host instead of asking for a path.
    save_workbook: Some(Box::new(|s| store.put(s.name, s.bytes))),
    ..Default::default()
});
sheet.set_host_theme(Some(HostTheme::from_palettes(&light_palette, &dark_palette)));
sheet.register_command(HostCommand::new("contoso.refresh", "Refresh Data", |app, _params| {
    app.run("range.setValues", serde_json::json!({"range": "A1:B2", "values": [[1, 2], [3, 4]]}))
}))?;
sheet.add_ribbon_tab(
    RibbonTab::new("Contoso").keytip("Y").group(
        RibbonGroup::new("Data").button(RibbonButton::new("Refresh\nData", "contoso.refresh").keytip("R")),
    ),
)?;
sheet.open_workbook_bytes("Plan.xlsx", &bytes, None)?;

// Every frame, inside the host's own layout:
egui::Panel::left("navigation").show(ui, |ui| { /* the host's panel */ });
egui::CentralPanel::default().show(ui, |ui| {
    sheet.logic(ui.ctx());
    sheet.ui(ui);
});
```

`cargo run -p gridcraft-ui-egui --example snapshot -- --sample sales --embedded --host-theme
--host-tab out.png` renders a sample host (a navigation panel beside the sheet, a sample palette
and a sample `Contoso` tab) offscreen.

## Window chrome: `EmbedOptions`

`SheetApp::embed` (a plain struct, set it any time) decides what the sheet draws and owns.
`EmbedOptions::default()` is the standalone app; `EmbedOptions::embedded()` (used by
`SheetApp::embedded`) turns off everything that belongs to the window:

| Field | Standalone | Embedded | Off means |
|---|---|---|---|
| `title_bar` | on | off | no title bar (title, Search, Share, Comments, light/dark switch) |
| `quick_access_toolbar` | on | off | no AutoSave/Home/Save/Undo/Redo/More (they live in the title bar) |
| `backstage` | on | off | no File tab; `ui.backstage` returns an error and File › Options shows a notice that there is no File view; Print prints directly |
| `status_bar` | on | off | no status bar (the user's View setting applies when on) |
| `window_controls` | on | off | no window title, no drag/maximize from the title bar; `ui.resize`, `ui.focus` and `app.quit` are refused on the control channel |
| `global_style` | on | off | see below |
| `keyboard` | `Window` | `Focus` | see [Keyboard](#keyboard) |
| `reserved_shortcuts` | none | none | shortcuts the host keeps |

The ribbon (with collapse and keytips), the formula bar, the grid, the sheet tabs, task panes,
dialogs and message boxes stay in every mode.

**Global style.** With `global_style` on, the sheet owns the egui context's style, as the
standalone app does: light/dark (`view.theme`), the interface size (`view.interfaceScale`), the
visuals, spacing and fonts. Off, it leaves all of that to the host:

- light or dark is the host's (`ctx.set_theme`); `view.theme`, `view.darkMode` and
  `view.interfaceScale` return an error and the View tab hides their controls;
- GridCraft's visuals, spacing and text styles apply to the sheet's own region only
  (`theme::sheet_style`); the host's widgets, and popups and dialogs, keep the host's style;
- GridCraft's fonts are *merged* into the host's font set: the host keeps its font data and its
  proportional and monospace families, GridCraft adds its named families (`ui`, `cell`, …). If
  the host later replaces the whole font set (before or after the sheet draws, in any frame),
  the sheet installs its families again before it draws. `Services::fonts` can adjust the set
  (for example put the host's typeface first in the `ui` family); a family the sheet draws with
  that the adjustment drops is put back.

Call `SheetApp::logic` before `SheetApp::ui` every frame; don't call `setup_context*` (that is
the standalone app's start-up).

## Keyboard

With `KeyboardScope::Focus` the sheet handles keys only while it *has the keyboard*:

- a press inside the sheet's region gives it the keyboard; a press elsewhere in the host takes it
  away. A press on one of the sheet's dialogs keeps it with the sheet; menus leave it where it
  was;
- a host widget that takes egui's focus (Tab, or the host's `request_focus`) takes the keyboard.
  Focus that egui moves between the sheet's own widgets (Shift+Tab between the fields of one of
  its dialogs) leaves it with the sheet;
- `SheetApp::focus_keyboard()` / `release_keyboard()` hand it over explicitly, and
  `has_keyboard()` tells who has it. Keys sent over the control channel give it to the sheet.

Without the keyboard the sheet sees no key, text, copy, cut, paste or IME event, never grabs
egui's focus, and its shortcuts and keytips (Alt, F10) stay off. The events are taken out only
while the sheet draws and put back afterwards, so host widgets drawn later still get them.

With the keyboard, the sheet uses up the keyboard events it saw: a host that handles its own
shortcuts *after* `SheetApp::ui` doesn't act on them twice. Two ways to keep keys for the host
even then:

- `EmbedOptions::reserved_shortcuts`: `KeyboardShortcut`s matched exactly (Shift and Alt must
  match; `Modifiers::COMMAND` is Cmd on macOS, Ctrl elsewhere). Reserving Cmd/Ctrl+C, X or V also
  keeps the copy, cut and paste events;
- `Services::keep_key`: a callback asked for every keyboard event while the sheet has the
  keyboard; `true` keeps the event for the host.

A key kept either way keeps the text it types (a reserved `/` doesn't start an edit with `/`), and
a kept Alt+key doesn't leave the sheet taking the Alt release for a keytip tap. egui's own quit
shortcuts (`Options::quit_shortcuts`, Cmd/Ctrl+Q by default) are always the host's. Reserved
events never reach the sheet and are back in the frame's input after `SheetApp::ui`.

## Colours: `HostTheme`

`theme::Palette` names the colours a design system usually has: `accent`, `selection`,
`surface`, `raised`, `border`, `text`, `text_muted`. `Palette::tokens(dark)` maps them onto
GridCraft's `theme::Tokens` and derives the rest (hover and pressed fills, header highlights, the
translucent selection fill). The cells keep the workbook's own colours: white paper, black text,
light gridlines.

`HostTheme { light, dark }` holds complete `Tokens` for both appearances, so a host can start
from `HostTheme::from_palettes(&light, &dark)` and change any single token.
`SheetApp::set_host_theme(Some(theme))` applies it from the next frame on; `None` restores
GridCraft's colours. Which appearance applies follows the egui context: the host's light/dark
when the sheet leaves the global style to it, the sheet's own `view.theme` otherwise (then the
context's visuals are built from the host theme too). Every `Tokens::get(ctx)` sees the host
theme, so a host can colour its own panels from the same tokens.

## Ribbon tabs: `RibbonTab`

`SheetApp::add_ribbon_tab` adds a tab after the built-in ones (and before contextual tabs such
as Table Design). A tab has

- a `label`, shown as given and used as its identity (`ui.ribbonTab {"tab": label}` selects
  it); it can't be a built-in tab's name or another host tab's;
- an optional `keytip` (one to three letters or digits, e.g. `Y`), typed after Alt or F10. It
  may not clash with a built-in keytip (`keytips::keytip_table::conflicts`) or another host tab's;
- `groups` of `RibbonButton`s, laid out like the built-in groups: large buttons (icon over a one-
  or two-line label, `\n` breaks it) stand alone, small ones stack three high. A button runs a
  command (`command`, `params`) — a host command or any built-in one — and is greyed out while
  the command isn't available. Its `keytip` (one or two letters or digits) follows the tab's.
  Icons are GridCraft's own (`ButtonIcon::Builtin`) or painted by the host (`paint_icon`);
- an optional `draw` callback, `FnMut(&mut SheetApp, &mut egui::Ui)`, drawn after the groups:
  immediate-mode content of any kind (the `widgets` helpers draw ribbon-style buttons).

Host tabs collapse and expand with the ribbon, show their keytip badges like the built-in tabs
and work on the web. `remove_ribbon_tab(label)` takes one away.

## Commands: `HostCommand`

Everything is a command, the host's too. `SheetApp::register_command(HostCommand::new(id,
label, run))` adds one; `run` gets `&mut SheetApp` and the params and returns a JSON result or
an error message. Ids are namespaced (`contoso.refresh`): letters, digits, `.`, `_` and `-`, at
least one dot, never a built-in id or the `ui.` / `app.` namespaces. `.params(doc)` documents
the params and `.enabled_when(f)` makes it unavailable (greyed out buttons, an error from
`run`) while `f` says so.

A host command runs through `SheetApp::run` like a built-in one, so ribbon buttons, keytips, the
command search, `engine.execute` and `engine.commands` on the control channel (`"host": true`,
with its ribbon path) all reach it. A command that runs itself again gets an error instead of
recursing; one that unregisters itself while it runs is gone once it finishes.

## Workbooks the host stores

- `open_workbook_bytes(name, bytes, password)` opens any format `file.open` reads and shows it,
  without file dialogs; an untouched blank workbook is replaced. A protected workbook without
  `password` is an error containing `gridcraft_engine::io::PASSWORD_REQUIRED`. A cell still being
  edited is committed when valid and dropped otherwise.
- `workbook_bytes(format)` returns `(name, bytes)` for `xlsx`, `csv`, `tsv`, `json` or `html`,
  committing the cell being edited first.
- `mark_saved()` clears the dirty flag once the host has stored the bytes.
- `Services::save_workbook`: with it, Save on a workbook that has no file, and Save As, hand the
  host a `HostSave { name, bytes, save_as }` (always .xlsx) instead of opening a file picker;
  `Ok` marks the workbook saved, an error is shown and leaves it dirty. A workbook opened from a
  path still saves to that path.
- Open (Cmd/Ctrl+O, `file.open` without params) calls `Services::open_async` (or `pick_open`)
  when the host sets it: the host can show its own document browser there and deliver the chosen
  workbook as `(name, bytes)` through `Services::inbox`, which opens it on the next frame (asking
  for a password when needed). Without either service, Open does nothing.

## Agents and the control channel

Everything the sheet can do is a command, so a host's own agent integration can call
`SheetApp::run(id, params)` directly. To serve the control channel ([`control-protocol.md`](control-protocol.md))
the host sets `SheetApp::control_rx` to a receiver of `ControlRequest`s from a transport of its
own (the standalone app's localhost server lives in `apps/gridcraft`); `SheetApp::logic` answers
them. Synthetic input (`ui.click`, `ui.text`, `ui.key`, …) reaches the sheet only when the host's
`eframe::App::raw_input_hook` calls `SheetApp::raw_input_hook`; keys sent that way give the sheet
the keyboard.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-14 | major | Created: embed options, keyboard scope and reserved shortcuts, host theme, host ribbon tabs and commands, workbook bytes in and out |
| 2026-10-14 | minor | Review fixes: Shift+Tab inside the sheet's dialogs keeps the keyboard, kept keys keep their text and don't arm keytips, quit shortcuts stay the host's, host font sets and `Services::fonts` can't remove the sheet's families, a running command can unregister itself; agents and the control channel |
