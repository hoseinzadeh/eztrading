//! The SML grammar, shared with the compiler (`core/transpiler/src/eztrading.pest`).

use pest_derive::Parser;

#[derive(Parser)]
#[grammar = "../../transpiler/src/eztrading.pest"]
pub struct SmlParser;
