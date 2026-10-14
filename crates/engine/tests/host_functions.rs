//! Worksheet functions of the embedding application through the session: registration, pending
//! data and the ready signal, undo, the function list, XLSX round trips with and without the
//! host, and random command sequences around a host whose data arrives late.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gridcraft_core::{Array, CellError, CellRef, Value};
use gridcraft_engine::calc::Calc;
use gridcraft_engine::model::Workbook;
use gridcraft_engine::{EngineError, HostArg, HostFunctionInfo, HostFunctions, HostResult, Session};
use serde_json::{Value as Json, json};

/// A host with data that arrives late: `QUOTE` is pending until [`Contoso::ready`].
#[derive(Default)]
struct Contoso {
    ready: AtomicBool,
    calls: AtomicUsize,
    /// Keys asked for while pending (what the host would go and fetch).
    asked: Mutex<Vec<String>>,
}

impl Contoso {
    fn ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
    }
}

impl HostFunctions for Contoso {
    fn namespace(&self) -> &str {
        "CONTOSO"
    }
    fn catalog(&self) -> Vec<HostFunctionInfo> {
        vec![
            HostFunctionInfo::new("QUOTE", "The price of a key, from a service.").arg("key", "What to price."),
            HostFunctionInfo::new("SEQ", "1..n down a column.").arg("n", "How many."),
            HostFunctionInfo::new("TABLE", "A pending table of n rows by 2 columns.").arg("n", "Rows.").optional_arg("scale", "Multiplier."),
            HostFunctionInfo::new("BOOM", "Panics."),
        ]
    }
    fn call(&self, name: &str, args: &[HostArg]) -> HostResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Like built-in functions, an error in an argument is the result (and so a circular
        // reference stays an error all around its loop).
        if let Some(e) = args.iter().find_map(|a| a.value().and_then(Value::as_error)) {
            return e.into();
        }
        let num = |i: usize| args.get(i).and_then(HostArg::value).and_then(|v| v.to_number().ok());
        match name {
            "QUOTE" => {
                let key = args.first().and_then(HostArg::value).map(|v| v.display()).unwrap_or_default();
                if !self.ready.load(Ordering::SeqCst) {
                    if let Ok(mut asked) = self.asked.lock() {
                        asked.push(key);
                    }
                    return HostResult::Pending;
                }
                // A deterministic price per key.
                let price = key.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b))) % 1000;
                f64::from(price).into()
            }
            "SEQ" => match num(0) {
                Some(n) if (1.0..=100.0).contains(&n) => {
                    HostResult::Array(Array::column((1..=n as usize).map(|i| Value::Number(i as f64)).collect()))
                }
                _ => CellError::Num.into(),
            },
            "TABLE" => {
                if !self.ready.load(Ordering::SeqCst) {
                    return HostResult::Pending;
                }
                let scale = num(1).unwrap_or(1.0);
                match num(0) {
                    Some(n) if (1.0..=20.0).contains(&n) => {
                        let rows = n as usize;
                        let data = (0..rows * 2).map(|i| Value::Number(i as f64 * scale)).collect();
                        HostResult::Array(Array { rows, cols: 2, data })
                    }
                    _ => CellError::Num.into(),
                }
            }
            "BOOM" => panic!("host bug"),
            _ => CellError::Name.into(),
        }
    }
}

fn set(s: &mut Session, cell: &str, input: &str) {
    s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
}

fn value(s: &Session, cell: &str) -> Value {
    s.doc().unwrap().wb.sheet(0).unwrap().value(CellRef::parse(cell).unwrap())
}

fn session() -> (Session, Arc<Contoso>) {
    let host = Arc::new(Contoso::default());
    let mut s = Session::new();
    assert_eq!(s.register_host_functions(host.clone()).unwrap(), "CONTOSO");
    s.new_workbook();
    (s, host)
}

const GETTING_DATA: Value = Value::Error(CellError::GettingData);

#[test]
fn pending_formulas_get_their_data_when_the_host_reports_it_ready() {
    let (mut s, host) = session();
    set(&mut s, "A1", "AAPL");
    set(&mut s, "B1", "=CONTOSO.QUOTE(A1)");
    set(&mut s, "C1", "=B1*2");
    set(&mut s, "D1", "=CONTOSO.TABLE(3)");
    assert_eq!([value(&s, "B1"), value(&s, "C1"), value(&s, "D1")], [GETTING_DATA, GETTING_DATA, GETTING_DATA]);
    assert_eq!(host.asked.lock().unwrap().as_slice(), ["AAPL"]);
    let journal = s.journal.len();
    host.ready();
    // The signal recalculates the host's formulas and their dependents.
    assert_eq!(s.host_data_ready("contoso").unwrap(), 3);
    let price = value(&s, "B1").as_f64().unwrap();
    assert_eq!(value(&s, "C1"), Value::Number(price * 2.0));
    // The pending table spills now.
    assert_eq!((value(&s, "D1"), value(&s, "E3")), (Value::Number(0.0), Value::Number(5.0)));
    assert_eq!(s.journal.len(), journal, "data arriving is not a user action to record");
    // A namespace nobody calls recalculates nothing.
    assert_eq!(s.host_data_ready("FABRIKAM").unwrap(), 0);
}

#[test]
fn undo_back_to_a_pending_workbook_asks_the_host_again() {
    let (mut s, host) = session();
    set(&mut s, "A1", "=CONTOSO.QUOTE(\"X\")");
    set(&mut s, "A2", "=A1+1");
    set(&mut s, "B1", "7");
    host.ready();
    s.host_data_ready("CONTOSO").unwrap();
    let ready = value(&s, "A2");
    assert!(ready.as_f64().is_some());
    // Undoing the last edit restores a workbook from while the quote was pending.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!((value(&s, "B1"), value(&s, "A2")), (Value::Empty, ready.clone()));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!((value(&s, "B1"), value(&s, "A2")), (Value::Number(7.0), ready));
}

#[test]
fn registration_recalculates_open_workbooks_and_removal_gives_name_errors() {
    let host = Arc::new(Contoso::default());
    let mut s = Session::new();
    s.new_workbook();
    set(&mut s, "A1", "=CONTOSO.SEQ(2)");
    set(&mut s, "B1", "=SUM(A1#)");
    assert_eq!(value(&s, "A1"), Value::Error(CellError::Name));
    s.register_host_functions(host.clone()).unwrap();
    assert_eq!((value(&s, "A2"), value(&s, "B1")), (Value::Number(2.0), Value::Number(3.0)));
    // Re-registering re-reads the catalog; a bad host is refused and changes nothing.
    s.register_host_functions(host.clone()).unwrap();
    struct Bad;
    impl HostFunctions for Bad {
        fn namespace(&self) -> &str {
            "A1"
        }
        fn catalog(&self) -> Vec<HostFunctionInfo> {
            vec![]
        }
        fn call(&self, _: &str, _: &[HostArg]) -> HostResult {
            HostResult::Pending
        }
    }
    assert!(matches!(s.register_host_functions(Arc::new(Bad)), Err(EngineError::Other(_))));
    assert!(s.unregister_host_functions("contoso"));
    assert!(!s.unregister_host_functions("contoso"));
    assert_eq!((value(&s, "A1"), value(&s, "A2")), (Value::Error(CellError::Name), Value::Empty));
}

#[test]
fn a_panicking_host_gives_an_error_value_not_a_failed_command() {
    let (mut s, _) = session();
    set(&mut s, "A1", "=CONTOSO.BOOM()");
    assert_eq!(value(&s, "A1"), Value::Error(CellError::Value));
}

#[test]
fn the_function_list_and_evaluate_know_host_functions() {
    let (mut s, _) = session();
    let list = s.execute("formulas.functions", json!({"category": "UserDefined"})).unwrap();
    let names: Vec<&str> = list.as_array().unwrap().iter().filter_map(|f| f["name"].as_str()).collect();
    assert_eq!(names, ["CONTOSO.BOOM", "CONTOSO.QUOTE", "CONTOSO.SEQ", "CONTOSO.TABLE"]);
    let table = &list[3];
    assert_eq!(table["signature"], "CONTOSO.TABLE(n, [scale])");
    assert_eq!(table["namespace"], "CONTOSO");
    assert_eq!(table["args"][1], json!({"name": "scale", "description": "Multiplier.", "optional": true}));
    assert_eq!(table["volatile"], false);
    let found = s.execute("formulas.functions", json!({"search": "from a service"})).unwrap();
    assert_eq!(found[0]["name"], "CONTOSO.QUOTE");
    assert_eq!(s.execute("formulas.evaluate", json!({"formula": "=SUM(CONTOSO.SEQ(4))"})).unwrap(), json!(10.0));
    set(&mut s, "A1", "=SUM(CONTOSO.SEQ(3))*2");
    let steps = s.execute("formulas.evaluateFormula", json!({"cell": "A1"})).unwrap();
    assert_eq!(steps["result"], json!(12.0));
}

fn save(s: &mut Session) -> String {
    s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap()["base64"].as_str().unwrap().to_string()
}

fn open(s: &mut Session, base64: &str) {
    s.execute("file.open", json!({"name": "book.xlsx", "base64": base64})).unwrap();
}

#[test]
fn xlsx_round_trip_with_the_host_and_without_it() {
    let (mut s, host) = session();
    host.ready();
    set(&mut s, "A1", "=CONTOSO.QUOTE(\"X\")");
    set(&mut s, "B1", "=CONTOSO.SEQ(3)");
    set(&mut s, "C1", "=SUM(B1#)+A1");
    let price = value(&s, "A1");
    let file = save(&mut s);

    // Another application (or this one without the host): the cached values, spills included,
    // until a recalculation reaches the formulas.
    let mut plain = Session::new();
    open(&mut plain, &file);
    let formula =
        |s: &Session, a: &str| s.doc().unwrap().wb.sheet(0).unwrap().cell(CellRef::parse(a).unwrap()).unwrap().formula.as_ref().unwrap().text.clone();
    assert_eq!((formula(&plain, "A1"), formula(&plain, "B1")), ("CONTOSO.QUOTE(\"X\")".to_string(), "CONTOSO.SEQ(3)".to_string()));
    assert_eq!(
        [value(&plain, "A1"), value(&plain, "B3"), value(&plain, "C1")],
        [price.clone(), Value::Number(3.0), Value::Number(6.0 + price.as_f64().unwrap())]
    );
    assert!(!plain.doc().unwrap().is_dirty());
    plain.execute("formulas.calculateNow", json!({})).unwrap();
    assert_eq!(
        [value(&plain, "A1"), value(&plain, "B1"), value(&plain, "B2")],
        [Value::Error(CellError::Name), Value::Error(CellError::Name), Value::Empty]
    );

    // With the host, but its data not there yet: the cached values while pending, then the data.
    let late = Arc::new(Contoso::default());
    let mut again = Session::new();
    again.register_host_functions(late.clone()).unwrap();
    open(&mut again, &file);
    assert_eq!(
        [value(&again, "A1"), value(&again, "B3"), value(&again, "C1")],
        [price.clone(), Value::Number(3.0), Value::Number(6.0 + price.as_f64().unwrap())]
    );
    assert_eq!(late.asked.lock().unwrap().as_slice(), ["X"], "the host was asked while opening");
    late.ready();
    assert!(again.host_data_ready("CONTOSO").unwrap() >= 2);
    assert_eq!(value(&again, "A1"), price);

    // Registered after opening: the formulas are calculated then.
    let mut later = Session::new();
    open(&mut later, &file);
    let host = Arc::new(Contoso::default());
    host.ready();
    later.register_host_functions(host).unwrap();
    assert_eq!([value(&later, "A1"), value(&later, "B2")], [price, Value::Number(2.0)]);
}

#[test]
fn a_pending_workbook_saves_and_reopens_as_pending() {
    let (mut s, _) = session();
    set(&mut s, "A1", "=CONTOSO.QUOTE(\"X\")");
    let file = save(&mut s);
    let (mut other, host) = session();
    open(&mut other, &file);
    assert_eq!(value(&other, "A1"), GETTING_DATA);
    host.ready();
    other.host_data_ready("CONTOSO").unwrap();
    assert!(value(&other, "A1").as_f64().is_some());
}

// ---------------------------------------------------------------- command sequences

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

fn cell(rng: &mut Rng) -> String {
    format!("{}{}", ["A", "B", "C", "D", "E"][rng.below(5)], 1 + rng.below(8))
}

fn input(rng: &mut Rng) -> String {
    let (c, n) = (cell(rng), 1 + rng.below(4));
    match rng.below(12) {
        0 => format!("=CONTOSO.QUOTE({c})"),
        1 => format!("=CONTOSO.QUOTE(\"K{n}\")*{c}"),
        2 => format!("=CONTOSO.SEQ({n})"),
        3 => format!("=CONTOSO.TABLE({n},{c})"),
        4 => format!("=SUM({c}:E8)+{n}"),
        5 => format!("={c}*2"),
        6 => "=CONTOSO.BOOM()".into(),
        7 => format!("=IFERROR(CONTOSO.QUOTE({c}),-1)"),
        8 => format!("=FABRIKAM.X({c})"),
        9 => String::new(),
        _ => format!("{}", rng.below(100)),
    }
}

fn step(rng: &mut Rng) -> (String, Json) {
    match rng.below(20) {
        0..=8 => ("cell.set".into(), json!({"cell": cell(rng), "input": input(rng)})),
        9 => ("range.setValues".into(), json!({"range": cell(rng), "values": [[input(rng), input(rng)], [rng.below(9), "x"]]})),
        10 => ("edit.undo".into(), json!({"steps": 1 + rng.below(2)})),
        11 => ("edit.redo".into(), json!({})),
        12 => ("home.insertRows".into(), json!({"rows": format!("{0}:{0}", 1 + rng.below(6))})),
        13 => ("home.deleteRows".into(), json!({"rows": format!("{0}:{0}", 1 + rng.below(6))})),
        14 => ("home.insertColumns".into(), json!({"cols": rng.pick(&["A:A", "B:B", "C:D"])})),
        15 => ("edit.copy".into(), json!({"range": format!("{}:{}", cell(rng), cell(rng))})),
        16 => ("edit.paste".into(), json!({"at": cell(rng)})),
        17 => ("data.sortAscending".into(), json!({"range": "A1:E8", "column": rng.pick(&["A", "B", "C"])})),
        18 => ("formulas.calculateNow".into(), json!({})),
        _ => ("edit.clearContents".into(), json!({"range": format!("{}:{}", cell(rng), cell(rng))})),
    }
}

/// The first cell whose value differs. `file`: `b` was saved and reopened, and files cache
/// `#SPILL!` and `#CALC!` as `#VALUE!`.
fn values_differ(a: &Workbook, b: &Workbook, file: bool) -> Option<String> {
    let as_cached = |v: Value| match v {
        Value::Error(CellError::Spill | CellError::Calc) if file => Value::Error(CellError::Value),
        v => v,
    };
    for (x, y) in a.sheets.iter().zip(&b.sheets) {
        let mut keys: Vec<CellRef> = x.cells.iter().map(|(c, _)| c).chain(y.cells.iter().map(|(c, _)| c)).collect();
        keys.extend(x.spill.keys().chain(y.spill.keys()).copied());
        keys.sort_unstable_by_key(|c| (c.row, c.col));
        keys.dedup();
        for c in keys {
            if as_cached(x.value(c)) != as_cached(y.value(c)) {
                let f = x.cell(c).and_then(|c| c.formula.as_ref()).map(|f| format!(" (={})", f.text)).unwrap_or_default();
                let dump = |sh: &gridcraft_engine::model::Sheet| {
                    let cells: Vec<String> = sh
                        .cells
                        .iter()
                        .map(|(c, cell)| match &cell.formula {
                            Some(f) => format!("{c}: ={} -> {:?}", f.text, cell.value),
                            None => format!("{c}: {:?}", cell.value),
                        })
                        .collect();
                    format!("{}\nspills: {:?}", cells.join("\n"), sh.spill_ranges)
                };
                return Some(format!(
                    "{}!{c}{f}: {:?} vs {:?}\n--- incremental:\n{}\n--- other:\n{}",
                    x.name,
                    x.value(c),
                    y.value(c),
                    dump(x),
                    dump(y)
                ));
            }
        }
    }
    None
}

/// Whether a formula shows `#SPILL!`.
fn spill_conflict(wb: &Workbook) -> bool {
    wb.sheets.iter().any(|sh| sh.cells.iter().any(|(_, c)| c.formula.is_some() && c.value == Value::Error(CellError::Spill)))
}

/// Whether the first sheet has a circular reference (an `IFERROR` may hide its `#CIRC!`), whose
/// values depend on where evaluation entered the loop, so a full recalculation may legitimately
/// differ. Formula → formulas its precedents cover (directly or through their spills).
fn circular(wb: &Workbook, calc: &Calc) -> bool {
    let Some(sh) = wb.sheet(0) else { return false };
    let formulas: Vec<CellRef> = sh.cells.iter().filter(|(_, c)| c.formula.is_some()).map(|(c, _)| c).collect();
    let covers = |area: &gridcraft_engine::calc::Area, g: CellRef| {
        area.sheet == 0 && (area.range.contains(g) || sh.spill_ranges.get(&g).is_some_and(|r| r.intersects(&area.range)))
    };
    let edges: Vec<Vec<usize>> = formulas
        .iter()
        .map(|f| {
            let areas = calc.graph.precedents_of((0, *f));
            formulas.iter().enumerate().filter(|(_, g)| areas.iter().any(|a| covers(a, **g))).map(|(i, _)| i).collect()
        })
        .collect();
    // Depth-first search for a back edge: 0 unvisited, 1 on the stack, 2 done.
    let mut state = vec![0u8; formulas.len()];
    for start in 0..formulas.len() {
        if state[start] != 0 {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        state[start] = 1;
        while let Some((node, next)) = stack.pop() {
            match edges[node].get(next) {
                Some(&to) => {
                    stack.push((node, next + 1));
                    match state[to] {
                        1 => return true,
                        0 => {
                            state[to] = 1;
                            stack.push((to, 0));
                        }
                        _ => {}
                    }
                }
                None => state[node] = 2,
            }
        }
    }
    false
}

/// Random edits around host formulas, with the host's data arriving partway through. After every
/// step: no command panics (or fails because of the host), and a full recalculation with the same
/// host state gives the values the incremental recalculation left (circular references aside). At
/// the end: the data has arrived everywhere, and an XLSX save and reopen gives the same formulas
/// and values.
#[test]
fn command_sequences_with_late_host_data_stay_consistent() {
    let seeds: u64 = std::env::var("GRIDCRAFT_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
    let steps: usize = std::env::var("GRIDCRAFT_FUZZ_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let only: Option<u64> = std::env::var("GRIDCRAFT_FUZZ_SEED").ok().and_then(|v| v.parse().ok());
    for seed in only.map_or(0..seeds, |s| s..s + 1) {
        let mut rng = Rng(seed ^ 0x40_57_F0_0D);
        let (mut s, host) = session();
        let ready_at = rng.below(steps);
        let mut log = Vec::new();
        for i in 0..steps {
            if i == ready_at {
                host.ready();
                s.host_data_ready("CONTOSO").unwrap();
                log.push("<host data ready>".to_string());
            }
            let (id, params) = step(&mut rng);
            log.push(format!("{id} {params}"));
            let before = s.doc().unwrap().wb.clone();
            if let Err(EngineError::Internal(_, msg)) = s.execute(&id, params) {
                panic!("seed {seed}: `{id}` panicked: {msg}\n{}", log.join("\n"));
            }
            s.take_ui_requests();
            let wb = s.doc().unwrap().wb.clone();
            let mut fresh = (*wb).clone();
            let mut calc = Calc::new();
            calc.set_host_functions(s.host_functions().clone());
            calc.recalc_all(&mut fresh);
            if circular(&fresh, &calc) {
                continue;
            }
            // Two arrays that would spill over the same cells: which one spills depends on the
            // order they were calculated in, as with any dynamic array.
            if let Some(diff) = values_differ(&wb, &fresh, false)
                && !(spill_conflict(&wb) || spill_conflict(&fresh))
            {
                let before = values_differ(&before, &Workbook::new(), false).unwrap_or_default();
                panic!("seed {seed}, step {i}: a full recalculation disagrees: {diff}\n--- before the step: {before}\n{}", log.join("\n"));
            }
        }
        host.ready();
        s.host_data_ready("CONTOSO").unwrap();
        let wb = s.doc().unwrap().wb.clone();
        assert!(
            !wb.sheets.iter().any(|sh| sh.cells.iter().any(|(_, c)| c.value == GETTING_DATA)),
            "seed {seed}: pending values after the data arrived\n{}",
            log.join("\n")
        );
        let file = save(&mut s);
        let (mut back, _) = session();
        open(&mut back, &file);
        let reopened = back.doc().unwrap().wb.clone();
        for (x, y) in wb.sheets.iter().zip(&reopened.sheets) {
            let formulas = |sh: &gridcraft_engine::model::Sheet| {
                sh.cells.iter().filter_map(|(c, cell)| Some((c, cell.formula.as_ref()?.text.clone()))).collect::<Vec<_>>()
            };
            assert_eq!(formulas(x), formulas(y), "seed {seed}");
        }
        // The reopened host is pending again: the cached values show until its data arrives.
        if circular(&reopened, &back.doc().unwrap().calc) {
            continue;
        }
        if let Some(diff) = values_differ(&wb, &reopened, true) {
            panic!("seed {seed}: reopened values differ: {diff}\n{}", log.join("\n"));
        }
    }
}
