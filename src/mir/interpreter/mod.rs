use crate::mir::function::{insert_fn_arg_args, resolve_fns_to_vars};
use crate::mir::interpreter::if_statement::flatten_ifs;
use crate::mir::interpreter::label::label_to_index;
use crate::mir::interpreter::loop_statement::flatten_loops;
use crate::mir::type_check::type_check;
use crate::mir::{MIRContext, MIRExpression, MIRExpressionInner, MIRFnSource, MIRStatement};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

mod if_statement;
mod label;
mod loop_statement;

/// This is used to evaluate functions/expressions at compile-time.
pub struct Interpreter<'a> {
    ctx: MIRContext<'a>,
    constants: RefCell<HashMap<Cow<'a, str>, InterpreterData<'a>>>,
    statics: RefCell<HashMap<Cow<'a, str>, InterpreterData<'a>>>,
    /// Tracks static / constant evaluations to detect a cycle.
    /// We can't do the same for functions, since recursion can be
    /// useful there.
    current_evals: RefCell<HashSet<Cow<'a, str>>>,
}

/// The underlying data contained behind a variable / return value.
#[derive(Debug, Clone)]
pub enum InterpreterData<'a> {
    U32(u32),
    Bool(bool),
    Unit(()),
    String(Cow<'a, str>),
    FunctionPtr(Cow<'a, str>),
}

/// Contains information about the function currently being called
/// and is updated by statements as they are executed.
/// It's safe to use a default here for expressions
/// outside a function (e.g., consts).
#[derive(Default)]
pub struct InterpreterScope<'a> {
    /// The variables currently in scope.
    variables: HashMap<Cow<'a, str>, Option<InterpreterData<'a>>>,

    /// The index of the next statement to execute.
    next_idx: Option<usize>,
}

impl<'a> Interpreter<'a> {
    /// Creates a new interpreter.
    /// This applies certain MIR passes to allow
    /// the interpreter to function.
    ///
    /// This should be called after type checking. In particular,
    /// the following passes must have run:
    /// - Insert function arguments as phantom variables.
    /// - Resolve function calls to variables.
    /// - Type check.
    ///
    /// Returns Err if one of the passes failed, indicating
    /// a compile error.
    pub fn new(mut ctx: MIRContext<'a>) -> Result<Self, ()> {
        insert_fn_arg_args(&mut ctx);

        // Args now exist as phantom variables.
        // This is needed for type checking.

        resolve_fns_to_vars(&mut ctx);

        // Functions now have correct indirect/direct markers.

        if !type_check(&mut ctx) {
            return Err(());
        }

        // Type information now exists.

        flatten_loops(&mut ctx);

        // Loops no longer exist
        // in MIR.

        // This needs to happen after
        // scope drop is added because
        // it erases scope.
        flatten_ifs(&mut ctx);

        // If statements no longer exist
        // in MIR.

        // The MIR is now fully flattened, so we can
        // use one index to refer to any statement in a function.

        // This needs to happen after all
        // operations that modify the order of MIR
        // and create labels/gotos.
        label_to_index(&mut ctx);

        // Gotos can now easily jump to labels.

        Ok(Self {
            ctx,
            constants: RefCell::new(HashMap::new()),
            statics: RefCell::new(HashMap::new()),
            current_evals: RefCell::new(HashSet::new()),
        })
    }

    /// Evaluates a constant.
    /// If an error occurs, returns None.
    pub fn eval_const(&self, name: &Cow<'a, str>) -> Result<InterpreterData<'a>, ()> {
        if let Some(const_data) = self.constants.borrow().get(name) {
            return Ok(const_data.clone());
        }

        let Some(constant) = self.ctx.program.constants.get(name) else {
            panic!("Constant not found!");
        };

        {
            let mut current_evals = self.current_evals.borrow_mut();
            if current_evals.contains(name) {
                eprintln!("Constant/static loop detected: {current_evals:?}");
                return Err(());
            }
            current_evals.insert(name.clone());
        }

        let expr_data = self.eval_expr(&constant.value, &InterpreterScope::default())?;

        self.constants
            .borrow_mut()
            .insert(name.clone(), expr_data.clone());

        self.current_evals.borrow_mut().remove(name);

        Ok(expr_data)
    }

    /// Evaluates a static.
    /// If an error occurs, returns None.
    pub fn eval_static(&self, name: &Cow<'a, str>) -> Result<InterpreterData<'a>, ()> {
        if let Some(static_data) = self.statics.borrow().get(name) {
            return Ok(static_data.clone());
        }

        let Some(static_def) = self.ctx.program.statics.get(name) else {
            panic!("Static not found!");
        };

        {
            let mut current_evals = self.current_evals.borrow_mut();
            if current_evals.contains(name) {
                eprintln!("Constant/static loop detected: {current_evals:?}");
                return Err(());
            }
            current_evals.insert(name.clone());
        }

        let expr_data = self.eval_expr(&static_def.value, &InterpreterScope::default())?;

        self.statics
            .borrow_mut()
            .insert(name.clone(), expr_data.clone());

        self.current_evals.borrow_mut().remove(name);

        Ok(expr_data)
    }

    /// Evaluates an expression.
    /// This expression MUST exist inside this interpreter's context.
    /// If an error occurs, returns None.
    pub fn eval_expr(
        &self,
        expr: &MIRExpression<'a>,
        scope: &InterpreterScope<'a>,
    ) -> Result<InterpreterData<'a>, ()> {
        macro_rules! simple_binary {
            ($left:expr, $right:expr, $($red_i:path > $red_o:path)|+, $op:tt, $ret:path) => {{
                use InterpreterData::*;

                // None here means an error.
                let left = self.eval_expr($left, scope)?;
                let right = self.eval_expr($right, scope)?;

                $(if let $red_i(left, ..) = &left {
                    if let $red_i(right, ..) = &right {
                        return Ok($red_o(*left $op *right));
                    }
                })+

                panic!("Failed to reduce binary expression: {:?} {:?}", left, right);
            }};
        }

        match &expr.inner {
            MIRExpressionInner::Add(left, right) => {
                simple_binary!(left, right, U32 > U32, +, Add)
            }
            MIRExpressionInner::Sub(left, right) => {
                simple_binary!(left, right, U32 > U32, -, Sub)
            }
            MIRExpressionInner::Mul(left, right) => {
                simple_binary!(left, right, U32 > U32, *, Mul)
            }
            MIRExpressionInner::Div(left, right) => {
                simple_binary!(left, right, U32 > U32, /, Div)
            }
            MIRExpressionInner::Equal(left, right) => {
                simple_binary!(left, right, U32 > Bool | Bool > Bool | Unit > Bool | String > Bool | FunctionPtr > Bool, ==, Equal)
            }
            MIRExpressionInner::NotEqual(left, right) => {
                simple_binary!(left, right, U32 > Bool | Bool > Bool | Unit > Bool | String > Bool | FunctionPtr > Bool, !=, NotEqual)
            }
            MIRExpressionInner::Greater(left, right) => {
                simple_binary!(left, right, U32 > Bool | Bool > Bool, >, Greater)
            }
            MIRExpressionInner::Less(left, right) => {
                simple_binary!(left, right, U32 > Bool | Bool > Bool, <, Less)
            }
            MIRExpressionInner::GreaterEq(left, right) => {
                simple_binary!(left, right, U32 > Bool | Bool > Bool, >=, GreaterEq)
            }
            MIRExpressionInner::LessEq(left, right) => {
                simple_binary!(left, right, U32 > Bool | Bool > Bool, <=, LessEq)
            }
            MIRExpressionInner::BoolAnd(left, right) => {
                simple_binary!(left, right, Bool > Bool, &&, BoolAnd)
            }
            MIRExpressionInner::BoolOr(left, right) => {
                simple_binary!(left, right, Bool > Bool, ||, BoolOr)
            }
            // TODO: Handle negatives.
            MIRExpressionInner::Number(val) => Ok(InterpreterData::U32(*val as u32)),
            MIRExpressionInner::String(val) => Ok(InterpreterData::String(val.clone())),
            MIRExpressionInner::Bool(val) => Ok(InterpreterData::Bool(*val)),
            MIRExpressionInner::Unit => Ok(InterpreterData::Unit(())),
            MIRExpressionInner::Variable(name) => {
                if let Some(data) = scope.variables.get(name) {
                    return Ok(data.as_ref().expect("Variable has not been set!").clone());
                };

                if self.ctx.program.constants.contains_key(name) {
                    return self.eval_const(name);
                }

                if self.ctx.program.statics.contains_key(name) {
                    return self.eval_static(name);
                }

                panic!("Variable not found in scope!");
            }
            MIRExpressionInner::FunctionCall(fn_data) => match &fn_data.source {
                MIRFnSource::Direct(fn_name, ..) => self.eval_function(
                    &fn_name,
                    fn_data
                        .args
                        .iter()
                        .map(|arg| self.eval_expr(arg, scope))
                        .collect::<Result<Vec<_>, ()>>()?,
                ),
                MIRFnSource::Indirect(fn_name) => {
                    let InterpreterData::FunctionPtr(source) = self.eval_expr(&fn_name, scope)?
                    else {
                        panic!("Wrong indirect function call type!");
                    };

                    self.eval_function(
                        &source,
                        fn_data
                            .args
                            .iter()
                            .map(|arg| self.eval_expr(arg, scope))
                            .collect::<Result<Vec<_>, ()>>()?,
                    )
                }
            },
        }
    }

    /// Evaluates a function call.
    /// If an error occurs, returns Err.
    pub fn eval_function(
        &self,
        fn_name: &Cow<'a, str>,
        args: Vec<InterpreterData<'a>>,
    ) -> Result<InterpreterData<'a>, ()> {
        let Some(fn_data) = self.ctx.program.functions.get(fn_name) else {
            panic!("Function not found!");
        };
        assert_eq!(fn_data.args.len(), args.len());

        let mut scope = InterpreterScope::default();
        for (data, arg) in args.into_iter().zip(fn_data.args.iter()) {
            scope.variables.insert(arg.name.clone(), Some(data));
        }

        let mut ret: Option<InterpreterData<'a>> = None;

        let mut index = 0;
        while index < fn_data.body.len() {
            let statement = &fn_data.body[index];

            if let Some(data) = self.eval_statement(statement, &mut scope)? {
                // We returned.
                ret = Some(data);
                break;
            }

            if let Some(next_idx) = scope.next_idx {
                index = next_idx;
                scope.next_idx = None;

                continue;
            }

            index += 1;
        }

        Ok(ret.unwrap_or(InterpreterData::Unit(())))
    }

    /// Evaluates a statement.
    /// If an error occurs, returns Err.
    /// If the statement returns, returns Some(data).
    /// Otherwise None.
    pub fn eval_statement(
        &self,
        statement: &MIRStatement<'a>,
        scope: &mut InterpreterScope<'a>,
    ) -> Result<Option<InterpreterData<'a>>, ()> {
        match statement {
            MIRStatement::CreateVariable {
                var, value, arg, ..
            } => {
                // Phantom variables for args aren't needed here.
                if *arg {
                    return Ok(None);
                }

                let value = match value {
                    Some(value) => Some(self.eval_expr(value, scope)?),
                    None => None,
                };

                scope.variables.insert(var.name.clone(), value);
            }
            MIRStatement::SetVariable { name, value, .. } => {
                let value = Some(self.eval_expr(value, scope)?);

                scope.variables.insert(name.clone(), value);
            }
            MIRStatement::DropVariable(_, _) => {
                // Dropping has no effect on the interpreter.
            }
            MIRStatement::FunctionCall(fn_data) => match &fn_data.source {
                MIRFnSource::Direct(fn_name, ..) => {
                    self.eval_function(
                        fn_name,
                        fn_data
                            .args
                            .iter()
                            .map(|arg| self.eval_expr(arg, scope))
                            .collect::<Result<Vec<_>, ()>>()?,
                    )?;
                }
                MIRFnSource::Indirect(fn_name) => {
                    let InterpreterData::FunctionPtr(source) = self.eval_expr(&fn_name, scope)?
                    else {
                        panic!("Wrong indirect function call type!");
                    };

                    self.eval_function(
                        &source,
                        fn_data
                            .args
                            .iter()
                            .map(|arg| self.eval_expr(arg, scope))
                            .collect::<Result<Vec<_>, ()>>()?,
                    )?;
                }
            },
            MIRStatement::Return { expr, .. } => {
                let Some(expr) = expr else {
                    return Ok(Some(InterpreterData::Unit(())));
                };

                return Ok(Some(self.eval_expr(expr, scope)?));
            }
            MIRStatement::Label { .. } => {
                // Labels are handled by pre-run passes.
            }
            MIRStatement::Goto { index, .. } => {
                scope.next_idx = Some(index.expect("Goto statement has no index!"));
            }
            MIRStatement::GotoNotEqual {
                index, condition, ..
            } => {
                let InterpreterData::Bool(condition) = self.eval_expr(condition, scope)? else {
                    panic!("GotoNotEqual condition is not a boolean!");
                };

                if !condition {
                    scope.next_idx = Some(index.expect("GotoNotEqual statement has no index!"));
                }
            }
            MIRStatement::IfStatement { .. }
            | MIRStatement::LoopStatement { .. }
            | MIRStatement::ContinueStatement { .. }
            | MIRStatement::BreakStatement { .. } => {
                panic!("This statement type cannot exist at this phase!");
            }
        }

        Ok(None)
    }
}

impl<'a> From<InterpreterData<'a>> for MIRExpressionInner<'a> {
    fn from(value: InterpreterData<'a>) -> Self {
        match value {
            // TODO: Handle negative numbers.
            InterpreterData::U32(v) => MIRExpressionInner::Number(v as i64),
            InterpreterData::Bool(v) => MIRExpressionInner::Bool(v),
            InterpreterData::Unit(_) => todo!("Add a unit expression to support this"),
            InterpreterData::String(v) => MIRExpressionInner::String(v),
            InterpreterData::FunctionPtr(v) => MIRExpressionInner::Variable(v),
        }
    }
}
