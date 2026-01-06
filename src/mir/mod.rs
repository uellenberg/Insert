mod display;
mod drop;
mod expr;
mod function;
mod interpreter;
mod scope;
mod type_check;

use crate::mir::drop::drop_at_scope_end;
use crate::mir::expr::{const_eval, const_optimize_expr};
use crate::mir::function::{
    export_functions, export_helpers, inline_functions, insert_fn_arg_args, resolve_fns_to_vars,
};
use crate::mir::interpreter::Interpreter;
use crate::mir::type_check::type_check;
use crate::parser::file_cache::FileCache;
use crate::parser::span::Span;
use indexmap::IndexMap;
use std::borrow::Cow;

/// Context that can be used
/// throughout the MIR processing.
#[derive(Debug, Default, Clone)]
pub struct MIRContext<'a> {
    /// The current program.
    pub program: MIRProgram<'a>,

    /// A cache of files that have been loaded.
    pub file_cache: FileCache,
}

/// Applies all MIR phases and
/// optimizations, returning
/// whether it was successful.
pub fn visit_mir(ctx: &mut MIRContext<'_>) -> bool {
    insert_fn_arg_args(ctx);

    // Args now exist as phantom variables.

    resolve_fns_to_vars(ctx);

    // Functions now have correct indirect/direct markers.

    if !type_check(ctx) {
        return false;
    }

    // Type information now exists.

    if !inline_functions(ctx) {
        return false;
    }

    // The interpreter runs in its own scope, to avoid messing
    // with our MIR.
    // It applies some passes which can't be easily translated back.
    let Ok(mut interpreter) = Interpreter::new(ctx.clone()) else {
        return false;
    };

    if !const_eval(ctx, &mut interpreter) {
        return false;
    }

    // Constants are now only literals.

    if !const_optimize_expr(ctx) {
        return false;
    }

    // Expressions no longer contain references
    // to constants.

    drop_at_scope_end(ctx);

    // All variables are now dropped, including
    // arg variables.

    // TODO: Add a pass to remove unit variables (probably part of SSA -> function scope var generation).

    export_helpers(ctx);

    // Helper functions are now exported when needed.

    export_functions(ctx);

    // Non-export functions are now removed.

    true
}

/// An entire program.
/// Every name must be unique
/// among all named items contained
/// inside here.
#[derive(Debug, Default, Clone)]
pub struct MIRProgram<'a> {
    /// A list of constants in the program.
    /// Name -> Constant data.
    pub constants: IndexMap<Cow<'a, str>, MIRConstant<'a>>,

    /// A list of statics in the program.
    /// Name -> Static data.
    pub statics: IndexMap<Cow<'a, str>, MIRStatic<'a>>,

    /// A list of functions in the program.
    /// Name -> Function data.
    pub functions: IndexMap<Cow<'a, str>, MIRFunction<'a>>,
}

/// A constant variable.
/// These cannot be modified, and can only
/// be initialized with simple expressions.
#[derive(Debug, Clone)]
pub struct MIRConstant<'a> {
    /// The variable's name.
    pub name: Cow<'a, str>,

    /// The constant's type.
    pub ty: MIRType<'a>,

    /// The constant's value.
    pub value: MIRExpression<'a>,

    /// The code that created
    /// this item.
    pub span: Span<'a>,
}

/// A static variable.
/// These can be modified and can only
/// be initialized with simple expressions.
#[derive(Debug, Clone)]
pub struct MIRStatic<'a> {
    /// The variable's name.
    pub name: Cow<'a, str>,

    /// The constant's type.
    pub ty: MIRType<'a>,

    /// The constant's value.
    pub value: MIRExpression<'a>,

    /// The code that created
    /// this item.
    pub span: Span<'a>,
}

/// The function's type (how it should be used and emitted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MIRFunctionType {
    /// Exported to the target code.
    /// The default for all functions.
    Export,

    /// Directly inlined at the call site.
    Inline,

    /// Exported only if called.
    Helper,
}

/// A function.
#[derive(Debug, Clone)]
pub struct MIRFunction<'a> {
    /// The function's name.
    pub name: Cow<'a, str>,

    /// The function's type (how it should be used and emitted).
    pub fn_type: MIRFunctionType,

    /// The function's return type.
    pub ret_ty: MIRType<'a>,

    /// A list of the arguments that
    /// the function takes in.
    pub args: Vec<MIRVariable<'a>>,

    /// A list of statements
    /// that will be executed
    /// when the function runs.
    pub body: Vec<MIRStatement<'a>>,

    /// The code that created
    /// this item.
    pub span: Span<'a>,
}

/// A variable inside a function.
#[derive(Debug, Clone)]
pub struct MIRVariable<'a> {
    /// The variable's name.
    pub name: Cow<'a, str>,

    /// The type of the data stored
    /// inside the variable.
    pub ty: MIRType<'a>,

    /// The code that created
    /// this item.
    pub span: Span<'a>,
}

/// A statement inside a function's
/// body.
#[derive(Debug, Clone)]
pub enum MIRStatement<'a> {
    /// Creates a new variable.
    CreateVariable {
        /// The variable to create.
        var: MIRVariable<'a>,

        /// An optional initial value.
        value: Option<MIRExpression<'a>>,

        /// This is used for function arguments,
        /// to allow them to be analyzed the same
        /// way as normal variables.
        /// Arg variables aren't lowered to IR.
        arg: bool,

        /// The code that created
        /// this item.
        span: Span<'a>,
    },

    /// Drops the value stored
    /// inside a variable and
    /// invalidates it.
    DropVariable(Cow<'a, str>, Span<'a>),

    /// Sets a variable to a certain value.
    SetVariable {
        /// Is the variable's name.
        name: Cow<'a, str>,

        /// Is the expression to set it to.
        value: MIRExpression<'a>,

        /// The code that created
        /// this item.
        span: Span<'a>,
    },

    /// Calls a function, ignoring its return value.
    FunctionCall(MIRFnCall<'a>),

    /// Exits the function with an optional
    /// value.
    Return {
        /// The value to return (if it exists).
        expr: Option<MIRExpression<'a>>,

        /// The code that created
        /// this item.
        span: Span<'a>,
    },

    /// A label that can be jumped to.
    Label {
        /// The label's name.
        name: Cow<'a, str>,

        /// The code that created
        /// this item.
        span: Span<'a>,
    },

    /// Jumps to the specified label.
    Goto {
        /// The name of the label to jump to.
        name: Cow<'a, str>,

        /// The goto label's index in MIR.
        /// This is added in late passes of the interpreter.
        index: Option<usize>,

        /// The code that created
        /// this item.
        span: Span<'a>,
    },

    /// Jumps to the specified label if the
    /// given condition is not true.
    GotoNotEqual {
        /// The name of the label to jump to.
        name: Cow<'a, str>,

        /// The goto label's index in MIR.
        /// This is added in late passes of the interpreter.
        index: Option<usize>,

        /// The condition to check.
        /// If it's false, we'll jump.
        condition: MIRExpression<'a>,

        /// The code that created
        /// this item.
        span: Span<'a>,
    },

    /// An if statement.
    IfStatement {
        /// The if statement's condition.
        condition: MIRExpression<'a>,

        /// Code that runs on the true case.
        on_true: Vec<MIRStatement<'a>>,

        /// Code that runs on the false case.
        on_false: Vec<MIRStatement<'a>>,

        /// The code that created
        /// this item.
        span: Span<'a>,
    },

    /// An infinite loop.
    LoopStatement {
        /// Code that runs inside the loop.
        body: Vec<MIRStatement<'a>>,

        /// The code that created
        /// this item.
        span: Span<'a>,
    },

    /// Goes to the top of the parent
    /// loop.
    ContinueStatement {
        /// The code that created
        /// this item.
        span: Span<'a>,
    },

    /// Immediately exits the parent loop.
    BreakStatement {
        /// The code that created
        /// this item.
        span: Span<'a>,
    },
}

/// An expression that evaluates to some
/// value.
#[derive(Debug, Clone)]
pub struct MIRExpression<'a> {
    /// The expression.
    pub inner: MIRExpressionInner<'a>,

    /// The expression's type.
    /// This is only available
    /// after type checking and inference.
    pub ty: Option<MIRType<'a>>,

    /// The expression's span.
    pub span: Span<'a>,
}

/// An expression that evaluates to some
/// value.
#[derive(Debug, Clone)]
pub enum MIRExpressionInner<'a> {
    /// Addition.
    Add(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),

    /// Subtraction.
    Sub(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),

    /// Multiplication.
    Mul(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),

    /// Division.
    Div(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),

    /// Equals.
    Equal(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),

    /// Not equal.
    NotEqual(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),

    /// Less than.
    Less(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),

    /// Greater than.
    Greater(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),

    /// Less than or equal.
    LessEq(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),

    /// Greater than or equal.
    GreaterEq(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),

    /// Logical (boolean) and.
    BoolAnd(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),

    /// Logical (boolean) or.
    BoolOr(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),

    /// Number literal.
    Number(i64),

    /// String literal (language-dependent).
    String(Cow<'a, str>),

    /// Bool literal.
    Bool(bool),

    /// Unit literal.
    Unit,

    /// Variable access.
    Variable(Cow<'a, str>),

    /// Function call (using return value).
    FunctionCall(Box<MIRFnCall<'a>>),
}

/// A type written out as text.
#[derive(Debug, Clone)]
pub struct MIRType<'a> {
    /// The type represented by the literal.
    pub ty: MIRTypeInner<'a>,

    /// The literal's span.
    /// This type is sometimes
    /// inferred.
    /// In that case, it will be
    /// placed at the inference site,
    /// if possible, or else None.
    pub span: Option<Span<'a>>,
}

/// The type of data a variable represents.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum MIRTypeInner<'a> {
    /// Unsigned 32-bit integer.
    U32,

    /// Boolean value.
    Bool,

    /// Unit type (void).
    Unit,

    /// String (language-dependent).
    String,

    /// A function pointer, args -> return value.
    FunctionPtr(Vec<MIRTypeInner<'a>>, Box<MIRTypeInner<'a>>),

    /// A named type (struct).
    Named(Cow<'a, str>),
}

impl<'a> From<MIRTypeInner<'a>> for Cow<'a, str> {
    fn from(value: MIRTypeInner<'a>) -> Self {
        match value {
            MIRTypeInner::U32 => Cow::Borrowed("u32"),
            MIRTypeInner::Unit => Cow::Borrowed("()"),
            MIRTypeInner::Bool => Cow::Borrowed("bool"),
            MIRTypeInner::String => Cow::Borrowed("string"),
            MIRTypeInner::FunctionPtr(args, ret) => Cow::Owned(format!(
                "fn({}) -> {}",
                args.iter()
                    .cloned()
                    .map(|v| v.into())
                    .intersperse(Cow::Borrowed(", "))
                    .collect::<String>(),
                ret
            )),
            MIRTypeInner::Named(val) => val,
        }
    }
}

/// A function call.
#[derive(Debug, Clone)]
pub struct MIRFnCall<'a> {
    /// The source for the function (name or ptr).
    pub source: MIRFnSource<'a>,

    /// The function's arguments.
    pub args: Vec<MIRExpression<'a>>,

    /// The function's return type,
    /// if known at the time.
    pub ret_ty: Option<MIRType<'a>>,

    /// A span representing the entire function call.
    pub span: Span<'a>,
}

/// The source for a function pointer
/// when performing a function call.
#[derive(Debug, Clone)]
pub enum MIRFnSource<'a> {
    /// A direct function call, containing
    /// the name of the function to call.
    Direct(Cow<'a, str>, Span<'a>),

    /// An indirect function call, meaning
    /// the function pointer is stored in
    /// a variable.
    /// Contains the name of the variable
    /// storing the pointer.
    Indirect(MIRExpression<'a>),
}
