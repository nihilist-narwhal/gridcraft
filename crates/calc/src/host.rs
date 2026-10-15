//! Worksheet functions provided by the application that embeds GridCraft ("host functions").
//!
//! An embedding application implements [`HostFunctions`] for a namespace of its own and registers
//! it (in the engine, `Session::register_host_functions`). Formulas call its functions with
//! dotted names in the style of add-in functions: `=CONTOSO.LOOKUP(A1,"price")` calls `LOOKUP` of
//! the host registered for the namespace `CONTOSO`.
//!
//! A function name resolves, in order, to a built-in function (a namespace never shadows one:
//! `NORM.DIST` stays the built-in even with a host registered for `NORM`), to a function of the
//! host registered for the name's first segment, to a defined name holding a LAMBDA; anything
//! else is `#NAME?`, as for any unknown function. The file-format prefixes for user-defined and
//! add-in functions (`_xludf.`, `_xll.`) are ignored when resolving, so a workbook that another
//! application saved as `_xludf.CONTOSO.LOOKUP(…)` still finds the host.
//!
//! Calls are synchronous and must return promptly. A host that needs time (a request to a
//! service, a long computation) returns [`HostResult::Pending`], which the cell shows as
//! `#GETTING_DATA`, and reports when the data is there (`Session::host_data_ready`): the formulas
//! that call its namespace, and what depends on them, are calculated again. Nothing waits or
//! polls, so the same host works on wasm and on native.
//!
//! What a host returns is treated as hostile input (see [`HostFunction::call`]): numbers that are
//! not finite become `#NUM!`, text longer than a cell holds and malformed or oversized arrays
//! become errors, arrays nested in arrays become `#CALC!` (and are dropped without recursion
//! however deep they nest), and a host that panics gives `#VALUE!`.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use gridcraft_core::{Array, CellError, CellRef, Value};

/// Largest array a host function may return (the limit of the dynamic-array functions); a larger
/// one is `#NUM!`.
pub const MAX_HOST_ARRAY_CELLS: usize = 10_000_000;
/// Longest text a host function may return, in characters (what a cell holds); longer is `#VALUE!`.
pub const MAX_HOST_TEXT: usize = 32_767;
/// Longest namespace, in characters.
pub const MAX_NAMESPACE_LEN: usize = 64;
/// Longest full function name (`NAMESPACE.NAME`), in characters: the limit of names in formulas.
pub const MAX_FUNCTION_NAME_LEN: usize = 255;
/// Most functions one host may declare.
pub const MAX_HOST_FUNCTIONS: usize = 10_000;

/// One argument as the host receives it.
#[derive(Clone, Debug, PartialEq)]
pub enum HostArg {
    /// Left out: the middle argument of `CONTOSO.F(1,,3)`.
    Omitted,
    /// The argument's value. A reference is read: one cell gives its value (`Value::Empty` for a
    /// blank cell), a block gives a `Value::Array`, row-major. A multi-area reference is
    /// `#VALUE!`.
    Value(Value),
}

impl HostArg {
    /// The value, `None` when the argument was left out.
    pub fn value(&self) -> Option<&Value> {
        match self {
            HostArg::Omitted => None,
            HostArg::Value(v) => Some(v),
        }
    }
}

/// What a host function returns.
#[derive(Clone, Debug, PartialEq)]
pub enum HostResult {
    /// One value. A `Value::Array` is taken as [`HostResult::Array`].
    Value(Value),
    /// A 2-D array, row-major: it spills from the formula's cell like a dynamic array, and is
    /// `#SPILL!` when something is in the way.
    Array(Array),
    /// Not ready: the cell shows `#GETTING_DATA` until the host reports the namespace's data
    /// ready, which calls the function again.
    Pending,
    /// An error value (`#N/A`, `#VALUE!`…).
    Error(CellError),
}

impl From<Value> for HostResult {
    fn from(v: Value) -> Self {
        HostResult::Value(v)
    }
}

impl From<f64> for HostResult {
    fn from(v: f64) -> Self {
        HostResult::Value(Value::Number(v))
    }
}

impl From<&str> for HostResult {
    fn from(v: &str) -> Self {
        HostResult::Value(Value::text(v))
    }
}

impl From<CellError> for HostResult {
    fn from(e: CellError) -> Self {
        HostResult::Error(e)
    }
}

/// An argument of a host function, for autocomplete, the function catalog and Insert Function.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostArgInfo {
    pub name: String,
    pub description: String,
    pub optional: bool,
}

/// A function a host declares.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostFunctionInfo {
    /// The name within the namespace (`LOOKUP` for `CONTOSO.LOOKUP`): ASCII letters, digits and
    /// `_`, in segments separated by `.`. Formulas match it ignoring case; [`HostFunctions::call`]
    /// receives it spelled as declared here.
    pub name: String,
    pub description: String,
    /// The arguments, for display. The engine passes whatever arguments a formula gives; the host
    /// checks them (and returns `#VALUE!` for a wrong count, as built-in functions do).
    pub args: Vec<HostArgInfo>,
    /// Calculated again on every recalculation, like `RAND` and `NOW`. Otherwise a call is
    /// calculated again when a cell its arguments read changes, when the host reports new data
    /// for its namespace, and on a full recalculation (F9).
    pub volatile: bool,
}

impl HostFunctionInfo {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        HostFunctionInfo { name: name.into(), description: description.into(), args: Vec::new(), volatile: false }
    }
    /// Adds a required argument.
    pub fn arg(mut self, name: impl Into<String>, description: impl Into<String>) -> Self {
        self.args.push(HostArgInfo { name: name.into(), description: description.into(), optional: false });
        self
    }
    /// Adds an optional argument.
    pub fn optional_arg(mut self, name: impl Into<String>, description: impl Into<String>) -> Self {
        self.args.push(HostArgInfo { name: name.into(), description: description.into(), optional: true });
        self
    }
    /// Marks the function volatile.
    pub fn volatile(mut self) -> Self {
        self.volatile = true;
        self
    }
    /// The call as the function catalog shows it: `CONTOSO.LOOKUP(key, [table])`.
    pub fn signature(&self, namespace: &str) -> String {
        let args: Vec<String> = self.args.iter().map(|a| if a.optional { format!("[{}]", a.name) } else { a.name.clone() }).collect();
        format!("{}.{}({})", namespace.to_ascii_uppercase(), self.name.to_ascii_uppercase(), args.join(", "))
    }
}

/// Worksheet functions an embedding application provides under its own namespace.
///
/// Implementations are shared between documents (and possibly threads), so they take `&self`;
/// a host that keeps state (a cache of fetched data) uses interior mutability.
pub trait HostFunctions: Send + Sync {
    /// The namespace: formulas call `NAMESPACE.NAME(…)`. ASCII letters, digits and `_`, starting
    /// with a letter, at most [`MAX_NAMESPACE_LEN`] characters; matched ignoring case.
    fn namespace(&self) -> &str;
    /// The functions the host provides. Read when the host is registered.
    fn catalog(&self) -> Vec<HostFunctionInfo>;
    /// Calculates `name` (as declared in the catalog) for `args`. Must return promptly: slow work
    /// returns [`HostResult::Pending`] and reports the data ready later.
    fn call(&self, name: &str, args: &[HostArg]) -> HostResult;
}

/// Why a host can't be registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostFunctionError {
    /// The namespace is empty, too long, has characters other than ASCII letters, digits and `_`,
    /// doesn't start with a letter, or reads as a cell reference.
    InvalidNamespace(String),
    /// A function name is empty, too long, or has characters other than ASCII letters, digits,
    /// `_` and separating dots.
    InvalidName(String),
    /// A function's full name is a built-in function's, which always wins.
    Builtin(String),
    /// Two functions have the same name (ignoring case).
    Duplicate(String),
    /// The catalog declares more than [`MAX_HOST_FUNCTIONS`] functions.
    TooMany(usize),
    /// The host panicked while giving its namespace or catalog.
    Panicked,
}

impl fmt::Display for HostFunctionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostFunctionError::InvalidNamespace(n) => {
                write!(f, "invalid namespace `{n}`: use ASCII letters, digits and _, starting with a letter (at most {MAX_NAMESPACE_LEN})")
            }
            HostFunctionError::InvalidName(n) => {
                write!(f, "invalid function name `{n}`: use ASCII letters, digits, _ and separating dots (at most {MAX_FUNCTION_NAME_LEN} in all)")
            }
            HostFunctionError::Builtin(n) => write!(f, "`{n}` is a built-in function"),
            HostFunctionError::Duplicate(n) => write!(f, "function `{n}` is declared twice"),
            HostFunctionError::TooMany(n) => write!(f, "{n} functions declared (at most {MAX_HOST_FUNCTIONS})"),
            HostFunctionError::Panicked => write!(f, "the host panicked while declaring its functions"),
        }
    }
}

impl std::error::Error for HostFunctionError {}

#[derive(Clone)]
struct Registered {
    /// Upper case.
    namespace: String,
    host: Arc<dyn HostFunctions>,
    /// Upper-case name within the namespace → its declaration.
    functions: BTreeMap<String, HostFunctionInfo>,
}

/// The hosts registered with a session, by namespace. Cheap to clone (hosts are shared).
#[derive(Clone, Default)]
pub struct HostFunctionRegistry {
    hosts: Vec<Registered>,
}

impl fmt::Debug for HostFunctionRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.hosts.iter().map(|h| (&h.namespace, h.functions.len()))).finish()
    }
}

/// A host function a name resolves to.
#[derive(Clone, Copy)]
pub struct HostFunction<'a> {
    /// The namespace, upper case.
    pub namespace: &'a str,
    pub info: &'a HostFunctionInfo,
    host: &'a Arc<dyn HostFunctions>,
}

impl fmt::Debug for HostFunction<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostFunction").field("namespace", &self.namespace).field("info", self.info).finish()
    }
}

impl HostFunction<'_> {
    /// `NAMESPACE.NAME`, upper case.
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.namespace, self.info.name.to_ascii_uppercase())
    }

    /// Calls the host and makes its result safe to store: `#GETTING_DATA` for a pending result,
    /// `#VALUE!` when the host panics, and the checks of [`sanitize`].
    pub fn call(&self, args: &[HostArg]) -> Value {
        let host = self.host;
        let name = self.info.name.as_str();
        // A misbehaving host must not take the workbook down: its panic becomes an error value
        // (on wasm, where panics abort, this is only as good as the host).
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| host.call(name, args))) {
            Ok(r) => sanitize(r),
            Err(_) => Value::Error(CellError::Value),
        }
    }
}

impl HostFunctionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// Registers `host` under its namespace, replacing a host registered for the same namespace
    /// (which also re-reads a changed catalog). Returns the namespace, upper case.
    pub fn register(&mut self, host: Arc<dyn HostFunctions>) -> Result<String, HostFunctionError> {
        // The host's own code: a panic in it refuses the host rather than unwinding through
        // the engine (on wasm, where panics abort, this is only as good as the host).
        let declared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (host.namespace().to_string(), host.catalog())));
        let Ok((namespace, catalog)) = declared else { return Err(HostFunctionError::Panicked) };
        if !valid_namespace(&namespace) {
            return Err(HostFunctionError::InvalidNamespace(namespace));
        }
        let namespace = namespace.to_ascii_uppercase();
        if catalog.len() > MAX_HOST_FUNCTIONS {
            return Err(HostFunctionError::TooMany(catalog.len()));
        }
        let mut functions = BTreeMap::new();
        for info in catalog {
            let upper = info.name.to_ascii_uppercase();
            let full = format!("{namespace}.{upper}");
            if !valid_function_name(&info.name) || full.chars().count() > MAX_FUNCTION_NAME_LEN {
                return Err(HostFunctionError::InvalidName(info.name));
            }
            if crate::eval::is_known_function(&full) {
                return Err(HostFunctionError::Builtin(full));
            }
            if functions.contains_key(&upper) {
                return Err(HostFunctionError::Duplicate(full));
            }
            functions.insert(upper, info);
        }
        let entry = Registered { namespace: namespace.clone(), host, functions };
        match self.hosts.iter_mut().find(|h| h.namespace == namespace) {
            Some(slot) => *slot = entry,
            None => self.hosts.push(entry),
        }
        Ok(namespace)
    }

    /// Removes the host registered for `namespace` (ignoring case). `false` when there was none.
    pub fn unregister(&mut self, namespace: &str) -> bool {
        let before = self.hosts.len();
        self.hosts.retain(|h| !h.namespace.eq_ignore_ascii_case(namespace));
        self.hosts.len() != before
    }

    /// The registered namespaces, upper case, in registration order.
    pub fn namespaces(&self) -> impl Iterator<Item = &str> {
        self.hosts.iter().map(|h| h.namespace.as_str())
    }

    /// Whether a host is registered for `namespace` (ignoring case).
    pub fn has_namespace(&self, namespace: &str) -> bool {
        self.hosts.iter().any(|h| h.namespace.eq_ignore_ascii_case(namespace))
    }

    /// Every registered function, by namespace then name.
    pub fn functions(&self) -> impl Iterator<Item = HostFunction<'_>> {
        self.hosts.iter().flat_map(|h| h.functions.values().map(move |info| HostFunction { namespace: &h.namespace, info, host: &h.host }))
    }

    /// The host function a formula's function name calls, if any. Built-in functions are not
    /// looked up here: callers try them first.
    pub fn lookup(&self, name: &str) -> Option<HostFunction<'_>> {
        let (namespace, local) = split_host_name(name)?;
        let h = self.hosts.iter().find(|h| h.namespace.eq_ignore_ascii_case(namespace))?;
        let info = h.functions.get(&local.to_ascii_uppercase())?;
        Some(HostFunction { namespace: &h.namespace, info, host: &h.host })
    }
}

/// The function name without the file-format prefixes of user-defined and add-in functions
/// (`_xludf.CONTOSO.LOOKUP` → `CONTOSO.LOOKUP`), any case.
pub fn strip_udf_prefix(name: &str) -> &str {
    for p in ["_xludf.", "_xll."] {
        if let Some(head) = name.get(..p.len())
            && head.eq_ignore_ascii_case(p)
        {
            return name.get(p.len()..).unwrap_or("");
        }
    }
    name
}

/// A function name as (namespace, name within it): `CONTOSO.LOOKUP` → (`CONTOSO`, `LOOKUP`),
/// `CONTOSO.STATS.MEAN` → (`CONTOSO`, `STATS.MEAN`). `None` without a dot or with an empty part.
/// Whether the name is a built-in function is not checked.
pub fn split_host_name(name: &str) -> Option<(&str, &str)> {
    let (namespace, local) = strip_udf_prefix(name).split_once('.')?;
    (!namespace.is_empty() && !local.is_empty()).then_some((namespace, local))
}

fn valid_namespace(n: &str) -> bool {
    let mut chars = n.chars();
    let Some(first) = chars.next() else { return false };
    first.is_ascii_alphabetic()
        && n.len() <= MAX_NAMESPACE_LEN
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        // `AB12.X(…)` would read as a cell followed by `.X`; R1C1-style names likewise.
        && CellRef::parse(n).is_none()
        && !looks_r1c1(n)
}

/// `R`, `C`, `RC`, `R1`, `C2`, `R1C1`: references in R1C1 notation, which a namespace can't be.
fn looks_r1c1(n: &str) -> bool {
    let u = n.to_ascii_uppercase();
    let rest = match u.strip_prefix('R') {
        Some(r) => r.trim_start_matches(|c: char| c.is_ascii_digit()),
        None => u.as_str(),
    };
    let rest = match rest.strip_prefix('C') {
        Some(r) => r.trim_start_matches(|c: char| c.is_ascii_digit()),
        None => rest,
    };
    rest.is_empty() && (u.starts_with('R') || u.starts_with('C'))
}

fn valid_function_name(n: &str) -> bool {
    !n.is_empty() && n.split('.').all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// A host's result as a value the engine can store: `#GETTING_DATA` while pending; numbers that
/// are NaN or infinite `#NUM!`; text longer than [`MAX_HOST_TEXT`] characters `#VALUE!`; the
/// internal circular-reference error `#VALUE!`; arrays whose size doesn't match their data
/// `#VALUE!`, larger than [`MAX_HOST_ARRAY_CELLS`] `#NUM!`; arrays inside arrays `#CALC!` (Excel's
/// nested-array error).
pub fn sanitize(r: HostResult) -> Value {
    match r {
        HostResult::Pending => Value::Error(CellError::GettingData),
        HostResult::Error(e) => error(e),
        HostResult::Value(Value::Array(a)) => match Arc::try_unwrap(a) {
            Ok(a) => array(a),
            // Shared with the host: check the shape before copying what is kept, so a malformed
            // or oversized array is never copied.
            Err(a) => match shape_error(a.rows, a.cols, a.data.len()) {
                Some(e) => Value::Error(e),
                None => array(Array { rows: a.rows, cols: a.cols, data: a.data.clone() }),
            },
        },
        HostResult::Value(v) => scalar(v),
        HostResult::Array(a) => array(a),
    }
}

fn error(e: CellError) -> Value {
    Value::Error(if e == CellError::Circ { CellError::Value } else { e })
}

fn scalar(v: Value) -> Value {
    match v {
        Value::Number(n) => Value::number(n),
        Value::Text(t) if t.len() > MAX_HOST_TEXT && t.chars().count() > MAX_HOST_TEXT => Value::Error(CellError::Value),
        Value::Error(e) => error(e),
        Value::Array(a) => {
            drop_deep(Value::Array(a));
            Value::Error(CellError::Calc)
        }
        v => v,
    }
}

/// Why an array of `rows` × `cols` holding `len` values can't be kept: `#VALUE!` when the shape
/// overflows, is empty or doesn't match the data, `#NUM!` when it is larger than
/// [`MAX_HOST_ARRAY_CELLS`].
fn shape_error(rows: usize, cols: usize, len: usize) -> Option<CellError> {
    match rows.checked_mul(cols) {
        None => Some(CellError::Value),
        Some(cells) if cells > MAX_HOST_ARRAY_CELLS => Some(CellError::Num),
        Some(cells) if cells == 0 || cells != len => Some(CellError::Value),
        Some(_) => None,
    }
}

fn array(a: Array) -> Value {
    let Array { rows, cols, data } = a;
    if let Some(e) = shape_error(rows, cols, data.len()) {
        drop_all(data);
        return Value::Error(e);
    }
    let data: Vec<Value> = data.into_iter().map(scalar).collect();
    Array::new(rows, cols, data).map(Value::from).unwrap_or(Value::Error(CellError::Value))
}

fn drop_all(data: Vec<Value>) {
    for v in data {
        drop_deep(v);
    }
}

/// Drops a value without recursing into nested arrays (a host can nest them deeper than the stack
/// allows a recursive drop to go).
fn drop_deep(v: Value) {
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        if let Value::Array(a) = v
            && let Ok(a) = Arc::try_unwrap(a)
        {
            stack.extend(a.data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(&'static str, Vec<HostFunctionInfo>);

    impl HostFunctions for Fake {
        fn namespace(&self) -> &str {
            self.0
        }
        fn catalog(&self) -> Vec<HostFunctionInfo> {
            self.1.clone()
        }
        fn call(&self, _: &str, _: &[HostArg]) -> HostResult {
            HostResult::Value(Value::Number(1.0))
        }
    }

    fn register(ns: &'static str, names: &[&str]) -> Result<String, HostFunctionError> {
        let host = Fake(ns, names.iter().map(|n| HostFunctionInfo::new(*n, "")).collect());
        HostFunctionRegistry::new().register(Arc::new(host))
    }

    #[test]
    fn namespaces_and_names_are_checked() {
        assert_eq!(register("Contoso", &["Lookup", "stats.mean"]), Ok("CONTOSO".into()));
        for bad in ["", "1X", "_X", "A-B", "AB12", "R1C1", "RC", "C", "CONTOSO.DATA", "É"] {
            assert!(matches!(register(bad, &["F"]), Err(HostFunctionError::InvalidNamespace(_))), "{bad}");
        }
        assert_eq!(register("X".repeat(65).leak(), &["F"]), Err(HostFunctionError::InvalidNamespace("X".repeat(65))));
        for bad in ["", "A..B", ".A", "A.", "A B", "A(", "Ü"] {
            assert!(matches!(register("CONTOSO", &[bad]), Err(HostFunctionError::InvalidName(_))), "{bad:?}");
        }
        assert!(matches!(register("CONTOSO", &["F".repeat(250).leak()]), Err(HostFunctionError::InvalidName(_))));
        // A namespace that prefixes built-ins keeps them built-in: declaring one is refused.
        assert_eq!(register("NORM", &["DIST"]), Err(HostFunctionError::Builtin("NORM.DIST".into())));
        assert_eq!(register("NORM", &["FIT"]), Ok("NORM".into()));
        assert_eq!(register("CONTOSO", &["F", "f"]), Err(HostFunctionError::Duplicate("CONTOSO.F".into())));
        let many: Vec<String> = (0..=MAX_HOST_FUNCTIONS).map(|i| format!("F{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(register("CONTOSO", &many), Err(HostFunctionError::TooMany(MAX_HOST_FUNCTIONS + 1)));
    }

    #[test]
    fn lookup_by_namespace_ignoring_case_and_udf_prefixes() {
        let mut reg = HostFunctionRegistry::new();
        reg.register(Arc::new(Fake("Contoso", vec![HostFunctionInfo::new("Lookup", "finds"), HostFunctionInfo::new("Stats.Mean", "")]))).unwrap();
        for name in ["CONTOSO.LOOKUP", "contoso.lookup", "_xludf.CONTOSO.LOOKUP", "_XLL.Contoso.Lookup"] {
            let f = reg.lookup(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!((f.namespace, f.info.name.as_str(), f.full_name()), ("CONTOSO", "Lookup", "CONTOSO.LOOKUP".to_string()));
        }
        assert_eq!(reg.lookup("CONTOSO.STATS.MEAN").map(|f| f.full_name()), Some("CONTOSO.STATS.MEAN".into()));
        for name in ["CONTOSO.NOPE", "OTHER.LOOKUP", "LOOKUP", "CONTOSO.", ".LOOKUP", "_xludf.LOOKUP"] {
            assert!(reg.lookup(name).is_none(), "{name}");
        }
        assert!(reg.has_namespace("contoso"));
        assert_eq!(reg.functions().count(), 2);
        // Registering again replaces; unregistering removes.
        reg.register(Arc::new(Fake("CONTOSO", vec![HostFunctionInfo::new("Other", "")]))).unwrap();
        assert!(reg.lookup("CONTOSO.LOOKUP").is_none() && reg.lookup("CONTOSO.OTHER").is_some());
        assert_eq!(reg.namespaces().collect::<Vec<_>>(), ["CONTOSO"]);
        assert!(reg.unregister("Contoso") && !reg.unregister("CONTOSO") && reg.is_empty());
    }

    #[test]
    fn signatures() {
        let f = HostFunctionInfo::new("Lookup", "").arg("key", "").optional_arg("table", "");
        assert_eq!(f.signature("contoso"), "CONTOSO.LOOKUP(key, [table])");
        assert!(HostFunctionInfo::new("Now", "").volatile().volatile);
    }

    #[test]
    fn hostile_results_are_made_safe() {
        let e = |e: CellError| Value::Error(e);
        assert_eq!(sanitize(HostResult::Pending), e(CellError::GettingData));
        assert_eq!(sanitize(f64::NAN.into()), e(CellError::Num));
        assert_eq!(sanitize(f64::INFINITY.into()), e(CellError::Num));
        assert_eq!(sanitize((-0.0).into()), Value::Number(0.0));
        assert_eq!(sanitize(HostResult::Error(CellError::Circ)), e(CellError::Value));
        assert_eq!(sanitize(HostResult::Error(CellError::NA)), e(CellError::NA));
        assert_eq!(sanitize(Value::text("x".repeat(MAX_HOST_TEXT)).into()), Value::text("x".repeat(MAX_HOST_TEXT)));
        assert_eq!(sanitize(Value::text("x".repeat(MAX_HOST_TEXT + 1)).into()), e(CellError::Value));
        // Shapes that don't match their data, overflow, or are too large.
        let arr = |rows, cols, data: Vec<Value>| HostResult::Array(Array { rows, cols, data });
        assert_eq!(sanitize(arr(2, 2, vec![Value::Number(1.0)])), e(CellError::Value));
        assert_eq!(sanitize(arr(0, 0, vec![])), e(CellError::Value));
        assert_eq!(sanitize(arr(usize::MAX, 2, vec![])), e(CellError::Value));
        assert_eq!(sanitize(arr(MAX_HOST_ARRAY_CELLS + 1, 1, vec![])), e(CellError::Num));
        assert_eq!(sanitize(arr(1, 1, vec![Value::Empty; 2])), e(CellError::Value));
        // Elements are checked like scalars; nested arrays are #CALC!.
        let nested = Value::from(Array::scalar(Value::Number(1.0)));
        let got = sanitize(arr(1, 3, vec![Value::Number(f64::NAN), nested.clone(), Value::Bool(true)]));
        assert_eq!(got, Value::from(Array::row(vec![e(CellError::Num), e(CellError::Calc), Value::Bool(true)])));
        // A `Value::Array` is an array result, even one the host keeps a handle on.
        assert_eq!(sanitize(HostResult::Value(nested.clone())), Value::from(Array::scalar(Value::Number(1.0))));
        let kept = nested.clone();
        assert_eq!(sanitize(HostResult::Value(nested)), kept);
        // A malformed or oversized array the host keeps a handle on is refused, not copied.
        let shared = |rows, cols, data: Vec<Value>| Value::Array(Arc::new(Array { rows, cols, data }));
        let held = shared(MAX_HOST_ARRAY_CELLS + 1, 1, vec![Value::Number(1.0)]);
        assert_eq!(sanitize(HostResult::Value(held.clone())), e(CellError::Num));
        let held = shared(2, 2, vec![Value::Number(1.0)]);
        assert_eq!(sanitize(HostResult::Value(held.clone())), e(CellError::Value));
    }

    #[test]
    fn a_host_that_panics_while_declaring_its_functions_is_refused() {
        struct Broken(bool);
        impl HostFunctions for Broken {
            fn namespace(&self) -> &str {
                if self.0 { panic!("no namespace") } else { "CONTOSO" }
            }
            fn catalog(&self) -> Vec<HostFunctionInfo> {
                panic!("no catalog")
            }
            fn call(&self, _: &str, _: &[HostArg]) -> HostResult {
                HostResult::Pending
            }
        }
        let mut reg = HostFunctionRegistry::new();
        for namespace_panics in [true, false] {
            assert_eq!(reg.register(Arc::new(Broken(namespace_panics))), Err(HostFunctionError::Panicked));
        }
        assert!(reg.is_empty());
    }

    #[test]
    fn deeply_nested_arrays_are_dropped_without_recursion() {
        // Far deeper than a recursive drop survives on a test thread's stack.
        let mut v = Value::Number(1.0);
        for _ in 0..1_000_000 {
            v = Value::from(Array::scalar(v));
        }
        assert_eq!(sanitize(HostResult::Array(Array::row(vec![v]))), Value::from(Array::scalar(Value::Error(CellError::Calc))));
    }
}
