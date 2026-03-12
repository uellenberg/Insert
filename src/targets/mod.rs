use crate::codegen;
use crate::codegen::Codegen;
use std::fmt::Debug;

/// Contains information about a target language.
pub trait Target: Debug {
    /// Gets an instance of the lowerer for this target.
    fn lowerer(&self) -> &'static dyn Codegen;

    /// The name of this target.
    /// This is used as the specified in target blocks
    /// and the CLI, so shouldn't change.
    fn name(&self) -> &'static str;

    /// The name of the main function, if that exists for this target.
    fn main(&self) -> Option<&'static str>;

    /// Returns whether strings should be represented as arrays of characters.
    fn str_char_arr(&self) -> bool;

    /// Returns whether booleans should be represented as integers (i32).
    fn bool_as_i32(&self) -> bool;

    /// Whether the language supports C-style truthy coercion (i.e, 0 -> false, 1 -> true).
    fn truthy_coercion(&self) -> bool;

    /// Whether arrays are internally references (and can be dereferenced / have
    /// arithmetic performed on them).
    fn array_as_ref(&self) -> bool;
}

#[derive(Debug)]
pub struct C;

impl Target for C {
    fn lowerer(&self) -> &'static dyn Codegen {
        codegen::c::C
    }

    fn name(&self) -> &'static str {
        "C"
    }

    fn main(&self) -> Option<&'static str> {
        Some("main")
    }

    fn str_char_arr(&self) -> bool {
        true
    }

    fn bool_as_i32(&self) -> bool {
        true
    }

    fn truthy_coercion(&self) -> bool {
        true
    }

    fn array_as_ref(&self) -> bool {
        true
    }
}
