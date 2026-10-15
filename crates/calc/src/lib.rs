//! GridCraft's calculation engine: the evaluator (references, names, tables, LET/LAMBDA,
//! dynamic arrays, functions the embedding application provides) and dependency-driven
//! recalculation with spills and cycle detection.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod eval;
pub mod host;
pub mod recalc;
mod trimrange;

pub use eval::{Area, SPECIAL_FUNCTIONS, is_known_function};
pub use host::{HostArg, HostArgInfo, HostFunction, HostFunctionError, HostFunctionInfo, HostFunctionRegistry, HostFunctions, HostResult};
pub use recalc::{Calc, Key, evaluate, now_serial};

#[cfg(test)]
mod host_tests;
#[cfg(test)]
mod tests;
