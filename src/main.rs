#![feature(box_patterns)]
#![feature(iter_intersperse)]

use crate::codegen::{Codegen, LowerOptions};
use crate::mir::{MIRContext, visit_mir};
use crate::parser::parse_file;
use crate::targets::Target;

mod codegen;
mod mir;
mod parser;
mod targets;

fn main() {
    let target = targets::C;

    let mut mir_ctx = MIRContext::default();

    if !parse_file("./test/const.int".as_ref(), &mut mir_ctx) {
        return;
    }

    println!("{}", mir_ctx.program);

    if !visit_mir(&mut mir_ctx) {
        return;
    }

    println!("{:#}", mir_ctx.program);

    let lower_options = LowerOptions { fancy: true };

    let c = target
        .lowerer()
        .lower_program(&mir_ctx.program, lower_options);
    println!("{c}");
}
