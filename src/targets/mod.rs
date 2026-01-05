use crate::codegen::Codegen;
use crate::codegen::c::CLowerer;
use crate::codegen::token::TokenInfo;

/// Contains information about a target language.
pub trait Target {
    /// The type used to lower from MIR to the target language.
    type Lowerer: Codegen + TokenInfo;

    /// Gets an instance of the lowerer for this target.
    fn lowerer(&self) -> Self::Lowerer;
}

pub struct C;

impl Target for C {
    type Lowerer = CLowerer;

    fn lowerer(&self) -> Self::Lowerer {
        CLowerer
    }
}
