//! Host functions in formulas and recalculation, with a fake `CONTOSO` host.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gridcraft_core::{Array, CellError, CellRef, RangeRef, Value};
use gridcraft_model::{CalcMode, Cell, Formula, Workbook};

use crate::host::{HostArg, HostFunctionInfo, HostFunctionRegistry, HostFunctions, HostResult, MAX_HOST_ARRAY_CELLS};
use crate::{Calc, Key};

fn c(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

/// A host whose functions cover the cases: plain values, arrays, pending data, errors and hostile
/// results. Counts its calls.
#[derive(Default)]
struct Contoso {
    calls: AtomicUsize,
    /// `QUOTE` is pending until this is set.
    ready: AtomicBool,
    /// Function names called, in order.
    log: Mutex<Vec<String>>,
}

impl Contoso {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl HostFunctions for Contoso {
    fn namespace(&self) -> &str {
        "Contoso"
    }
    fn catalog(&self) -> Vec<HostFunctionInfo> {
        vec![
            HostFunctionInfo::new("Add", "Adds two numbers.").arg("a", "").arg("b", ""),
            HostFunctionInfo::new("Seq", "1..n down a column.").arg("n", ""),
            HostFunctionInfo::new("Quote", "A price fetched from a service.").arg("key", ""),
            HostFunctionInfo::new("Tick", "Counts its calls.").volatile(),
            HostFunctionInfo::new("Hostile", "Misbehaves.").arg("kind", ""),
            HostFunctionInfo::new("Args", "Describes its arguments.").optional_arg("any", ""),
            HostFunctionInfo::new("Stats.Mean", "Mean of a range.").arg("values", ""),
        ]
    }
    fn call(&self, name: &str, args: &[HostArg]) -> HostResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut log) = self.log.lock() {
            log.push(name.to_string());
        }
        let num = |i: usize| args.get(i).and_then(HostArg::value).and_then(|v| v.as_f64());
        match name {
            "Add" => match (num(0), num(1)) {
                (Some(a), Some(b)) => (a + b).into(),
                _ => CellError::Value.into(),
            },
            "Seq" => {
                let n = num(0).unwrap_or(0.0) as usize;
                if n == 0 {
                    return CellError::Value.into();
                }
                HostResult::Array(Array::column((1..=n).map(|i| Value::Number(i as f64)).collect()))
            }
            "Quote" => {
                if !self.ready.load(Ordering::SeqCst) {
                    return HostResult::Pending;
                }
                match args.first().and_then(HostArg::value) {
                    Some(Value::Text(t)) if &**t == "ABC" => 101.5.into(),
                    _ => CellError::NA.into(),
                }
            }
            "Tick" => (self.calls() as f64).into(),
            "Hostile" => match num(0).unwrap_or(0.0) as u32 {
                0 => f64::NAN.into(),
                1 => f64::NEG_INFINITY.into(),
                2 => HostResult::Array(Array { rows: 3, cols: 3, data: vec![Value::Number(1.0)] }),
                3 => HostResult::Array(Array { rows: usize::MAX, cols: usize::MAX, data: vec![] }),
                4 => HostResult::Array(Array { rows: MAX_HOST_ARRAY_CELLS + 1, cols: 1, data: vec![] }),
                5 => HostResult::Value(Value::from(Array::column(vec![Value::Number(1.0), Value::from(Array::scalar(Value::Number(2.0)))]))),
                6 => HostResult::Error(CellError::Circ),
                7 => HostResult::Value(Value::text("x".repeat(40_000))),
                8 => HostResult::Error(CellError::GettingData),
                9 => {
                    let mut v = Value::Number(1.0);
                    for _ in 0..200_000 {
                        v = Value::from(Array::scalar(v));
                    }
                    HostResult::Value(v)
                }
                _ => panic!("host failure"),
            },
            "Args" => {
                let describe = |a: &HostArg| match a {
                    HostArg::Omitted => "omitted".to_string(),
                    HostArg::Value(Value::Array(a)) => format!("array{}x{}", a.rows, a.cols),
                    HostArg::Value(Value::Empty) => "empty".to_string(),
                    HostArg::Value(Value::Number(_)) => "number".to_string(),
                    HostArg::Value(Value::Text(_)) => "text".to_string(),
                    HostArg::Value(Value::Error(e)) => e.to_string(),
                    HostArg::Value(_) => "other".to_string(),
                };
                Value::text(args.iter().map(describe).collect::<Vec<_>>().join(",")).into()
            }
            "Stats.Mean" => match args.first().and_then(HostArg::value) {
                Some(Value::Array(a)) => {
                    let nums: Vec<f64> = a.data.iter().filter_map(Value::as_f64).collect();
                    if nums.is_empty() { CellError::Div0.into() } else { (nums.iter().sum::<f64>() / nums.len() as f64).into() }
                }
                Some(v) => v.as_f64().map_or(HostResult::Error(CellError::Value), HostResult::from),
                None => CellError::Value.into(),
            },
            _ => CellError::Name.into(),
        }
    }
}

struct T {
    wb: Workbook,
    calc: Calc,
    host: Arc<Contoso>,
}

impl T {
    /// A workbook with the `CONTOSO` host registered.
    fn new() -> T {
        let mut t = T::bare();
        t.register();
        t
    }
    /// No host registered.
    fn bare() -> T {
        T { wb: Workbook::new(), calc: Calc::new(), host: Arc::new(Contoso::default()) }
    }
    fn register(&mut self) {
        let mut reg = HostFunctionRegistry::new();
        reg.register(self.host.clone()).unwrap();
        self.calc.set_host_functions(Arc::new(reg));
        self.calc.rebuild(&self.wb);
        self.calc.recalc_host_calls(&mut self.wb, Some("CONTOSO"));
    }
    fn set(&mut self, at: &str, input: &str) {
        let cell = if input.starts_with('=') {
            Cell::formula(Formula::new(input))
        } else {
            Cell::value(gridcraft_core::parse::parse_input(input, self.wb.date_system).value)
        };
        self.wb.sheet_mut(0).unwrap().cells.set(c(at), cell);
        self.calc.cells_changed(&mut self.wb, &[(0, c(at))]);
    }
    fn get(&self, at: &str) -> Value {
        self.wb.sheet(0).unwrap().value(c(at))
    }
    fn err(&self, at: &str) -> Option<CellError> {
        self.get(at).as_error()
    }
    fn ready(&mut self) -> usize {
        self.host.ready.store(true, Ordering::SeqCst);
        self.calc.recalc_host_calls(&mut self.wb, Some("contoso"))
    }
}

#[test]
fn scalar_results_recalculate_with_their_arguments_only() {
    let mut t = T::new();
    t.set("A1", "2");
    t.set("B1", "=CONTOSO.ADD(A1,3)");
    t.set("C1", "=B1*10");
    assert_eq!((t.get("B1"), t.get("C1")), (Value::Number(5.0), Value::Number(50.0)));
    // Names match ignoring case; the host receives its own spelling.
    t.set("D1", "=contoso.add(1,1)");
    assert_eq!(t.get("D1"), Value::Number(2.0));
    assert_eq!(t.host.log.lock().unwrap().last().map(String::as_str), Some("Add"));
    t.set("A1", "7");
    assert_eq!((t.get("B1"), t.get("C1")), (Value::Number(10.0), Value::Number(100.0)));
    // A non-volatile host function isn't called again for an unrelated edit.
    let calls = t.host.calls();
    t.set("Z9", "1");
    t.set("Z10", "=Z9+1");
    assert_eq!(t.host.calls(), calls);
    // A namespace with more than one dot.
    t.set("E1", "1");
    t.set("E2", "3");
    t.set("E3", "=CONTOSO.STATS.MEAN(E1:E2)");
    assert_eq!(t.get("E3"), Value::Number(2.0));
    t.set("E2", "5");
    assert_eq!(t.get("E3"), Value::Number(3.0));
}

#[test]
fn volatile_host_functions_recalculate_every_time() {
    let mut t = T::new();
    t.set("A1", "=CONTOSO.TICK()");
    let first = t.get("A1");
    t.set("Z1", "1");
    assert_ne!(t.get("A1"), first, "a volatile host function is called on every recalculation");
}

#[test]
fn arguments_reach_the_host_as_values() {
    let mut t = T::new();
    t.set("A1", "1");
    t.set("A2", "2");
    t.set("B1", "=CONTOSO.ARGS(,A1:A2,C9,\"x\",1/0,A1,)");
    assert_eq!(t.get("B1"), Value::text("omitted,array2x1,empty,text,#DIV/0!,number,omitted"));
    t.set("B2", "=CONTOSO.ARGS()");
    assert_eq!(t.get("B2"), Value::text(""));
    // Arguments that are themselves host calls and arrays.
    t.set("B3", "=CONTOSO.ADD(CONTOSO.ADD(1,2),SUM({1,2}))");
    assert_eq!(t.get("B3"), Value::Number(6.0));
}

#[test]
fn array_results_spill_and_block() {
    let mut t = T::new();
    t.set("A1", "3");
    t.set("B1", "=CONTOSO.SEQ(A1)");
    t.set("C1", "=SUM(B1#)");
    assert_eq!((t.get("B1"), t.get("B3"), t.get("C1")), (Value::Number(1.0), Value::Number(3.0), Value::Number(6.0)));
    t.set("A1", "4");
    assert_eq!((t.get("B4"), t.get("C1")), (Value::Number(4.0), Value::Number(10.0)));
    // Something in the way: #SPILL!, and the spill comes back when it's cleared.
    t.set("B3", "x");
    assert_eq!(t.err("B1"), Some(CellError::Spill));
    t.set("B3", "");
    assert_eq!((t.get("B3"), t.get("C1")), (Value::Number(3.0), Value::Number(10.0)));
}

#[test]
fn pending_data_shows_getting_data_until_the_host_reports_it_ready() {
    let mut t = T::new();
    t.set("A1", "ABC");
    t.set("B1", "=CONTOSO.QUOTE(A1)");
    t.set("C1", "=B1*2");
    t.set("D1", "=IFERROR(B1,0)");
    // Unrelated formulas.
    for r in 10..60 {
        t.set(&format!("E{r}"), &format!("=ROW()*{r}"));
    }
    assert_eq!((t.err("B1"), t.err("C1"), t.get("D1")), (Some(CellError::GettingData), Some(CellError::GettingData), Value::Number(0.0)));
    // Asked again while still pending: it stays pending, nothing loops or waits.
    assert_eq!(t.calc.recalc_host_calls(&mut t.wb, Some("CONTOSO")), 3);
    assert_eq!(t.err("B1"), Some(CellError::GettingData));
    // Ready: the host's formulas and their dependents only.
    assert_eq!(t.ready(), 3);
    assert_eq!((t.get("B1"), t.get("C1"), t.get("D1")), (Value::Number(101.5), Value::Number(203.0), Value::Number(101.5)));
    // Another namespace's news recalculates nothing here.
    assert_eq!(t.calc.recalc_host_calls(&mut t.wb, Some("OTHER")), 0);
    // A key the host doesn't know.
    t.set("A1", "XYZ");
    assert_eq!(t.err("B1"), Some(CellError::NA));
}

#[test]
fn results_restored_from_while_pending_are_asked_again() {
    let mut t = T::new();
    t.set("A1", "=CONTOSO.QUOTE(\"ABC\")");
    t.set("A2", "=A1+1");
    t.set("A3", "=IFERROR(CONTOSO.QUOTE(\"ABC\"),-1)");
    t.set("B1", "=FABRIKAM.X()");
    let pending_book = t.wb.clone();
    // The unknown function counts too: it may read anything, so it is calculated every time.
    assert_eq!(t.ready(), 4);
    // Undo brings back a workbook from while the data was pending, even where the pending error
    // was caught.
    t.wb = pending_book;
    t.calc.rebuild(&t.wb);
    assert_eq!((t.err("A2"), t.get("A3")), (Some(CellError::GettingData), Value::Number(-1.0)));
    // Every registered namespace's formulas (FABRIKAM has no host), their dependents, and the
    // formulas calculated every time.
    assert_eq!(t.calc.recalc_host_calls(&mut t.wb, None), 4);
    assert_eq!((t.get("A1"), t.get("A2"), t.get("A3")), (Value::Number(101.5), Value::Number(102.5), Value::Number(101.5)));
}

#[test]
fn manual_calculation_recalculates_only_the_host_formulas() {
    let mut t = T::new();
    t.set("A1", "=CONTOSO.QUOTE(\"ABC\")");
    t.set("A2", "=A1+1");
    t.wb.calc.mode = CalcMode::Manual;
    assert_eq!(t.ready(), 1);
    assert_eq!((t.get("A1"), t.err("A2")), (Value::Number(101.5), Some(CellError::GettingData)));
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.get("A2"), Value::Number(102.5));
}

#[test]
fn errors_and_hostile_results_are_contained() {
    let mut t = T::new();
    let cases = [
        (0, CellError::Num),
        (1, CellError::Num),
        (2, CellError::Value),
        (3, CellError::Value),
        (4, CellError::Num),
        (6, CellError::Value),
        (7, CellError::Value),
        (8, CellError::GettingData),
        (10, CellError::Value),
    ];
    for (kind, want) in cases {
        t.set("A1", &format!("=CONTOSO.HOSTILE({kind})"));
        assert_eq!(t.err("A1"), Some(want), "kind {kind}");
    }
    // A nested array: the element is #CALC!, the rest spills.
    t.set("A1", "=CONTOSO.HOSTILE(5)");
    assert_eq!((t.get("A1"), t.err("A2")), (Value::Number(1.0), Some(CellError::Calc)));
    // Nesting far deeper than recursion would survive.
    t.set("A1", "=CONTOSO.HOSTILE(9)");
    assert_eq!(t.err("A1"), Some(CellError::Calc));
    // A function the namespace doesn't have, and a namespace with no host, are unknown functions.
    t.set("B1", "=CONTOSO.NOPE(1)");
    t.set("B2", "=FABRIKAM.LOOKUP(1)");
    t.set("B3", "=CONTOSO.ADD(\"a\",1)");
    assert_eq!((t.err("B1"), t.err("B2"), t.err("B3")), (Some(CellError::Name), Some(CellError::Name), Some(CellError::Value)));
}

#[test]
fn built_ins_stay_built_in_and_udf_prefixes_resolve() {
    struct Norm;
    impl HostFunctions for Norm {
        fn namespace(&self) -> &str {
            "NORM"
        }
        fn catalog(&self) -> Vec<HostFunctionInfo> {
            vec![HostFunctionInfo::new("FIT", "")]
        }
        fn call(&self, _: &str, _: &[HostArg]) -> HostResult {
            HostResult::Value(Value::Number(7.0))
        }
    }
    let mut t = T::new();
    let mut reg = (**t.calc.host_functions()).clone();
    reg.register(Arc::new(Norm)).unwrap();
    t.calc.set_host_functions(Arc::new(reg));
    t.set("A1", "=NORM.DIST(0,0,1,TRUE)");
    t.set("A2", "=NORM.FIT()");
    t.set("A3", "=_xludf.CONTOSO.ADD(1,2)");
    t.set("A4", "=_xludf.SUM(1,2)");
    assert_eq!((t.get("A1"), t.get("A2"), t.get("A3")), (Value::Number(0.5), Value::Number(7.0), Value::Number(3.0)));
    assert_eq!(t.err("A4"), Some(CellError::Name), "`_xludf.` never means the built-in");
}

#[test]
fn registering_and_removing_a_host_recalculates_its_formulas() {
    let mut t = T::bare();
    t.set("A1", "=CONTOSO.ADD(1,2)");
    t.set("A2", "=A1*2");
    assert_eq!(t.err("A2"), Some(CellError::Name));
    t.register();
    assert_eq!((t.get("A1"), t.get("A2")), (Value::Number(3.0), Value::Number(6.0)));
    // The call is not volatile any more: an unrelated edit leaves the host alone.
    let calls = t.host.calls();
    t.set("Z1", "1");
    assert_eq!(t.host.calls(), calls);
    t.calc.set_host_functions(Arc::default());
    t.calc.rebuild(&t.wb);
    t.calc.recalc_host_calls(&mut t.wb, Some("CONTOSO"));
    assert_eq!((t.err("A1"), t.err("A2")), (Some(CellError::Name), Some(CellError::Name)));
}

#[test]
fn host_callers_are_indexed_by_namespace() {
    let mut t = T::bare();
    t.set("A1", "=CONTOSO.ADD(1,2)+FABRIKAM.X()");
    t.set("A2", "=SUM(1)+NORM.DIST(0,0,1,TRUE)");
    t.set("A3", "=_xludf.CONTOSO.ADD(1,2)");
    let keys = |t: &T, ns: Option<&str>| t.calc.graph.host_callers(ns).iter().map(|(_, c)| c.a1()).collect::<Vec<_>>();
    assert_eq!(keys(&t, Some("contoso")), ["A1", "A3"]);
    assert_eq!(keys(&t, Some("FABRIKAM")), ["A1"]);
    assert_eq!(keys(&t, None), ["A1", "A3"]);
    t.set("A1", "1");
    assert_eq!(keys(&t, Some("FABRIKAM")), Vec::<String>::new());
}

#[test]
fn evaluate_uses_the_host_functions() {
    let t = T::new();
    assert_eq!(t.calc.evaluate(&t.wb, 0, c("A1"), "=CONTOSO.ADD(2,2)"), Value::Number(4.0));
    assert_eq!(crate::evaluate(&t.wb, 0, c("A1"), "=CONTOSO.ADD(2,2)"), Value::Error(CellError::Name));
}

/// A workbook as read from a file another application saved: host formulas with cached values.
fn loaded() -> Workbook {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    let mut cell = |at: &str, f: &str, v: Value| {
        let mut x = Cell::formula(Formula::new(f));
        x.value = v;
        sh.cells.set(c(at), x);
    };
    cell("A1", "CONTOSO.QUOTE(\"ABC\")", Value::Number(99.0));
    cell("A2", "A1+1", Value::Number(100.0));
    cell("B1", "CONTOSO.SEQ(3)", Value::Number(1.0));
    cell("C1", "SUM(B1#)", Value::Number(6.0));
    cell("D1", "1+1", Value::Number(5.0));
    // An unknown function with nothing cached.
    cell("E1", "FABRIKAM.X()", Value::Empty);
    sh.spill_ranges.insert(c("B1"), RangeRef::parse("B1:B3").unwrap());
    sh.spill.insert(c("B2"), Value::Number(2.0));
    sh.spill.insert(c("B3"), Value::Number(3.0));
    wb
}

#[test]
fn without_the_host_a_loaded_workbook_shows_cached_values_until_recalculated() {
    let mut wb = loaded();
    let mut calc = Calc::new();
    calc.recalc_loaded(&mut wb);
    let v = |wb: &Workbook, at: &str| wb.sheet(0).unwrap().value(c(at));
    assert_eq!(
        [v(&wb, "A1"), v(&wb, "A2"), v(&wb, "B1"), v(&wb, "B3"), v(&wb, "C1"), v(&wb, "D1"), v(&wb, "E1")],
        [
            Value::Number(99.0),
            Value::Number(100.0),
            Value::Number(1.0),
            Value::Number(3.0),
            Value::Number(6.0),
            Value::Number(2.0),
            Value::Error(CellError::Name)
        ]
    );
    // An edit recalculates the unknown functions (they may read anything): #NAME?.
    wb.sheet_mut(0).unwrap().cells.set(c("Z1"), Cell::value(Value::Number(1.0)));
    calc.cells_changed(&mut wb, &[(0, c("Z1"))]);
    let name = Value::Error(CellError::Name);
    assert_eq!([v(&wb, "A1"), v(&wb, "A2"), v(&wb, "B1"), v(&wb, "B2")], [name.clone(), name.clone(), name, Value::Empty]);
    let mut wb = loaded();
    calc.recalc_all(&mut wb);
    assert_eq!(v(&wb, "A1"), Value::Error(CellError::Name));
}

#[test]
fn a_loaded_workbook_keeps_cached_values_while_the_host_is_pending() {
    let mut t = T::new();
    t.wb = loaded();
    t.calc.recalc_loaded(&mut t.wb);
    assert_eq!((t.get("A1"), t.get("A2")), (Value::Number(99.0), Value::Number(100.0)));
    // The host computed what it could.
    assert_eq!((t.get("B1"), t.get("B3"), t.get("C1")), (Value::Number(1.0), Value::Number(3.0), Value::Number(6.0)));
    t.ready();
    assert_eq!((t.get("A1"), t.get("A2")), (Value::Number(101.5), Value::Number(102.5)));
}

#[test]
fn the_registry_is_shareable_across_threads() {
    fn shareable<T: Send + Sync>() {}
    shareable::<HostFunctionRegistry>();
    shareable::<Calc>();
    let _: Vec<Key> = Vec::new();
}
