use crate::mir::function::{insert_fn_arg_args, resolve_fns_to_vars};
use crate::mir::interpreter::if_statement::flatten_ifs;
use crate::mir::interpreter::label::label_to_index;
use crate::mir::interpreter::loop_statement::flatten_loops;
use crate::mir::interpreter::native::{NativeFunctions, register_natives};
use crate::mir::type_check::type_check;
use crate::mir::{
    MIRContext, MIRExpression, MIRExpressionInner, MIRFnSource, MIRFunctionKey, MIRFunctionType,
    MIRStatement, MIRTypeInner,
};
use crate::parser::span::{Span, eprintln_span};
use num_traits::ToPrimitive;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

mod if_statement;
mod label;
mod loop_statement;
mod native;

/// This is used to evaluate functions/expressions at compile-time.
pub struct Interpreter<'a> {
    ctx: MIRContext<'a>,
    constants: RefCell<HashMap<Cow<'a, str>, InterpreterData<'a>>>,
    statics: RefCell<HashMap<Cow<'a, str>, VariableData<'a>>>,
    /// Tracks static / constant evaluations to detect a cycle.
    /// We can't do the same for functions, since recursion can be
    /// useful there.
    current_evals: RefCell<HashSet<Cow<'a, str>>>,
    /// Functions which are executed as native code when called in
    /// the interpreter.
    natives: NativeFunctions,
}

/// Data stored in a variable.
/// If this is None, then the variable is uninitialized.
type VariableData<'a> = Rc<RefCell<Option<InterpreterData<'a>>>>;

/// The underlying data contained behind a variable / return value.
#[derive(Debug, Clone)]
pub enum InterpreterData<'a> {
    I32(i32),
    U32(u32),
    UnknownNumber(i128),
    Bool(bool),
    Unit(()),
    Char(char),
    String(Rc<RefCell<Cow<'a, str>>>),
    FunctionPtr(MIRFunctionKey),
    /// A reference to a variable (either directly or to a field / array element).
    /// If the value behind the RefCell is None, that means the data hasn't been
    /// initialized yet.
    ///
    /// This also contains the original expression which produced this ref.
    /// This is to allow us to convert the ref back to an expression form.
    /// This means that the interpreter can't optimize refs very well, but that's okay,
    /// because the const_optimize_expr pass can handle that.
    ///
    /// This is None while the expression is being evaluated, but will always be Some
    /// afterwards.
    Ref(VariableData<'a>, Option<Box<MIRExpression<'a>>>),
    /// An array, passed by reference.
    /// This needs to store VariableData to allow taking references
    /// to individual indices.
    Array(Rc<RefCell<Vec<VariableData<'a>>>>),
}

/// Contains information about the function currently being called
/// and is updated by statements as they are executed.
/// It's safe to use a default here for expressions
/// outside a function (e.g., consts).
#[derive(Default)]
pub struct InterpreterScope<'a> {
    /// The variables currently in scope.
    variables: HashMap<Cow<'a, str>, VariableData<'a>>,

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
        let natives = register_natives(&mut ctx)?;

        // Native functions (for the interpreter) now exist.

        insert_fn_arg_args(&mut ctx);

        // Args now exist as phantom variables.
        // This is needed for type checking.

        resolve_fns_to_vars(&mut ctx);

        // Functions now have correct indirect/direct markers.

        if !type_check(&mut ctx) {
            return Err(());
        }

        // Type information now exists.

        if !flatten_loops(&mut ctx) {
            return Err(());
        }

        // Loops no longer exist
        // in MIR.

        // This needs to happen after
        // scope drop is added because
        // it erases scope.
        if !flatten_ifs(&mut ctx) {
            return Err(());
        }

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
            natives,
        })
    }

    /// Evaluates a constant.
    /// If an error occurs, returns None.
    pub fn eval_const(&self, name: &Cow<'a, str>) -> Result<InterpreterData<'a>, ()> {
        if let Some(const_data) = self.constants.borrow().get(name) {
            return Ok(const_data.clone());
        }

        let Some(constant) = self.ctx.program.const_names.get(name) else {
            panic!("Constant not found!");
        };
        let constant = &self.ctx.program.constants[*constant];

        {
            let mut current_evals = self.current_evals.borrow_mut();
            if current_evals.contains(name) {
                eprintln!("Constant/static loop detected: {current_evals:?}");
                return Err(());
            }
            current_evals.insert(name.clone());
        }

        let expr_data = self.eval_expr(&constant.value, &InterpreterScope::default(), false)?;

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
            return Ok(static_data.borrow().clone().expect("Uninitialized static"));
        }

        let Some(static_def) = self.ctx.program.static_names.get(name) else {
            panic!("Static not found!");
        };
        let static_def = &self.ctx.program.statics[*static_def];

        {
            let mut current_evals = self.current_evals.borrow_mut();
            if current_evals.contains(name) {
                eprintln!("Constant/static loop detected: {current_evals:?}");
                return Err(());
            }
            current_evals.insert(name.clone());
        }

        let expr_data = self.eval_expr(&static_def.value, &InterpreterScope::default(), false)?;

        self.statics
            .borrow_mut()
            .insert(name.clone(), Rc::new(RefCell::new(Some(expr_data.clone()))));

        self.current_evals.borrow_mut().remove(name);

        Ok(expr_data)
    }

    /// Evaluates an expression.
    /// This expression MUST exist inside this interpreter's context.
    /// If an error occurs, returns None.
    ///
    /// place determines whether to evaluate in place mode, which means
    /// that variables return references instead of copying the values
    /// of variables.
    /// This should be used when place mode is needed (e.g., when writing
    /// into a place) or recursively when reaching a reference.
    pub fn eval_expr(
        &self,
        expr: &MIRExpression<'a>,
        scope: &InterpreterScope<'a>,
        place: bool,
    ) -> Result<InterpreterData<'a>, ()> {
        macro_rules! simple_binary {
            ($left:expr, $right:expr, $($red_i:path > $red_o:path)|+, $op:tt, $ret:path) => {{
                use InterpreterData::*;

                // None here means an error.
                let left = self.eval_expr($left, scope, place)?;
                let right = self.eval_expr($right, scope, place)?;

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
                simple_binary!(left, right, I32 > I32 | U32 > U32, +, Add)
            }
            MIRExpressionInner::Sub(left, right) => {
                simple_binary!(left, right, I32 > I32 | U32 > U32, -, Sub)
            }
            MIRExpressionInner::Mul(left, right) => {
                simple_binary!(left, right, I32 > I32 | U32 > U32, *, Mul)
            }
            MIRExpressionInner::Div(left, right) => {
                simple_binary!(left, right, I32 > I32 | U32 > U32, /, Div)
            }
            MIRExpressionInner::Equal(left, right) => {
                simple_binary!(left, right, I32 > Bool | U32 > Bool | Bool > Bool | Unit > Bool | String > Bool | FunctionPtr > Bool, ==, Equal)
            }
            MIRExpressionInner::NotEqual(left, right) => {
                simple_binary!(left, right, I32 > Bool | U32 > Bool | Bool > Bool | Unit > Bool | String > Bool | FunctionPtr > Bool, !=, NotEqual)
            }
            MIRExpressionInner::Greater(left, right) => {
                simple_binary!(left, right, I32 > Bool | U32 > Bool | Bool > Bool, >, Greater)
            }
            MIRExpressionInner::Less(left, right) => {
                simple_binary!(left, right, I32 > Bool | U32 > Bool | Bool > Bool, <, Less)
            }
            MIRExpressionInner::GreaterEq(left, right) => {
                simple_binary!(left, right, I32 > Bool | U32 > Bool | Bool > Bool, >=, GreaterEq)
            }
            MIRExpressionInner::LessEq(left, right) => {
                simple_binary!(left, right, I32 > Bool | U32 > Bool | Bool > Bool, <=, LessEq)
            }
            MIRExpressionInner::BoolAnd(left, right) => {
                simple_binary!(left, right, Bool > Bool, &&, BoolAnd)
            }
            MIRExpressionInner::BoolOr(left, right) => {
                simple_binary!(left, right, Bool > Bool, ||, BoolOr)
            }
            MIRExpressionInner::Number(val) => {
                let Some(ty) = &expr.ty else {
                    unreachable!();
                };

                match ty.ty {
                    MIRTypeInner::I32 => Ok(InterpreterData::I32(*val as i32)),
                    MIRTypeInner::U32 => Ok(InterpreterData::U32(
                        // This ensures that negatives are converted consistently.
                        // They should wraparound, which won't happen when converting
                        // from different size and signedness simultaneously.
                        if *val < 0 {
                            *val as i32 as u32
                        } else {
                            *val as u32
                        },
                    )),
                    MIRTypeInner::UnknownNumber => Ok(InterpreterData::UnknownNumber(*val)),
                    _ => unreachable!(),
                }
            }
            MIRExpressionInner::String(val) => {
                Ok(InterpreterData::String(Rc::new(RefCell::new(val.clone()))))
            }
            MIRExpressionInner::Bool(val) => Ok(InterpreterData::Bool(*val)),
            MIRExpressionInner::Unit => Ok(InterpreterData::Unit(())),
            MIRExpressionInner::Char(val) => Ok(InterpreterData::Char(*val)),
            MIRExpressionInner::Variable(name, _) => {
                if place {
                    if let Some(data) = scope.variables.get(name) {
                        return Ok(InterpreterData::Ref(Rc::clone(data), None));
                    }

                    if self.ctx.program.static_names.contains_key(name) {
                        // Ensure the static exists.
                        self.eval_static(name)?;

                        return Ok(InterpreterData::Ref(
                            Rc::clone(&self.statics.borrow()[name]),
                            None,
                        ));
                    }

                    if self.ctx.program.const_names.contains_key(name) {
                        unreachable!(
                            "Tried to evaluate constant variable in place mode (should have been caught by type checker)"
                        )
                    }
                } else {
                    if let Some(data) = scope.variables.get(name) {
                        let data = data.borrow();
                        let Some(data) = data.as_ref() else {
                            eprintln_span!(Some(expr.span.clone()), "Variable has not been set!");
                            return Err(());
                        };

                        return Ok(data.clone());
                    };

                    if self.ctx.program.const_names.contains_key(name) {
                        return self.eval_const(name);
                    }

                    if self.ctx.program.static_names.contains_key(name) {
                        return self.eval_static(name);
                    }
                }

                panic!("Variable not found in scope!");
            }
            MIRExpressionInner::FunctionCall(fn_data) => match &fn_data.source {
                MIRFnSource::Direct(fn_name, ..) => self.eval_function(
                    fn_name,
                    fn_data
                        .args
                        .iter()
                        .map(|arg| {
                            self.eval_expr(arg, scope, place)
                                .map(|v| (v, arg.ty.as_ref().unwrap().ty.clone()))
                        })
                        .collect::<Result<Vec<_>, ()>>()?,
                ),
                MIRFnSource::Indirect(fn_name) => {
                    let InterpreterData::FunctionPtr(source) =
                        self.eval_expr(fn_name, scope, place)?
                    else {
                        panic!("Wrong indirect function call type!");
                    };

                    self.eval_function_key(
                        source,
                        fn_data
                            .args
                            .iter()
                            .map(|arg| {
                                self.eval_expr(arg, scope, place)
                                    .map(|v| (v, arg.ty.as_ref().unwrap().ty.clone()))
                            })
                            .collect::<Result<Vec<_>, ()>>()?,
                    )
                }
            },
            MIRExpressionInner::Ref(inner) => {
                // We need to go into place mode to capture a reference to
                // the inner expression.
                let mut res = self.eval_expr(inner, scope, true)?;
                let InterpreterData::Ref(_, expr) = &mut res else {
                    unreachable!("Inner data for reference wasn't a reference!");
                };
                // We need to use the top-level expr to preserve full information.
                *expr = Some(inner.clone());

                // When we switch into place mode, that by itself adds a level
                // of indirection, so we don't need to directly add a reference.
                // However, if we're already in place mode, then we do need to
                // add a level of indirection.
                if place {
                    unreachable!(
                        "Cannot create references in place mode (type check should have caught this)!"
                    );
                } else {
                    Ok(res)
                }
            }
            MIRExpressionInner::Deref(inner) => {
                let InterpreterData::Ref(inner, _) = self.eval_expr(inner, scope, place)? else {
                    unreachable!("Inner data for dereference wasn't a reference!");
                };

                // In place mode, we want a reference to the data behind the reference.
                // Place mode by default already has one layer of indirection, so if a: &i32
                // and we write *a, we'll see Ref<Ref<i32>>, where the inner ref represents the
                // variable a is pointing to, and the outer ref represents a.
                // Our job is to strip that away.
                //
                // In non-place mode, we'll just see Ref<i32>, then strip it to i32.
                //
                // So in both cases, we strip away the outer Ref.
                Ok(inner
                    .borrow()
                    .clone()
                    .expect("Dereferenced uninitialized reference"))
            }
            MIRExpressionInner::Member(_base, _name) => {
                todo!()
            }
            MIRExpressionInner::Index(base, index) => {
                // Index crosses the place expression boundary,
                // so place = false when evaluating it, no matter if
                // we're current in place mode.
                //
                // Because base is accessing an array, which is already a reference,
                // then we actually want to capture it in non-place mode as well.
                // However, place mode still determines whether we return a reference or not.
                let base = self.eval_expr(base, scope, false)?;
                let index = self.eval_expr(index, scope, false)?;

                let index: usize = match index {
                    InterpreterData::U32(val) => val as usize,
                    InterpreterData::I32(val) => {
                        if val < 0 {
                            panic!("Negative array index!");
                        }

                        val as usize
                    }
                    InterpreterData::UnknownNumber(val) => {
                        val.to_usize().expect("Array index too large!")
                    }
                    _ => panic!("Index is not an integer!"),
                };

                match base {
                    InterpreterData::Array(elems) => {
                        if index >= elems.borrow().len() {
                            panic!("Index out of bounds!");
                        }

                        if place {
                            Ok(InterpreterData::Ref(
                                Rc::clone(&elems.borrow()[index]),
                                None,
                            ))
                        } else {
                            Ok(elems.borrow()[index]
                                .borrow()
                                .clone()
                                .expect("Uninitialized array element"))
                        }
                    }
                    InterpreterData::String(inner) => {
                        if index >= inner.borrow().len() {
                            panic!("Index out of bounds!");
                        }

                        if place {
                            todo!("Cannot take a reference to a string index!");
                        } else {
                            Ok(InterpreterData::Char(
                                inner.borrow().chars().nth(index).unwrap(),
                            ))
                        }
                    }
                    _ => panic!("Index base is not an array or string!"),
                }
            }
            MIRExpressionInner::Array(elems) => {
                let elems = elems
                    .iter()
                    .map(|expr| {
                        self.eval_expr(expr, scope, false)
                            .map(|v| Rc::new(RefCell::new(Some(v))))
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(InterpreterData::Array(Rc::new(RefCell::new(elems))))
            }
            MIRExpressionInner::Neg(inner) => {
                let inner = self.eval_expr(inner, scope, place)?;

                match inner {
                    InterpreterData::I32(val) => Ok(InterpreterData::I32(-val)),
                    InterpreterData::U32(val) => panic!("Cannot negate unsigned number!"),
                    InterpreterData::UnknownNumber(val) => Ok(InterpreterData::UnknownNumber(-val)),
                    _ => panic!("Cannot negate non-number!"),
                }
            }
            MIRExpressionInner::Not(inner) => {
                let inner = self.eval_expr(inner, scope, place)?;

                match inner {
                    InterpreterData::Bool(val) => Ok(InterpreterData::Bool(!val)),
                    _ => panic!("Cannot negate non-bool!"),
                }
            }
            MIRExpressionInner::Quine
            | MIRExpressionInner::QuineLen
            | MIRExpressionInner::QuineSpace
            | MIRExpressionInner::QuineLine
            | MIRExpressionInner::Binding(_, _, _) => {
                // We can't handle these expressions.
                Err(())
            }
        }
    }

    /// Evaluates a function call, performing function lookup by name.
    /// If an error occurs, returns Err.
    pub fn eval_function(
        &self,
        fn_name: &Cow<'a, str>,
        args: Vec<(InterpreterData<'a>, MIRTypeInner<'a>)>,
    ) -> Result<InterpreterData<'a>, ()> {
        let call_args: Vec<_> = args.iter().map(|(_, ty)| ty.clone()).collect();

        let Some(fn_key) = self
            .ctx
            .program
            .function_names
            .get(fn_name)
            .and_then(|v| v.find_compatible(&call_args))
        else {
            panic!("Function not found!");
        };

        self.eval_function_key(fn_key, args)
    }

    /// Evaluates a function call, using the function's key directly.
    /// If an error occurs, returns Err.
    pub fn eval_function_key(
        &self,
        fn_key: MIRFunctionKey,
        args: Vec<(InterpreterData<'a>, MIRTypeInner<'a>)>,
    ) -> Result<InterpreterData<'a>, ()> {
        let fn_data = &self.ctx.program.functions[fn_key];

        if let Some(native) = self.natives.funcs.get(fn_key) {
            // This is a native function, so we can just call it directly.
            return Ok(native(&args));
        }

        if fn_data.fn_type == MIRFunctionType::Extern {
            eprintln!(
                "Function {} is extern and cannot be called at compile-time!",
                fn_data.name
            );
            return Err(());
        }

        assert_eq!(fn_data.args.len(), args.len());

        let mut scope = InterpreterScope::default();
        for ((data, _ty), arg) in args.into_iter().zip(fn_data.args.iter()) {
            scope
                .variables
                .insert(arg.name.clone(), Rc::new(RefCell::new(Some(data))));
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
            MIRStatement::CreateVariable { var, value, .. } => {
                // Phantom variables for args aren't needed here.
                if var.arg {
                    return Ok(None);
                }

                let value = match value {
                    Some(value) => Some(self.eval_expr(value, scope, false)?),
                    None => None,
                };

                scope
                    .variables
                    .insert(var.name.clone(), Rc::new(RefCell::new(value)));
            }
            MIRStatement::SetVariable { place, value, .. }
            | MIRStatement::AddAssign { place, value, .. }
            | MIRStatement::SubAssign { place, value, .. }
            | MIRStatement::MulAssign { place, value, .. }
            | MIRStatement::DivAssign { place, value, .. } => {
                let value = match statement {
                    MIRStatement::SetVariable { .. } => self.eval_expr(value, scope, false)?,
                    MIRStatement::AddAssign { .. } => self.eval_expr(
                        &MIRExpression {
                            inner: MIRExpressionInner::Add(
                                Box::new(place.clone()),
                                Box::new(value.clone()),
                            ),
                            ty: value.ty.clone(),
                            span: Span::empty(),
                        },
                        scope,
                        false,
                    )?,
                    MIRStatement::SubAssign { .. } => self.eval_expr(
                        &MIRExpression {
                            inner: MIRExpressionInner::Sub(
                                Box::new(place.clone()),
                                Box::new(value.clone()),
                            ),
                            ty: value.ty.clone(),
                            span: Span::empty(),
                        },
                        scope,
                        false,
                    )?,
                    MIRStatement::MulAssign { .. } => self.eval_expr(
                        &MIRExpression {
                            inner: MIRExpressionInner::Mul(
                                Box::new(place.clone()),
                                Box::new(value.clone()),
                            ),
                            ty: value.ty.clone(),
                            span: Span::empty(),
                        },
                        scope,
                        false,
                    )?,
                    MIRStatement::DivAssign { .. } => self.eval_expr(
                        &MIRExpression {
                            inner: MIRExpressionInner::Div(
                                Box::new(place.clone()),
                                Box::new(value.clone()),
                            ),
                            ty: value.ty.clone(),
                            span: Span::empty(),
                        },
                        scope,
                        false,
                    )?,
                    _ => unreachable!(),
                };

                let InterpreterData::Ref(place, _) = self.eval_expr(place, scope, true)? else {
                    panic!("SetVariable place is not a reference!");
                };

                *place.borrow_mut() = Some(value);
            }
            MIRStatement::IncrementVariable { place, .. }
            | MIRStatement::DecrementVariable { place, .. } => {
                let value = match statement {
                    MIRStatement::IncrementVariable { .. } => self.eval_expr(
                        &MIRExpression {
                            inner: MIRExpressionInner::Add(
                                Box::new(place.clone()),
                                Box::new(MIRExpression {
                                    inner: MIRExpressionInner::Number(1),
                                    ty: place.ty.clone(),
                                    span: Span::empty(),
                                }),
                            ),
                            ty: place.ty.clone(),
                            span: Span::empty(),
                        },
                        scope,
                        false,
                    )?,
                    MIRStatement::DecrementVariable { .. } => self.eval_expr(
                        &MIRExpression {
                            inner: MIRExpressionInner::Sub(
                                Box::new(place.clone()),
                                Box::new(MIRExpression {
                                    inner: MIRExpressionInner::Number(1),
                                    ty: place.ty.clone(),
                                    span: Span::empty(),
                                }),
                            ),
                            ty: place.ty.clone(),
                            span: Span::empty(),
                        },
                        scope,
                        false,
                    )?,
                    _ => unreachable!(),
                };

                let InterpreterData::Ref(place, _) = self.eval_expr(place, scope, true)? else {
                    panic!("SetVariable place is not a reference!");
                };

                *place.borrow_mut() = Some(value);
            }
            MIRStatement::DropVariable(_, _, _) => {
                // Dropping has no effect on the interpreter.
            }
            MIRStatement::FunctionCall(fn_data) => match &fn_data.source {
                MIRFnSource::Direct(fn_name, ..) => {
                    self.eval_function(
                        fn_name,
                        fn_data
                            .args
                            .iter()
                            .map(|arg| {
                                self.eval_expr(arg, scope, false)
                                    .map(|v| (v, arg.ty.as_ref().unwrap().ty.clone()))
                            })
                            .collect::<Result<Vec<_>, ()>>()?,
                    )?;
                }
                MIRFnSource::Indirect(fn_name) => {
                    let InterpreterData::FunctionPtr(source) =
                        self.eval_expr(fn_name, scope, false)?
                    else {
                        panic!("Wrong indirect function call type!");
                    };

                    self.eval_function_key(
                        source,
                        fn_data
                            .args
                            .iter()
                            .map(|arg| {
                                self.eval_expr(arg, scope, false)
                                    .map(|v| (v, arg.ty.as_ref().unwrap().ty.clone()))
                            })
                            .collect::<Result<Vec<_>, ()>>()?,
                    )?;
                }
            },
            MIRStatement::Return { expr, .. } => {
                let Some(expr) = expr else {
                    return Ok(Some(InterpreterData::Unit(())));
                };

                return Ok(Some(self.eval_expr(expr, scope, false)?));
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
                let InterpreterData::Bool(condition) = self.eval_expr(condition, scope, false)?
                else {
                    panic!("GotoNotEqual condition is not a boolean!");
                };

                if !condition {
                    scope.next_idx = Some(index.expect("GotoNotEqual statement has no index!"));
                }
            }
            MIRStatement::IfStatement { .. }
            | MIRStatement::LoopStatement { .. }
            | MIRStatement::ScopeStatement { .. }
            | MIRStatement::ContinueStatement { .. }
            | MIRStatement::BreakStatement { .. } => {
                panic!("This statement type cannot exist at this phase!");
            }
            MIRStatement::MarkerStatement { .. } => {
                // Markers don't do anything on their own.
                // The quine statement uses them, but we don't
                // allow it here.
                // The chunk statement writes to them, and that's
                // interpreter-only, so we must handle it here.
            }
        }

        Ok(None)
    }
}

impl<'a> From<InterpreterData<'a>> for MIRExpressionInner<'a> {
    fn from(value: InterpreterData<'a>) -> Self {
        match value {
            InterpreterData::I32(v) => MIRExpressionInner::Number(v as i128),
            InterpreterData::U32(v) => MIRExpressionInner::Number(v as i128),
            InterpreterData::UnknownNumber(v) => MIRExpressionInner::Number(v),
            InterpreterData::Bool(v) => MIRExpressionInner::Bool(v),
            InterpreterData::Unit(_) => todo!("Add a unit expression to support this"),
            InterpreterData::Char(v) => MIRExpressionInner::Char(v),
            InterpreterData::String(v) => MIRExpressionInner::String(v.borrow().clone()),
            InterpreterData::FunctionPtr(_v) => {
                todo!("Figure out how to handle function pointers to overloaded functions")
            }
            InterpreterData::Ref(_, expr) => MIRExpressionInner::Ref(expr.unwrap()),
            InterpreterData::Array(elems) => MIRExpressionInner::Array(
                elems
                    .borrow()
                    .iter()
                    .map(|expr| MIRExpression {
                        inner: expr
                            .borrow()
                            .clone()
                            .expect("Uninitialized array element!")
                            .into(),
                        ty: None,
                        span: Span::empty(),
                    })
                    .collect(),
            ),
        }
    }
}
