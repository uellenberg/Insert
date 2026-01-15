use crate::codegen;
use crate::codegen::Codegen;

/// Contains information about a target language.
pub trait Target {
    /// Gets an instance of the lowerer for this target.
    fn lowerer(&self) -> &'static dyn Codegen;

    /// The name of this target.
    /// This is used as the specified in target blocks
    /// and the CLI, so shouldn't change.
    fn name(&self) -> &'static str;
}

pub struct C;

impl Target for C {
    fn lowerer(&self) -> &'static dyn Codegen {
        codegen::c::C
    }

    fn name(&self) -> &'static str {
        "C"
    }
}
