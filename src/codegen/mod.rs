pub mod c;
pub mod token;

use crate::mir::{
    MIRExpression, MIRExpressionInner, MIRFnSource, MIRFunction, MIRProgram, MIRStatement,
    MIRStatic, MIRType,
};
use std::borrow::Cow;
use token::Tokens;

/// Options passed to the lowering process, controlling
/// how the output should be formatted.
#[derive(Clone, Debug, Default)]
pub struct LowerOptions {
    /// Should fancy tokens be included in the output?
    pub fancy: bool,
}

/// A trait for lowering MIR to a target language.
///
/// Implementors of this trait provide the logic to convert MIR constructs
/// into tokens for a specific target language (e.g., C).
pub trait Codegen {
    /// The writer info type used during code generation.
    /// This typically holds state like indentation level.
    /// It's always safe to create a default value of this
    /// for heuristics, but not for exact output tokens.
    type Writer: Default + Copy;

    /// Converts a program from MIR to the target language.
    fn lower_program(&self, program: &MIRProgram, options: LowerOptions) -> String;

    /// Converts a function from MIR to the target language.
    fn lower_function<'a>(&self, func: &MIRFunction<'a>, info: Self::Writer) -> Tokens<'a>;

    /// Converts a block of statements from MIR to the target language.
    fn lower_block<'a>(&self, block: &[MIRStatement<'a>], info: Self::Writer) -> Tokens<'a>;

    /// Converts a statement from MIR to the target language.
    /// Returns None if the statement should be ignored (e.g., analysis-only statements).
    fn lower_statement<'a>(
        &self,
        stmt: &MIRStatement<'a>,
        info: Self::Writer,
    ) -> Option<Tokens<'a>>;

    /// Converts a static variable from MIR to the target language.
    fn lower_static<'a>(&self, val: &MIRStatic<'a>, info: Self::Writer) -> Tokens<'a>;

    /// Converts an expression from MIR to the target language.
    fn lower_expression<'a>(&self, expr: &MIRExpression<'a>, info: Self::Writer) -> Tokens<'a>;

    /// Lowers a function source into a callable form.
    fn lower_fn_source<'a>(&self, src: &MIRFnSource<'a>, info: Self::Writer) -> Tokens<'a>;

    /// Converts a datatype from MIR to the target language.
    /// Returns (prefix, postfix) where a variable can be constructed as:
    /// {PREFIX} name{POSTFIX}.
    fn lower_datatype<'a>(&self, ty: &MIRType<'a>, info: Self::Writer) -> (Tokens<'a>, Tokens<'a>);

    /// Adds a datatype to a variable/function name.
    fn decorate_with_type<'a>(
        &self,
        name: Cow<'a, str>,
        ty: &MIRType<'a>,
        info: Self::Writer,
    ) -> Tokens<'a>;

    /// Lowers a child expression and correctly wraps it in parentheses if needed.
    fn lower_wrap_expression<'a>(
        &self,
        expr: &MIRExpression<'a>,
        outer: &MIRExpression<'a>,
        info: Self::Writer,
    ) -> Tokens<'a>;

    /// Returns the precedence of an operator, or None if precedence
    /// doesn't apply to it (e.g., variable / literals).
    ///
    /// Given outer(a, inner(b, c)), inner must be wrapped if its precedence
    /// number is higher than outer's. If inner/outer has no precedence, then it
    /// needs no wrapping.
    fn precedence(&self, op: &MIRExpressionInner) -> Option<usize>;
}
