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
    export_helpers, inline_functions, insert_fn_arg_args, prune_functions, resolve_fns_to_vars,
};
use crate::mir::interpreter::Interpreter;
use crate::mir::type_check::type_check;
use crate::parser::file_cache::FileCache;
use crate::parser::span::Span;
use ariadne::{ColorGenerator, Label, Report, ReportKind};
use slotmap::{SlotMap, new_key_type};
use std::borrow::Cow;
use std::collections::HashMap;

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

    // TODO: Add a pass to mangle functions (both for size optimization and to rename overloaded functions).

    // TODO: Add a pass to mangle variables (locals and statics), both for size optimization and to
    //       fix invalid variable names.

    export_helpers(ctx);

    // Helper functions are now exported when needed.

    prune_functions(ctx);

    // Non-export functions are now removed.

    true
}

new_key_type! {
    pub struct MIRConstKey;
    pub struct MIRStaticKey;
    pub struct MIRFunctionKey;
}

/// A declaration (base-level statement).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MIRDeclarationKey {
    Constant(MIRConstKey),
    Static(MIRStaticKey),
    Function(MIRFunctionKey),
}

/// An entire program.
/// Every name must be unique
/// among all named items contained
/// inside here.
#[derive(Debug, Default, Clone)]
pub struct MIRProgram<'a> {
    /// All the declarations in the program, in order.
    pub decls: Vec<MIRDeclarationKey>,

    /// A list of constants in the program.
    pub constants: SlotMap<MIRConstKey, MIRConstant<'a>>,

    /// A list of statics in the program.
    /// Name -> Static data.
    pub statics: SlotMap<MIRStaticKey, MIRStatic<'a>>,

    /// A list of functions in the program.
    /// Name -> Args -> Function data.
    pub functions: SlotMap<MIRFunctionKey, MIRFunction<'a>>,

    /// Name -> Constant.
    pub const_names: HashMap<Cow<'a, str>, MIRConstKey>,

    /// Name -> Static.
    pub static_names: HashMap<Cow<'a, str>, MIRStaticKey>,

    /// (Name, Args) -> Function.
    pub function_names: HashMap<Cow<'a, str>, HashMap<MIRFunctionArgs<'a>, MIRFunctionKey>>,
}

impl<'a> MIRContext<'a> {
    /// Tries to rename a declaration.
    /// On failure, this leaves the state unchanged and returns false.
    ///
    /// This doesn't modify the usage site of this declaration, so just
    /// calling this by itself will leave the program in an invalid state.
    pub fn try_rename(&mut self, key: MIRDeclarationKey, name: Cow<'a, str>) -> bool {
        if self.program.const_names.contains_key(&name)
            || self.program.static_names.contains_key(&name)
            // Functions can overload if there isn't already an exact match for the args.
            || (!matches!(key, MIRDeclarationKey::Function(_))
                && self.program.function_names.contains_key(&name))
        {
            return false;
        }

        match key {
            MIRDeclarationKey::Constant(key) => {
                self.program
                    .const_names
                    .remove(&self.program.constants[key].name);
                self.program.constants[key].name = name.clone();
                self.program.const_names.insert(name, key);
            }
            MIRDeclarationKey::Static(key) => {
                self.program
                    .static_names
                    .remove(&self.program.statics[key].name);
                self.program.statics[key].name = name.clone();
                self.program.static_names.insert(name, key);
            }
            MIRDeclarationKey::Function(key) => {
                let args_ty = self.program.functions[key].args_ty.clone();
                if self
                    .program
                    .function_names
                    .get(&name)
                    .is_some_and(|v| v.contains_key(&args_ty))
                {
                    // (name, arg) combo already exists.
                    return false;
                }

                self.program
                    .function_names
                    .get_mut(&self.program.functions[key].name)
                    .unwrap()
                    .remove(&args_ty);
                self.program.functions[key].name = name.clone();
                self.program
                    .function_names
                    .entry(name)
                    .or_default()
                    .insert(args_ty, key);
            }
        }

        true
    }

    /// Removes all declarations that don't match the given predicate.
    pub fn retain<T: FnMut(&Self, &MIRDeclarationKey) -> bool>(&mut self, mut func: T) {
        for to_remove in self
            .program
            .decls
            .iter()
            .filter(|v| !func(self, v))
            .cloned()
            .collect::<Vec<_>>()
        {
            match to_remove {
                MIRDeclarationKey::Constant(key) => {
                    let name = &self.program.constants[key].name;

                    self.program.const_names.remove(name);
                    self.program.constants.remove(key);
                    self.program
                        .decls
                        .retain(|val| val != &MIRDeclarationKey::Constant(key));
                }
                MIRDeclarationKey::Static(key) => {
                    let name = &self.program.statics[key].name;

                    self.program.static_names.remove(name);
                    self.program.statics.remove(key);
                    self.program
                        .decls
                        .retain(|val| val != &MIRDeclarationKey::Static(key));
                }
                MIRDeclarationKey::Function(key) => {
                    let name = &self.program.functions[key].name;
                    let args_ty = &self.program.functions[key].args_ty;

                    if let Some(inner) = self.program.function_names.get_mut(name) {
                        inner.remove(args_ty);
                    }
                    self.program.functions.remove(key);
                    self.program
                        .decls
                        .retain(|val| val != &MIRDeclarationKey::Function(key));
                }
            }
        }
    }

    /// Checks if the given declaration is already in use.
    /// If so, returns false and prints an error to the user.
    ///
    /// If the input is a function, args should be specified.
    /// If it's a const/static, None should be given instead.
    fn check_no_duplicates(
        &self,
        name: &str,
        args: Option<&MIRFunctionArgs<'a>>,
        span: &Span<'a>,
    ) -> bool {
        let defined_span;
        if let Some(var) = self.program.static_names.get(name) {
            defined_span = self.program.statics[*var].span.clone();
        } else if let Some(var) = self.program.const_names.get(name) {
            defined_span = self.program.constants[*var].span.clone();
        } else if let Some(args) = args {
            // We're trying to define a new function, and it hasn't conflicted
            // with anything else yet, as per the checks above.

            let mut found_duplicate = None;

            for option in self
                .program
                .function_names
                .get(name)
                .into_iter()
                .flat_map(|v| v.values())
            {
                let option = &self.program.functions[*option];

                if &option.args_ty == args {
                    found_duplicate = Some(option.span.clone());
                    break;
                }
            }

            if let Some(found_duplicate) = found_duplicate {
                // Two conflicting functions (same name, same args).
                defined_span = found_duplicate;
            } else {
                // This is just an overload, which is allowed.
                return true;
            }
        } else if let Some(option) = self
            .program
            .function_names
            .get(name)
            .and_then(|v| v.values().next())
        {
            // We aren't trying to define a function, but a function with the same name already exists.
            defined_span = self.program.functions[*option].span.clone();
        } else {
            // No duplicates.
            return true;
        }

        let mut colors = ColorGenerator::new();

        let prev = colors.next();
        let cur = colors.next();

        Report::build(ReportKind::Error, span.clone())
            .with_message("Duplicate identifier".to_string())
            .with_label(
                Label::new(defined_span)
                    .with_message(format!("Item with name {name} previously defined here"))
                    .with_color(prev),
            )
            .with_label(
                Label::new(span.clone())
                    .with_message("Redeclaration here".to_string())
                    .with_color(cur),
            )
            .finish()
            .eprint(self.file_cache.clone())
            .unwrap();

        false
    }

    /// Registers a declaration in the program.
    /// This doesn't add it to the list of declarations ([decls]),
    /// which is the caller's responsibility.
    /// It registers it with all other data structures.
    ///
    /// If the name of the declaration already exists, then this shows
    /// an error to the user and returns None. None should be treated as failure,
    /// and compilation aborted.
    pub fn register(&mut self, decl: MIRDeclaration<'a>) -> Option<MIRDeclarationKey> {
        let no_duplicates = match &decl {
            MIRDeclaration::Constant(const_) => {
                self.check_no_duplicates(&const_.name, None, &const_.span)
            }
            MIRDeclaration::Static(static_) => {
                self.check_no_duplicates(&static_.name, None, &static_.span)
            }
            MIRDeclaration::Function(func) => {
                self.check_no_duplicates(&func.name, Some(&func.args_ty), &func.span)
            }
        };
        if !no_duplicates {
            // Error has already been printed.
            return None;
        }

        match decl {
            MIRDeclaration::Constant(const_) => {
                let name = const_.name.clone();
                let key = self.program.constants.insert(const_);

                self.program.const_names.insert(name, key);
                Some(MIRDeclarationKey::Constant(key))
            }
            MIRDeclaration::Static(static_) => {
                let name = static_.name.clone();
                let key = self.program.statics.insert(static_);

                self.program.static_names.insert(name, key);
                Some(MIRDeclarationKey::Static(key))
            }
            MIRDeclaration::Function(func) => {
                let name = func.name.clone();
                let args_ty = func.args_ty.clone();
                let key = self.program.functions.insert(func);

                self.program
                    .function_names
                    .entry(name)
                    .or_default()
                    .insert(args_ty, key);
                Some(MIRDeclarationKey::Function(key))
            }
        }
    }
}

/// A declaration (base-level statement).
#[derive(Debug, Clone)]
pub enum MIRDeclaration<'a> {
    Constant(MIRConstant<'a>),
    Static(MIRStatic<'a>),
    Function(MIRFunction<'a>),
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

    /// The types for each of the function's arguments.
    /// These will always be fully resolved and can be
    /// used alongside its name to uniquely identify this
    /// function.
    pub args_ty: MIRFunctionArgs<'a>,

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
        /// Is a place expression that resolves
        /// to the variable.
        place: MIRExpression<'a>,

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
    /// Using 128-bit is less efficient but lets us
    /// get away with not specializing this to the
    /// number type (e.g., u64 and i64 both fit within it).
    Number(i128),

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

    /// Reference (address-of).
    Ref(Box<MIRExpression<'a>>),

    /// Dereference.
    Deref(Box<MIRExpression<'a>>),

    /// Member access (a.b).
    Member(Box<MIRExpression<'a>>, Cow<'a, str>),

    /// Index access (a[b]).
    Index(Box<MIRExpression<'a>>, Box<MIRExpression<'a>>),
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
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum MIRTypeInner<'a> {
    /// A currently unresolved number.
    /// This is eliminated during type checking, and defaults
    /// to i32 if there's any ambiguity.
    UnknownNumber,

    /// Signed 32-bit integer.
    I32,

    /// Unsigned 32-bit integer.
    U32,

    /// Boolean value.
    Bool,

    /// Unit type (void).
    Unit,

    /// String (language-dependent).
    String,

    /// A function pointer, args -> return value.
    FunctionPtr(MIRFunctionArgs<'a>, Box<MIRTypeInner<'a>>),

    /// A named type (struct).
    Named(Cow<'a, str>),
}

/// The type signature of a function, which along with its name, uniquely identifies it.
/// These types are always fully resolved.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MIRFunctionArgs<'a>(pub Vec<MIRTypeInner<'a>>);

impl<'a> From<MIRTypeInner<'a>> for Cow<'a, str> {
    fn from(value: MIRTypeInner<'a>) -> Self {
        match value {
            MIRTypeInner::UnknownNumber => Cow::Borrowed("number"),
            MIRTypeInner::I32 => Cow::Borrowed("i32"),
            MIRTypeInner::U32 => Cow::Borrowed("u32"),
            MIRTypeInner::Unit => Cow::Borrowed("()"),
            MIRTypeInner::Bool => Cow::Borrowed("bool"),
            MIRTypeInner::String => Cow::Borrowed("string"),
            MIRTypeInner::FunctionPtr(args, ret) => Cow::Owned(format!(
                "fn({}) -> {}",
                args.0
                    .iter()
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

    /// The function's arguments' types.
    /// Available after type checking.
    pub args_ty: Option<MIRFunctionArgs<'a>>,

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
