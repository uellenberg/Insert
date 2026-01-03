#![feature(box_patterns)]
#![feature(iter_intersperse)]
extern crate core;

use crate::codegen::LowerOptions;
use crate::codegen::c::mir_to_c;
use crate::mir::{MIRContext, visit_mir};
use crate::parser::parse_file;

mod codegen;
mod mir;
mod parser;

fn main() {
    let mut mir_ctx = MIRContext::default();

    if !parse_file("./test/recursive.int".as_ref(), &mut mir_ctx) {
        return;
    }

    println!("{}", mir_ctx.program);

    if !visit_mir(&mut mir_ctx) {
        return;
    }

    println!("{:#}", mir_ctx.program);

    let lower_options = LowerOptions { fancy: true };

    let c = mir_to_c(&mir_ctx.program, lower_options);
    println!("{c}");
}
