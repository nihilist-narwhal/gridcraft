# Architecture

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-14 · **Change:** minor (host functions: worksheet functions an embedding application provides under its own namespace) · **Target:** Microsoft Excel (Microsoft 365)

How GridCraft is built today. Rules for contributors are in [`AGENTS.md`](../AGENTS.md).

## Workspace and layering

An engine-first Cargo workspace. Layering is enforced by `cargo xtask layers`; nothing below L6
depends on egui, eframe, winit or rfd, so the UI crate is swappable. Line counts are Rust lines
including tests, 2026-10-10.

| Layer | Crate | Lines | Job |
|---|---|---|---|
| L0 | `core` | ~1.3k | Cell and range addresses (A1, R1C1, sheet-qualified; 1,048,576 × 16,384), `Value`, `CellError`, Excel coercion, 1900/1904 date serials, typed-input parsing |
| L0 | `numfmt` | ~2.1k | Excel number-format codes (sections, conditions, colours, dates, elapsed time, fractions, scientific, fill), General, `TEXT()`; en-US only |
| L1 | `formula` | ~1.7k | Tokenizer, parser, AST, printer (A1 and R1C1), reference adjustment for copy, fill and structural edits |
| L1 | `functions` | ~13.4k | The function library: `FnSpec` registry (name, arity, category, scalar args, signature, description) and pure implementations; automatic array lifting |
| L2 | `model` | ~2.2k | Workbook, sheets, copy-on-write cell store (64-row bands behind `Arc`), interned styles, names, tables, CF, validation, comments, charts, shapes, images, sparklines, print settings, protection |
| L2 | `calc` | ~2.9k | Evaluator (references, names, tables, LET/LAMBDA, special forms like OFFSET/INDIRECT/SUBTOTAL, [host functions](#host-functions)) and dependency-graph recalculation with dynamic arrays, spills and cycle detection; single-threaded |
| L2 | `xlsx` | ~10.7k | XLSX read/write (transitional and strict read), CSV/TSV, XLSB and ODS data import, PivotTable caches, DrawingML charts and drawings; byte slices only (WASM-safe); lenient reader that reports warnings |
| L2 | `pdf` | ~1k | Small PDF 1.7 writer (14 standard fonts, vectors, images) for print and export |
| L3 | `chart` | ~2.9k | Toolkit-free chart layout to drawing primitives, a rasterizer, sparklines |
| L5 | `engine` | ~17.7k | `Session`: open workbooks, selection, history (COW snapshots), clipboard, fill, sort, filter, PivotTables, what-if, print, file I/O (`io.rs`), and the command registry (`cmd/*`, 313 commands) with the Excel ribbon catalog (`catalog.rs`) |
| L6 | `ui-egui` | ~11.6k | Excel-style UI (title bar, QAT, ribbon, formula bar, virtualised grid, editor, tabs, status bar, dialogs, task panes, chart view) and the control channel |
| L6 | `mcp` | ~1.4k | MCP server over stdio (hand-written JSON-RPC), headless or bridged to a running app |
| L7 | `apps/gridcraft`, `apps/gridcraft-cli`, `apps/gridcraft-web` | ~2.2k | Desktop (eframe/wgpu), CLI (`info`, `convert`, `eval`, `cat`, `run`, `commands`, `functions`, `mcp`, `send`), web (WASM via trunk) |
| — | `xtask` | — | `ci`, `parity`, `assets`, `layers`, `wasm`, `version`, `stats` |

## Data flow

1. **Every action is a command.** The UI, CLI, control channel and MCP call
   `Session::execute(id, params)`. Commands are `CommandSpec { id, label, menu, shortcut, params,
   enabled, run, journal, undoable }`; ids follow Excel's ribbon (`home.bold`, `insert.chart`,
   `data.sortAscending`) plus primitives (`cell.set`, `selection.set`, `range.setValues`).
   `Session::execute` catches escaped panics and restores the workbook (never-crash guard).
2. **Edits** change the model through copy-on-write bands; undo keeps the previous snapshot.
3. **Recalc** marks dependents of changed cells dirty through the dependency graph (with range
   nodes for range dependents), evaluates in topological order, spills dynamic arrays and reports
   `#SPILL!`/cycles. Volatile functions are always dirty, and so are calls of functions that
   aren't built in (they may be defined names holding a LAMBDA), except non-volatile host
   functions.
4. **Rendering**: the egui grid reads display values (number formats applied in `engine/display.rs`)
   for the visible window only; charts render through `crates/chart` primitives.
5. **Files**: `engine/io.rs` sniffs content (XLSX, XLSB, ODS) before trusting the extension and
   dispatches to `crates/xlsx`; imports that lose data return warnings shown to the user.

## Host functions

An application that embeds GridCraft can add worksheet functions under a namespace of its own,
called like add-in functions: `=CONTOSO.LOOKUP(A1,"price")`.

- **API.** `calc::host` (L2) defines the trait `HostFunctions: Send + Sync` (`namespace()`,
  `catalog() -> Vec<HostFunctionInfo>`, `call(name, &[HostArg]) -> HostResult`) and the
  `HostFunctionRegistry`. `HostFunctionInfo { name, description, args: [HostArgInfo { name,
  description, optional }], volatile }` feeds `formulas.functions` (category `UserDefined`, with
  `namespace`, `args` and `volatile`) for autocomplete and Insert Function. `HostArg` is
  `Omitted` or a value (a reference is read: one value, or an array for a block). `HostResult`
  is a value, a 2-D array (it spills like a dynamic array, `#SPILL!` when blocked), `Pending` or
  an error. The engine (L5) holds the registry on the `Session`, shared by every document:
  `Session::register_host_functions(Arc<dyn HostFunctions>)` (a typed call: a trait object
  can't be command parameters), `unregister_host_functions(namespace)`, `host_functions()` and
  `host_data_ready(namespace)`, which runs the command `formulas.refreshHostFunctions
  {namespace?}` (not journaled: data arriving is no user action).
- **Resolution.** Built-in functions first: a namespace never shadows one (`NORM.DIST` stays
  built in with a host registered for `NORM`, and a host declaring a built-in's name is refused).
  Then the host registered for the name's first segment, then a defined name holding a LAMBDA;
  anything else is `#NAME?`. The file prefixes `_xludf.` and `_xll.` are ignored when resolving.
- **Recalculation.** A non-volatile host function depends on the cells its arguments read only,
  so editing other cells never calls the host; a volatile one is calculated every time. The
  graph indexes formulas by the namespaces they call, so registering or removing a host, or
  `host_data_ready`, recalculates just those formulas and their dependents, plus, as after any
  edit, the formulas calculated every time (volatile functions, INDIRECT/OFFSET, names, unknown
  functions); in manual calculation, the namespace's formulas only. Undo and redo ask the host
  again for the restored workbook's host formulas, whose results may be from while the data was
  pending.
- **Pending data.** Calls are synchronous and must return promptly; slow work returns `Pending`,
  shown as `#GETTING_DATA`, until the host reports its data ready. Nothing waits or polls, so it
  works the same on wasm. A host that never reports stays `#GETTING_DATA`.
- **Hostile results.** NaN and infinities are `#NUM!`; text over 32,767 characters, arrays whose
  shape doesn't match their data and the internal `#CIRC!` are `#VALUE!`; arrays over
  10,000,000 cells are `#NUM!`; arrays nested in arrays are `#CALC!` and are dropped without
  recursion however deep they nest; a host that panics gives `#VALUE!`, and one that panics
  while giving its namespace or catalog is refused (native builds; on wasm a panic aborts). The
  typed session calls keep the last-resort guard of `Session::execute`: a panic escaping the
  recalculation they run leaves the workbook as it was.
- **Files.** Formulas keep the host function's plain name (`CONTOSO.LOOKUP(…)`), as files store
  user-defined functions, and are written as dynamic-array formulas (`cm`, `t="array"`) since
  only the host knows whether it returns an array; Excel otherwise shows `=@CONTOSO.LOOKUP(…)`.
  The last values are cached, spilled ones included, so other applications show the numbers.
  The `_xludf.` and `_xll.` prefixes of calls read from a file are written back as files spell
  them.
  The reader keeps cached spilled values as the anchor's spill. Opening calculates every
  formula except those that call something unavailable, an unknown function (no host
  registered) or a host function whose data is pending: they keep the file's values (spills
  included) until they are recalculated, then show `#NAME?` (or the host's result).

## Agent control

- Desktop control channel: `gridcraft --control 7979`, JSON lines on localhost
  (`engine.execute`, `ui.screenshot`, pointer and keyboard input…), see
  [`control-protocol.md`](control-protocol.md).
- MCP: `gridcraft-cli mcp`, headless or `--connect` to a running app, see [`mcp.md`](mcp.md).
- CLI: [`cli.md`](cli.md).

## Platforms and builds

macOS universal, Windows x64/x86/arm64, Linux x86_64/aarch64/riscv64 (AppImage, Flatpak, deb,
rpm, tarball), FreeBSD, and web (WASM; WebGPU or WebGL2). Release workflows in
`.github/workflows/release.yml`; see [`releasing.md`](releasing.md).

## Known architectural gaps

- No threading in calc (see [hardware-parity.md](hardware-parity.md)).
- No string catalog or text shaping (see [localization-parity.md](localization-parity.md)).
- XLSX reader builds the whole workbook in memory; no streaming (#175).
- Unmodelled XLSX parts are not carried through a save.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-14 | trivial | Host functions: refreshes also recalculate the formulas calculated every time (as edits do); a host panicking while declaring its functions is refused; the typed session calls keep the last-resort panic guard; `_xll.` written back in lower case |
| 2026-10-14 | minor | Host functions: the `HostFunctions` API, resolution, recalculation and pending data, hostile results, file convention and cached values on open |
| 2026-10-10 | major | Created from the code on main: crates, layers, data flow, agent control, known gaps |
