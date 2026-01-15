#![feature(box_patterns)]
#![feature(iter_intersperse)]

use crate::codegen::LowerOptions;
use crate::mir::{MIRContext, visit_mir};
use crate::parser::parse_file;
use crate::targets::Target;
use clap::Parser;
use std::env;
use std::path::PathBuf;

mod codegen;
mod mir;
mod parser;
mod targets;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The target language to compile to.
    #[clap(
        short,
        long,
        default_value = "C",
        value_parser = clap::builder::PossibleValuesParser::new(
            ["C"]
        ),
    )]
    target: String,

    /// Should the output be fancy (formatted with indents / newlines)?
    #[clap(short, long, default_value = "false")]
    fancy: bool,

    /// The input file to compile.
    input: String,
}

fn main() {
    let args = Args::parse();

    let lower_options = LowerOptions { fancy: args.fancy };
    let target: &'static dyn Target = match args.target.as_str() {
        "C" => &targets::C,
        _ => unreachable!(),
    };

    let input_path = env::current_dir()
        .map(|dir| dir.join(&args.input))
        .unwrap_or(PathBuf::from(&args.input));

    let mut mir_ctx = MIRContext {
        lowerer: target.lowerer().new(),
        target,
        program: Default::default(),
        file_cache: Default::default(),
    };

    if !parse_file(&input_path, &mut mir_ctx) {
        return;
    }

    println!("{}", mir_ctx.program);

    if !visit_mir(&mut mir_ctx) {
        return;
    }

    println!("{:#}", mir_ctx.program);

    let c = target
        .lowerer()
        .new()
        .lower_program(&mir_ctx.program, lower_options);
    println!("{c}");
}
