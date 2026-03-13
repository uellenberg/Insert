use crate::mir::interpreter::{Interpreter, InterpreterScope};
use crate::mir::scope::StatementExplorer;
use crate::mir::{
    MIRContext, MIRExpression, MIRExpressionInner, MIRFnCall, MIRFnSource, MIRFunction,
    MIRStatement,
};
use crate::targets::Target;

/// Attempts to evaluate all constants and statics, returning
/// whether it was successful.
pub fn const_eval<'a>(ctx: &mut MIRContext<'a>, interpreter: &mut Interpreter<'a>) -> bool {
    for (const_name, const_key) in &ctx.program.const_names {
        let const_ = &mut ctx.program.constants[*const_key];

        // If an error occurs, ignore it, since this is an optional step.
        // This includes errors which the user may have to intervene with, or
        // errors caused by us being unable to evaluate the constant.
        //
        // We can still use partial evaluation to still get some optimization
        // when an error occurs, though.
        if let Ok(value) = interpreter.eval_const(const_name) {
            const_.value.inner = value.into();
        } else {
            partial_const_eval(&mut const_.value, interpreter);
        }
    }

    for (static_name, static_key) in &ctx.program.static_names {
        let static_ = &mut ctx.program.statics[*static_key];

        if let Ok(value) = interpreter.eval_static(static_name) {
            ctx.program
                .statics
                .get_mut(*static_key)
                .unwrap()
                .value
                .inner = value.into();
        } else {
            partial_const_eval(&mut static_.value, interpreter);
        }
    }

    true
}

/// Attempts to evaluate all subexpressions of the given expression
/// using the interpreter.
/// This is needed because the interpreter may fail to evaluate an expression,
/// but is still able to simplify subexpressions.
fn partial_const_eval<'a>(expr: &mut MIRExpression<'a>, interpreter: &Interpreter<'a>) {
    explore_expr_mut(expr, &mut |expr| {
        if let Ok(data) = interpreter.eval_expr(expr, &InterpreterScope::default(), false) {
            expr.inner = data.into();
        }

        true
    });
}

/// Inlines constants in all expressions, returning
/// whether it was successful.
/// After this, constants won't appear in any expressions.
/// This MUST occur after const evaluation.
pub fn inline_consts(ctx: &mut MIRContext) -> bool {
    for function in ctx.program.functions.values_mut() {
        let res = <StatementExplorer>::explore_block_mut(
            &mut function.body,
            &mut |statement, _scope| {
                find_exprs_mut(statement, &mut |expr, _| {
                    explore_expr_mut(expr, &mut |expr| {
                        if let MIRExpressionInner::Variable(name, _) = &expr.inner {
                            // Ensure the constant exists.
                            if let Some(key) = ctx.program.const_names.get(name) {
                                // Constants are guaranteed to already be evaluated.

                                // No need to validate that this is a primitive here,
                                // since eval_constant already does that.
                                *expr = ctx.program.constants[*key].value.clone();
                            }
                        }

                        true
                    })
                });

                true
            },
            &|_, _| true,
            &mut |_, _| true,
        );

        if !res {
            return false;
        }
    }

    true
}

/// Attempts to optimize all expressions
/// in this function, returning
/// whether it was successful.
/// This MUST occur after const inlining.
/// Returns (success, modified).
pub fn optimize_exprs(function: &mut MIRFunction, target: &dyn Target) -> (bool, bool) {
    let mut modified = false;

    let res = <StatementExplorer>::explore_block_mut(
        &mut function.body,
        &mut |statement, _scope| {
            find_exprs_mut(statement, &mut |expr, _| {
                let (success, modified1) = reduce_expr(expr, target);
                modified |= modified1;

                success
            });

            true
        },
        &|_, _| true,
        &mut |_, _| true,
    );

    if !res {
        return (false, false);
    }

    (true, modified)
}

/// Attempts to reduce an expression
/// using simple constant evaluation.
/// Returns (success, modified).
fn reduce_expr(expr: &mut MIRExpression, target: &dyn Target) -> (bool, bool) {
    let mut modified = false;

    macro_rules! simple_binary {
        ($left:expr, $right:expr, $($red_i:path)|+, $red_o:path, $op:tt) => {{
            use MIRExpressionInner::*;

            $(if let $red_i(left, ..) = $left.inner {
                if let $red_i(right, ..) = $right.inner {
                    return Some($red_o(left $op right));
                }
            })+

            None
        }};
    }

    let is_falsy = |expr: &MIRExpression| -> bool {
        match &expr.inner {
            MIRExpressionInner::Bool(false) => true,
            MIRExpressionInner::Number(0) if target.truthy_coercion() => true,
            MIRExpressionInner::Char('\0') if target.truthy_coercion() => true,
            _ => false,
        }
    };

    if !explore_expr_mut(expr, &mut |expr| {
        let mut failed = false;

        let new_expr = (|| match &expr.inner {
            MIRExpressionInner::Add(left, right) => {
                simple_binary!(left, right, Number, Number, +)
            }
            MIRExpressionInner::Sub(left, right) => {
                simple_binary!(left, right, Number, Number, -)
            }
            MIRExpressionInner::Mul(left, right) => {
                // Constant folding first, then -1 negation shorthand.
                match (&left.inner, &right.inner) {
                    // Normal reduction.
                    (MIRExpressionInner::Number(l), MIRExpressionInner::Number(r)) => {
                        Some(MIRExpressionInner::Number(l * r))
                    }

                    // -1*x can be replaced with -x.
                    (MIRExpressionInner::Number(-1), _) => {
                        Some(MIRExpressionInner::Neg(right.clone()))
                    }
                    (_, MIRExpressionInner::Number(-1)) => {
                        Some(MIRExpressionInner::Neg(left.clone()))
                    }
                    _ => None,
                }
            }
            MIRExpressionInner::Div(left, right) => {
                simple_binary!(left, right, Number, Number, /)
            }
            MIRExpressionInner::Equal(left, right) => {
                // For falsey values, a == 0 can be reduced to !a.
                if is_falsy(right) {
                    return Some(MIRExpressionInner::Not(left.clone()));
                }
                if is_falsy(left) {
                    return Some(MIRExpressionInner::Not(right.clone()));
                }

                // We can also reduce a == true to just a.
                if matches!(&right.inner, MIRExpressionInner::Bool(true)) {
                    return Some(left.inner.clone());
                }
                if matches!(&left.inner, MIRExpressionInner::Bool(true)) {
                    return Some(right.inner.clone());
                }

                simple_binary!(left, right, Number | Bool, Bool, ==)
            }
            MIRExpressionInner::NotEqual(left, right) => {
                // For falsy values, a != 0 can be reduced to a.
                if is_falsy(right) {
                    return Some(left.inner.clone());
                }
                if is_falsy(left) {
                    return Some(right.inner.clone());
                }

                // We can also reduce a == true to just !a.
                if matches!(&right.inner, MIRExpressionInner::Bool(true)) {
                    return Some(MIRExpressionInner::Not(left.clone()));
                }
                if matches!(&left.inner, MIRExpressionInner::Bool(true)) {
                    return Some(MIRExpressionInner::Not(right.clone()));
                }

                simple_binary!(left, right, Number | Bool, Bool, !=)
            }
            MIRExpressionInner::Greater(left, right) => {
                simple_binary!(left, right, Number | Bool, Bool, >)
            }
            MIRExpressionInner::Less(left, right) => {
                simple_binary!(left, right, Number | Bool, Bool, <)
            }
            MIRExpressionInner::GreaterEq(left, right) => {
                simple_binary!(left, right, Number | Bool, Bool, >=)
            }
            MIRExpressionInner::LessEq(left, right) => {
                simple_binary!(left, right, Number | Bool, Bool, <=)
            }
            MIRExpressionInner::BoolAnd(left, right) => {
                simple_binary!(left, right, Bool, Bool, &&)
            }
            MIRExpressionInner::BoolOr(left, right) => {
                simple_binary!(left, right, Bool, Bool, ||)
            }
            MIRExpressionInner::Neg(box MIRExpression {
                inner: MIRExpressionInner::Number(n),
                ..
            }) => Some(MIRExpressionInner::Number(-n)),
            MIRExpressionInner::Not(inner) => match &inner.inner {
                MIRExpressionInner::Bool(b) => Some(MIRExpressionInner::Bool(!b)),
                MIRExpressionInner::Number(n) if target.truthy_coercion() => {
                    Some(MIRExpressionInner::Bool(*n == 0))
                }
                MIRExpressionInner::Char(n) if target.truthy_coercion() => {
                    Some(MIRExpressionInner::Bool(*n == '\0'))
                }
                _ => None,
            },
            MIRExpressionInner::Ref(box MIRExpression {
                inner: MIRExpressionInner::Deref(inner),
                ..
            })
            | MIRExpressionInner::Deref(box MIRExpression {
                inner: MIRExpressionInner::Ref(inner),
                ..
            }) => Some(inner.inner.clone()),

            // TODO: Implement member access reduction for const structs.
            MIRExpressionInner::Member(_, _) => None,

            MIRExpressionInner::Index(
                box MIRExpression {
                    inner: MIRExpressionInner::Array(elems),
                    ..
                },
                box MIRExpression {
                    inner: MIRExpressionInner::Number(idx),
                    ..
                },
            ) => {
                if idx < &0 || idx >= &(elems.len() as i128) {
                    eprintln!("Array index out of range!");
                    failed = true;
                    return None;
                }

                Some(elems[*idx as usize].inner.clone())
            }

            MIRExpressionInner::Index(
                box MIRExpression {
                    inner: MIRExpressionInner::String(elems),
                    ..
                },
                box MIRExpression {
                    inner: MIRExpressionInner::Number(idx),
                    ..
                },
            ) => {
                if idx < &0 || idx >= &(elems.len() as i128) {
                    eprintln!("Array index out of range!");
                    failed = true;
                    return None;
                }

                Some(MIRExpressionInner::Char(
                    elems.chars().nth(*idx as usize).unwrap(),
                ))
            }

            // a[0] can be written as *a if arrays are refs.
            MIRExpressionInner::Index(
                inner,
                box MIRExpression {
                    inner: MIRExpressionInner::Number(0),
                    ..
                },
            ) if target.array_as_ref() => Some(MIRExpressionInner::Deref(inner.clone())),

            // &a[b] can be written as a+b if arrays are refs.
            MIRExpressionInner::Ref(box MIRExpression {
                inner: MIRExpressionInner::Index(base, offset),
                ..
            }) if target.array_as_ref() => {
                Some(MIRExpressionInner::Add(base.clone(), offset.clone()))
            }

            MIRExpressionInner::Ternary(cond, on_true, on_false) => match &cond.inner {
                MIRExpressionInner::Bool(true) => Some(on_true.inner.clone()),
                MIRExpressionInner::Bool(false) => Some(on_false.inner.clone()),
                MIRExpressionInner::Number(num) if target.truthy_coercion() => Some(if *num == 0 {
                    on_false.inner.clone()
                } else {
                    on_true.inner.clone()
                }),
                MIRExpressionInner::Char(num) if target.truthy_coercion() => {
                    Some(if *num == '\0' {
                        on_false.inner.clone()
                    } else {
                        on_true.inner.clone()
                    })
                }
                _ => None,
            },

            // Already fully simplified (recursion handled by explore_expr_mut).
            MIRExpressionInner::Index(_, _)
            | MIRExpressionInner::FunctionCall(_)
            | MIRExpressionInner::Number(_)
            | MIRExpressionInner::String(_)
            | MIRExpressionInner::Bool(_)
            | MIRExpressionInner::Char(_)
            | MIRExpressionInner::Unit
            | MIRExpressionInner::Variable(_, _)
            | MIRExpressionInner::Ref(_)
            | MIRExpressionInner::Deref(_)
            | MIRExpressionInner::Neg(_)
            | MIRExpressionInner::Array(_)
            | MIRExpressionInner::Quine
            | MIRExpressionInner::QuineLen
            | MIRExpressionInner::QuineSpace
            | MIRExpressionInner::QuineLine
            | MIRExpressionInner::Binding(_, _, _) => None,
        })();

        if failed {
            return false;
        }

        if let Some(new_expr) = new_expr {
            expr.inner = new_expr;
            modified = true;
        }

        true
    }) {
        return (false, false);
    }

    (true, modified)
}

macro_rules! explore_expr_body {
    ($recurse:expr, $expr:expr, $inner_expr_ref:expr, $fn_data:ident => ($fn_source:expr, $fn_args:expr), $visit:expr) => {{
        macro_rules! binary_recurse {
            ($left:expr, $right:expr) => {{
                if !$recurse($left, $visit) {
                    return false;
                }
                if !$recurse($right, $visit) {
                    return false;
                }
            }};
        }

        match $inner_expr_ref {
            // Binary expressions.
            MIRExpressionInner::Add(left, right) => binary_recurse!(left, right),
            MIRExpressionInner::Sub(left, right) => binary_recurse!(left, right),
            MIRExpressionInner::Mul(left, right) => binary_recurse!(left, right),
            MIRExpressionInner::Div(left, right) => binary_recurse!(left, right),
            MIRExpressionInner::Equal(left, right) => binary_recurse!(left, right),
            MIRExpressionInner::NotEqual(left, right) => binary_recurse!(left, right),
            MIRExpressionInner::Greater(left, right) => binary_recurse!(left, right),
            MIRExpressionInner::Less(left, right) => binary_recurse!(left, right),
            MIRExpressionInner::GreaterEq(left, right) => binary_recurse!(left, right),
            MIRExpressionInner::LessEq(left, right) => binary_recurse!(left, right),
            MIRExpressionInner::BoolAnd(left, right) => binary_recurse!(left, right),
            MIRExpressionInner::BoolOr(left, right) => binary_recurse!(left, right),
            MIRExpressionInner::Index(base, index) => binary_recurse!(base, index),

            // Unary expressions.
            MIRExpressionInner::Ref(inner)
            | MIRExpressionInner::Deref(inner)
            | MIRExpressionInner::Member(inner, _)
            | MIRExpressionInner::Neg(inner)
            | MIRExpressionInner::Not(inner) => {
                if !$recurse(inner, $visit) {
                    return false;
                }
            }

            MIRExpressionInner::FunctionCall($fn_data) => {
                if let MIRFnSource::Indirect(expr) = $fn_source {
                    if !$recurse(expr, $visit) {
                        return false;
                    }
                }

                for arg in $fn_args {
                    if !$recurse(arg, $visit) {
                        return false;
                    }
                }
            }

            MIRExpressionInner::Array(elems) => {
                for elem in elems {
                    if !$recurse(elem, $visit) {
                        return false;
                    }
                }
            }

            MIRExpressionInner::Binding(_, inner, _) => {
                if !$recurse(inner, $visit) {
                    return false;
                }
            }

            MIRExpressionInner::Ternary(cond, on_true, on_false) => {
                if !$recurse(cond, $visit) {
                    return false;
                }

                if !$recurse(on_true, $visit) {
                    return false;
                }

                if !$recurse(on_false, $visit) {
                    return false;
                }
            }

            // No inner expressions.
            MIRExpressionInner::Number(_)
            | MIRExpressionInner::String(_)
            | MIRExpressionInner::Bool(_)
            | MIRExpressionInner::Char(_)
            | MIRExpressionInner::Unit
            | MIRExpressionInner::Variable(_, _)
            | MIRExpressionInner::Quine
            | MIRExpressionInner::QuineLen
            | MIRExpressionInner::QuineSpace
            | MIRExpressionInner::QuineLine => {}
        }

        if !$visit($expr) {
            return false;
        }

        true
    }};
}

/// Recursively traverses an expression,
/// calling the visit function on each expression
/// bottom-up (visit is called after visiting children).
/// The visit function should return true on success,
/// and this function will return whether all visits
/// succeeded.
pub fn explore_expr<'a>(
    expr: &MIRExpression<'a>,
    visit: &mut impl FnMut(&MIRExpression<'a>) -> bool,
) -> bool {
    explore_expr_body!(explore_expr, expr, &expr.inner, fn_data => (&fn_data.source, &fn_data.args), visit)
}

/// Recursively traverses an expression,
/// calling the rewrite on each expression
/// bottom-up (rewrite is called after visiting children).
/// The rewrite function should return true on success,
/// and this function will return whether all rewrites
/// succeeded.
pub fn explore_expr_mut<'a>(
    expr: &mut MIRExpression<'a>,
    rewrite: &mut impl FnMut(&mut MIRExpression<'a>) -> bool,
) -> bool {
    explore_expr_body!(explore_expr_mut, expr, &mut expr.inner, fn_data => (&mut fn_data.source, &mut fn_data.args), rewrite)
}

/// This functions the same as [explore_expr], but it only explores
/// the outermost part of a place expression. In other words, the boundary
/// where a place expression stops being strictly a place expression.
/// This is used to differentiate between the part of the place expression
/// which refers to an area to modify, and the part that's supplemental towards that.
///
/// For example, in this expression:
/// *a[b + 1]
///
/// The b + 1 part is considered an inner part, so isn't returned.
pub fn explore_outer_place<'a>(
    expr: &MIRExpression<'a>,
    visit: &mut impl FnMut(&MIRExpression<'a>) -> bool,
) -> bool {
    let mut should_visit = false;

    match &expr.inner {
        MIRExpressionInner::Ref(inner)
        | MIRExpressionInner::Deref(inner)
        | MIRExpressionInner::Member(inner, _)
        // The index part of the expression crosses the boundary into non-place expressions, so
        // isn't returned.
        | MIRExpressionInner::Index(inner, _) => {
            if !explore_outer_place(inner, visit) {
                return false;
            }

            should_visit = true;
        }

        // A ternary can be used in a place expression, but only the ref
        // returned (i.e., the true and false branches) are the outer places,
        // not the condition.
        MIRExpressionInner::Ternary(_, on_true, on_false) => {
            if !explore_outer_place(on_true, visit) {
                return false;
            }
            if !explore_outer_place(on_false, visit) {
                return false;
            }

            should_visit = true;
        }

        MIRExpressionInner::Variable(_, _) => {
            should_visit = true;
        }

        // No need to explore non-place expressions, because we've
        // already crossed the boundary.
        _ => {}
    }

    if should_visit && !visit(expr) {
        return false;
    }

    true
}

macro_rules! extract_expr_body {
    ($statement:expr, $for_each:expr) => {{
        // for_each is fn(expr, is write place) -> bool.

        match $statement {
            // No expressions.
            MIRStatement::CreateVariable { value: None, .. } => {}
            MIRStatement::DropVariable(..) => {}
            MIRStatement::Goto { .. } => {}
            MIRStatement::Label { .. } => {}
            MIRStatement::ContinueStatement { .. } => {}
            MIRStatement::BreakStatement { .. } => {}
            MIRStatement::LoopStatement {
                condition: None, ..
            } => {}
            MIRStatement::ScopeStatement { .. } => {}
            MIRStatement::MarkerStatement { .. } => {}

            MIRStatement::LoopStatement {
                condition: Some(condition),
                ..
            } => {
                if !$for_each(condition, false) {
                    return false;
                }
            }

            MIRStatement::CreateVariable {
                value: Some(value), ..
            }
            | MIRStatement::IfStatement {
                condition: value, ..
            }
            | MIRStatement::GotoNotEqual {
                condition: value, ..
            } => {
                if !$for_each(value, false) {
                    return false;
                }
            }

            MIRStatement::SetVariable { place, value, .. }
            | MIRStatement::AddAssign { place, value, .. }
            | MIRStatement::SubAssign { place, value, .. }
            | MIRStatement::MulAssign { place, value, .. }
            | MIRStatement::DivAssign { place, value, .. } => {
                if !$for_each(place, true) {
                    return false;
                }
                if !$for_each(value, false) {
                    return false;
                }
            }

            MIRStatement::IncrementVariable { place, .. }
            | MIRStatement::DecrementVariable { place, .. } => {
                if !$for_each(place, true) {
                    return false;
                }
            }

            MIRStatement::FunctionCall(MIRFnCall { source, args, .. }) => {
                if let MIRFnSource::Indirect(expr) = source {
                    if !$for_each(expr, false) {
                        return false;
                    }
                }

                for arg in args {
                    if !$for_each(arg, false) {
                        return false;
                    }
                }
            }

            MIRStatement::Return { expr, .. } => {
                if let Some(expr) = expr {
                    if !$for_each(expr, false) {
                        return false;
                    }
                }
            }
        }

        true
    }};
}

/// Extracts all expressions from a statement
/// and runs the for_each function on them  (expr, is write place).
pub fn find_exprs<'a, 'b>(
    statement: &'b MIRStatement<'a>,
    for_each: &mut impl FnMut(&'b MIRExpression<'a>, bool) -> bool,
) -> bool {
    extract_expr_body!(statement, for_each)
}

/// Extracts all expressions from a statement
/// and runs the rewrite function on them (expr, is write place).
pub fn find_exprs_mut<'a, 'b>(
    statement: &'b mut MIRStatement<'a>,
    rewrite: &mut impl FnMut(&'b mut MIRExpression<'a>, bool) -> bool,
) -> bool {
    extract_expr_body!(statement, rewrite)
}
