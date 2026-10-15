//! Worksheet functions of the embedding application ("host functions", see
//! [`gridcraft_calc::host`]).
//!
//! The application registers them on the [`Session`] with a typed call, since a trait object
//! can't travel as command parameters. Everything after that is the usual flow: formulas call
//! them (`=CONTOSO.LOOKUP(A1)`), `formulas.functions` lists them, and
//! `formulas.refreshHostFunctions` calculates their formulas again when the application's data is
//! ready ([`Session::host_data_ready`] runs it).
//!
//! ```
//! use std::sync::Arc;
//! use gridcraft_engine::{HostArg, HostFunctionInfo, HostFunctions, HostResult, Session};
//!
//! struct Contoso;
//! impl HostFunctions for Contoso {
//!     fn namespace(&self) -> &str {
//!         "CONTOSO"
//!     }
//!     fn catalog(&self) -> Vec<HostFunctionInfo> {
//!         vec![HostFunctionInfo::new("DOUBLE", "Twice a number.").arg("number", "The number to double.")]
//!     }
//!     fn call(&self, _name: &str, args: &[HostArg]) -> HostResult {
//!         match args.first().and_then(HostArg::value).and_then(|v| v.as_f64()) {
//!             Some(n) => (2.0 * n).into(),
//!             None => gridcraft_engine::core::CellError::Value.into(),
//!         }
//!     }
//! }
//!
//! let mut s = Session::new();
//! s.register_host_functions(Arc::new(Contoso)).unwrap();
//! s.new_workbook();
//! s.execute("cell.set", serde_json::json!({"cell": "A1", "input": "=CONTOSO.DOUBLE(21)"})).unwrap();
//! let v = s.execute("cell.get", serde_json::json!({"cell": "A1"})).unwrap();
//! assert_eq!(v["value"], 42.0);
//! ```

use std::sync::Arc;

use gridcraft_calc::{HostFunctionRegistry, HostFunctions};
use serde_json::json;

use crate::{DocState, EngineError, Result, Session};

impl Session {
    /// Registers worksheet functions of the embedding application under the host's namespace,
    /// replacing a host registered for that namespace before (which also re-reads its catalog),
    /// and calculates the formulas calling them in every open workbook. Returns the namespace,
    /// upper case. Documents opened later have them from the start.
    pub fn register_host_functions(&mut self, host: Arc<dyn HostFunctions>) -> Result<String> {
        let mut hosts = (*self.hosts).clone();
        let namespace = hosts.register(host).map_err(|e| EngineError::Other(format!("can't register host functions: {e}")))?;
        self.set_hosts(Arc::new(hosts), &namespace);
        Ok(namespace)
    }

    /// Removes the host registered for `namespace` (ignoring case): its formulas become `#NAME?`,
    /// like any unknown function. `false` when no host was registered for it.
    pub fn unregister_host_functions(&mut self, namespace: &str) -> bool {
        let mut hosts = (*self.hosts).clone();
        if !hosts.unregister(namespace) {
            return false;
        }
        self.set_hosts(Arc::new(hosts), namespace);
        true
    }

    /// The registered host functions.
    pub fn host_functions(&self) -> &Arc<HostFunctionRegistry> {
        &self.hosts
    }

    /// The application's data for `namespace` is ready: runs `formulas.refreshHostFunctions`,
    /// which calculates the formulas calling its functions again, with their dependents, in every
    /// open workbook (formulas whose result was pending now get it). Returns how many formulas
    /// were calculated. Each report calls every function of the namespace that formulas use
    /// again, so a host whose data arrives in many small pieces reports once per batch (for
    /// example once per frame of the application's event loop), not once per piece.
    pub fn host_data_ready(&mut self, namespace: &str) -> Result<usize> {
        let r = self.execute("formulas.refreshHostFunctions", json!({"namespace": namespace}))?;
        Ok(usize::try_from(r["cells"].as_u64().unwrap_or(0)).unwrap_or(usize::MAX))
    }

    fn set_hosts(&mut self, hosts: Arc<HostFunctionRegistry>, namespace: &str) {
        self.hosts = hosts;
        for d in &mut self.docs {
            install(d, &self.hosts, Some(namespace));
        }
    }
}

/// Gives a document the session's host functions and calculates the formulas calling functions
/// of `namespace` (`None`: of every namespace registered before or now). The graph is rebuilt:
/// whether a call is a non-volatile host function decides what it depends on.
///
/// The typed calls that get here don't run through [`Session::execute`], so this keeps its
/// last-resort guard: a panic escaping the recalculation leaves the workbook as it was.
pub(crate) fn install(d: &mut DocState, hosts: &Arc<HostFunctionRegistry>, namespace: Option<&str>) {
    let before = d.wb.clone();
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| install_unguarded(d, hosts, namespace))).is_err() {
        d.wb = before;
        d.calc.set_host_functions(Arc::clone(hosts));
        d.calc.rebuild(&d.wb);
    }
}

fn install_unguarded(d: &mut DocState, hosts: &Arc<HostFunctionRegistry>, namespace: Option<&str>) {
    let gone: Vec<String> = d.calc.host_functions().namespaces().filter(|ns| !hosts.has_namespace(ns)).map(str::to_string).collect();
    d.calc.set_host_functions(Arc::clone(hosts));
    d.calc.rebuild(&d.wb);
    let mut wb = (*d.wb).clone();
    let mut n = d.calc.recalc_host_calls(&mut wb, namespace);
    if namespace.is_none() {
        for ns in &gone {
            n = n.saturating_add(d.calc.recalc_host_calls(&mut wb, Some(ns)));
        }
    }
    if n > 0 {
        d.wb = Arc::new(wb);
    }
}

/// `formulas.refreshHostFunctions`: recalculates the formulas calling functions of `namespace`
/// (of every registered namespace: `None`), and their dependents, in every open document.
/// Returns the number of formulas calculated.
pub(crate) fn refresh(s: &mut Session, namespace: Option<&str>) -> usize {
    let mut total = 0usize;
    for d in &mut s.docs {
        let mut wb = (*d.wb).clone();
        let n = d.calc.recalc_host_calls(&mut wb, namespace);
        if n > 0 {
            d.wb = Arc::new(wb);
            total = total.saturating_add(n);
        }
    }
    total
}

/// After undo or redo: the restored workbook holds the host results of its time, which may have
/// been pending then (caught by `IFERROR` or not), so formulas calling registered host functions
/// ask the host again.
pub(crate) fn recalc_restored(d: &mut DocState) {
    if d.calc.host_functions().is_empty() {
        return;
    }
    let mut wb = (*d.wb).clone();
    if d.calc.recalc_host_calls(&mut wb, None) > 0 {
        d.wb = Arc::new(wb);
    }
}
