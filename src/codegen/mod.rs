pub mod c;
pub mod token;

use crate::codegen::token::TokenInfo;
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
pub trait Codegen: TokenInfo {
    /// Creates a new boxed instance of this codegen.
    fn new(&self) -> Box<dyn Codegen>;

    /// Converts a program from MIR to the target language.
    fn lower_program(&mut self, program: &MIRProgram, options: LowerOptions) -> String;

    /// Converts a function from MIR to the target language.
    fn lower_function<'a>(&mut self, func: &MIRFunction<'a>) -> Tokens<'a>;

    /// Converts a block of statements from MIR to the target language.
    fn lower_block<'a>(&mut self, block: &[MIRStatement<'a>]) -> Tokens<'a>;

    /// Converts a statement from MIR to the target language.
    /// Returns None if the statement should be ignored (e.g., analysis-only statements).
    fn lower_statement<'a>(&mut self, stmt: &MIRStatement<'a>) -> Option<Tokens<'a>>;

    /// Converts a static variable from MIR to the target language.
    fn lower_static<'a>(&mut self, val: &MIRStatic<'a>) -> Tokens<'a>;

    /// Converts an expression from MIR to the target language.
    fn lower_expression<'a>(&mut self, expr: &MIRExpression<'a>) -> Tokens<'a>;

    /// Lowers a function source into a callable form.
    fn lower_fn_source<'a>(&mut self, src: &MIRFnSource<'a>) -> Tokens<'a>;

    /// Converts a datatype from MIR to the target language.
    /// Returns (prefix, postfix) where a variable can be constructed as:
    /// {PREFIX} name{POSTFIX}.
    fn lower_datatype<'a>(&mut self, ty: &MIRType<'a>) -> (Tokens<'a>, Tokens<'a>);

    /// Adds a datatype to a variable/function name.
    fn decorate_with_type<'a>(&mut self, name: Cow<'a, str>, ty: &MIRType<'a>) -> Tokens<'a>;

    /// Lowers a child expression and correctly wraps it in parentheses if needed.
    fn lower_wrap_expression<'a>(
        &mut self,
        expr: &MIRExpression<'a>,
        outer: &MIRExpression<'a>,
    ) -> Tokens<'a>;

    /// Returns the precedence of an operator, or None if precedence
    /// doesn't apply to it (e.g., variable / literals).
    ///
    /// Given outer(a, inner(b, c)), inner must be wrapped if its precedence
    /// number is higher than outer's. If inner/outer has no precedence, then it
    /// needs no wrapping.
    fn precedence(&self, op: &MIRExpressionInner) -> Option<usize>;

    /// Lowers required imports to the target language.
    fn lower_imports<'a>(&mut self, imports: &[Cow<'a, str>]) -> Tokens<'a>;
}
