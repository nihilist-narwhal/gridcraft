//! User-defined functions (an add-in's, or the embedding application's `CONTOSO.LOOKUP`): their
//! formulas keep their text, are marked as array formulas (they may return arrays), and cache the
//! last values, spilled ones included, so other applications show the numbers.

use gridcraft_core::{CellError, CellRef, RangeRef, Value};
use gridcraft_model::{Cell, Formula, Workbook};

use crate::package::Package;
use crate::xml::El;
use crate::{read_xlsx, write_xlsx};

fn at(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

fn cell<'a>(sheet: &'a El, address: &str) -> &'a El {
    sheet.child("sheetData").unwrap().kids("row").flat_map(|r| r.kids("c")).find(|c| c.attr("r") == Some(address)).unwrap()
}

fn formula(text: &str, value: Value) -> Cell {
    let mut cell = Cell::formula(Formula::new(text));
    cell.value = value;
    cell
}

/// Host formulas as the engine leaves them after calculating: a scalar, an array spilled below
/// its anchor, a pending one, and one read back from another application with `_xludf.`.
fn book() -> Workbook {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.cells.set(at("A1"), Cell::value(Value::Number(2.0)));
    sh.cells.set(at("B1"), formula("CONTOSO.ADD(A1,3)", Value::Number(5.0)));
    sh.cells.set(at("C1"), formula("contoso.seq(3)", Value::Number(1.0)));
    sh.spill_ranges.insert(at("C1"), RangeRef::parse("C1:C3").unwrap());
    sh.spill.insert(at("C2"), Value::Number(2.0));
    sh.spill.insert(at("C3"), Value::Number(3.0));
    sh.cells.set(at("D1"), formula("CONTOSO.QUOTE(\"ABC\")", Value::Error(CellError::GettingData)));
    sh.cells.set(at("E1"), formula("_xludf.CONTOSO.STATS.MEAN(A1:A2)", Value::text("mean")));
    sh.cells.set(at("F1"), formula("NORM.DIST(0,0,1,TRUE)", Value::Number(0.5)));
    wb
}

#[test]
fn host_formulas_are_written_as_user_defined_array_formulas_with_cached_values() {
    let bytes = write_xlsx(&book()).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();
    let f = |a: &str| cell(&sheet, a).child("f").unwrap().clone();
    let v = |a: &str| cell(&sheet, a).child("v").map(|v| v.text.clone());
    // Plain names, as files store user-defined functions; no `_xlfn.` on a namespace.
    assert_eq!(f("B1").text, "CONTOSO.ADD(A1,3)");
    assert_eq!(f("C1").text, "CONTOSO.SEQ(3)");
    assert_eq!(f("E1").text, "_xludf.CONTOSO.STATS.MEAN(A1:A2)");
    assert_eq!(f("F1").text, "_xlfn.NORM.DIST(0,0,1,TRUE)");
    // Array formulas with dynamic-array metadata, so Excel neither adds `@` nor drops the spill.
    for (a, r) in [("B1", "B1"), ("C1", "C1:C3"), ("D1", "D1"), ("E1", "E1")] {
        assert_eq!(cell(&sheet, a).attr("cm"), Some("1"), "{a}");
        assert_eq!((f(a).attr("t"), f(a).attr("ref")), (Some("array"), Some(r)), "{a}");
    }
    assert_eq!(cell(&sheet, "F1").attr("cm"), None, "a built-in returning one value stays plain");
    // The last values are cached: spilled ones as plain values, a pending one as #GETTING_DATA.
    assert_eq!(
        [v("B1"), v("C1"), v("C2"), v("C3"), v("E1")],
        [Some("5"), Some("1"), Some("2"), Some("3"), Some("mean")].map(|x| x.map(String::from))
    );
    assert_eq!((cell(&sheet, "D1").attr("t"), v("D1").as_deref()), (Some("e"), Some("#GETTING_DATA")));
}

#[test]
fn host_formulas_read_back_with_their_text_and_cached_values() {
    let (back, report) = read_xlsx(&write_xlsx(&book()).unwrap()).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let sh = back.sheet(0).unwrap();
    let text = |a: &str| sh.cell(at(a)).and_then(|c| c.formula.as_ref()).map(|f| (f.text.clone(), f.array));
    assert_eq!(text("B1"), Some(("CONTOSO.ADD(A1,3)".into(), None)));
    assert_eq!(text("C1"), Some(("CONTOSO.SEQ(3)".into(), None)), "a dynamic array, not a legacy one");
    assert_eq!(text("E1"), Some(("_xludf.CONTOSO.STATS.MEAN(A1:A2)".into(), None)));
    // Cached values come back, the spilled ones as the anchor's spill (cells stay free for it).
    assert_eq!(
        [sh.value(at("B1")), sh.value(at("C1")), sh.value(at("C2")), sh.value(at("C3")), sh.value(at("D1"))],
        [Value::Number(5.0), Value::Number(1.0), Value::Number(2.0), Value::Number(3.0), Value::Error(CellError::GettingData)]
    );
    assert!(sh.cell(at("C2")).is_none());
    assert_eq!(sh.spill_ranges.get(&at("C1")), Some(&RangeRef::parse("C1:C3").unwrap()));
    // Saved again unchanged.
    let again = read_xlsx(&write_xlsx(&back).unwrap()).unwrap().0;
    let sh2 = again.sheet(0).unwrap();
    for a in ["B1", "C1", "C2", "C3", "D1", "E1"] {
        assert_eq!(sh2.value(at(a)), sh.value(at(a)), "{a}");
    }
}

#[test]
fn cached_spills_from_other_applications_stay_with_their_anchor() {
    // A file another application wrote: a user-defined function with the `_xludf.` prefix, a
    // spill with a blank inside it, and a malformed `ref` that doesn't start at its cell (its
    // values are not taken as a spill).
    let sheet = r#"<sheetData>
        <row r="1"><c r="A1" cm="1"><f t="array" ref="A1:A3">_xludf.CONTOSO.SEQ(3)</f><v>10</v></c><c r="B1" cm="1"><f t="array" ref="C5:C6">CONTOSO.X()</f><v>1</v></c></row>
        <row r="2"><c r="A2"><v>20</v></c></row>
        <row r="5"><c r="C5"><v>7</v></c></row>
        <row r="6"><c r="C6"><v>8</v></c></row>
    </sheetData>"#;
    let (wb, _) = read_xlsx(&super::minimal(sheet, &[], "", "")).unwrap();
    let sh = wb.sheet(0).unwrap();
    assert_eq!(sh.cell(at("A1")).unwrap().formula.as_ref().unwrap().text, "_xludf.CONTOSO.SEQ(3)");
    assert_eq!(sh.cell(at("B1")).unwrap().formula.as_ref().unwrap().text, "CONTOSO.X()");
    assert_eq!(sh.spill_ranges.get(&at("A1")), Some(&RangeRef::parse("A1:A3").unwrap()));
    assert_eq!((sh.value(at("A2")), sh.value(at("A3"))), (Value::Number(20.0), Value::Empty));
    assert!(!sh.spill_ranges.contains_key(&at("B1")));
    assert!(!sh.spill.contains_key(&at("C5")) && sh.value(at("C6")) == Value::Empty);
}
