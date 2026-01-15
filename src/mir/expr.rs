use crate::mir::interpreter::Interpreter;
use crate::mir::scope::StatementExplorer;
use crate::mir::{
    MIRContext, MIRExpression, MIRExpressionInner, MIRFnCall, MIRFnSource, MIRStatement,
};
use std::borrow::Cow;

/// Attempts to evaluate all constants and statics, returning
/// whether it was successful.
pub fn const_eval<'a>(ctx: &mut MIRContext<'a>, interpreter: &mut Interpreter<'a>) -> bool {
    for (const_name, const_key) in &ctx.program.const_names {
        let Ok(value) = interpreter.eval_const(const_name) else {
            return false;
        };

        ctx.program
            .constants
            .get_mut(*const_key)
            .unwrap()
            .value
            .inner = value.into();
    }

    for (static_name, static_key) in &ctx.program.static_names {
        let Ok(value) = interpreter.eval_static(static_name) else {
            return false;
        };

        ctx.program
            .statics
            .get_mut(*static_key)
            .unwrap()
            .value
            .inner = value.into();
    }

    true
}

/// Attempts to optimize all expressions
/// in every function, returning
/// whether it was successful.
/// This MUST occur after const evaluation.
pub fn const_optimize_expr(ctx: &mut MIRContext<'_>) -> bool {
    for function in ctx.program.functions.values_mut() {
        let res = <StatementExplorer>::explore_block_mut(
            &mut function.body,
            &|statement, _scope| {
                find_exprs_mut(statement, &mut |expr| {
                    reduce_expr(expr, &mut |name| {
                        // Ensure the constant exists.
                        let key = ctx.program.const_names.get(&name)?;

                        // Constants are guaranteed to already be evaluated.

                        // No need to validate that this is a primitive here,
                        // since eval_constant already does that.
                        Some(ctx.program.constants[*key].value.clone())
                    });

                    true
                });

                true
            },
            &|_, _| true,
            &|_, _| true,
        );

        if !res {
            return false;
        }
    }

    true
}

/// Attempts to reduce an expression
/// using simple constant evaluation.
fn reduce_expr<'a>(
    expr: &mut MIRExpression<'a>,
    get_const: &mut impl FnMut(Cow<'a, str>) -> Option<MIRExpression<'a>>,
) {
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

    explore_expr_mut(expr, &mut |expr| {
        let new_expr = (|| match &expr.inner {
            MIRExpressionInner::Add(left, right) => {
                simple_binary!(left, right, Number, Number, +)
            }
            MIRExpressionInner::Sub(left, right) => {
                simple_binary!(left, right, Number, Number, -)
            }
            MIRExpressionInner::Mul(left, right) => {
                simple_binary!(left, right, Number, Number, *)
            }
            MIRExpressionInner::Div(left, right) => {
                simple_binary!(left, right, Number, Number, /)
            }
            MIRExpressionInner::Equal(left, right) => {
                simple_binary!(left, right, Number | Bool, Bool, ==)
            }
            MIRExpressionInner::NotEqual(left, right) => {
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

            MIRExpressionInner::Variable(name) => get_const(name.clone()).map(|v| v.inner),

            // TODO: Implement member access reduction for const structs.
            MIRExpressionInner::Member(_, _) => None,

            // TODO: Implement ref/deref reduction.
            MIRExpressionInner::Ref(_) | MIRExpressionInner::Deref(_) => None,

            // Already fully simplified (recursion handled by explore_expr_mut).
            MIRExpressionInner::Index(_, _)
            | MIRExpressionInner::FunctionCall(_)
            | MIRExpressionInner::Number(_)
            | MIRExpressionInner::String(_)
            | MIRExpressionInner::Bool(_)
            | MIRExpressionInner::Unit => None,
        })();

        if let Some(new_expr) = new_expr {
            expr.inner = new_expr;
        }

        true
    });
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
            | MIRExpressionInner::Member(inner, _) => {
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

            // No inner expressions.
            MIRExpressionInner::Number(_)
            | MIRExpressionInner::String(_)
            | MIRExpressionInner::Bool(_)
            | MIRExpressionInner::Unit
            | MIRExpressionInner::Variable(_) => {}
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

        MIRExpressionInner::Variable(_) => {
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
        match $statement {
            // No expressions.
            MIRStatement::CreateVariable { value: None, .. } => {}
            MIRStatement::DropVariable(..) => {}
            MIRStatement::Goto { .. } => {}
            MIRStatement::Label { .. } => {}
            MIRStatement::ContinueStatement { .. } => {}
            MIRStatement::BreakStatement { .. } => {}
            MIRStatement::LoopStatement { .. } => {}

            MIRStatement::CreateVariable {
                value: Some(value), ..
            }
            | MIRStatement::SetVariable { value, .. }
            | MIRStatement::IfStatement {
                condition: value, ..
            }
            | MIRStatement::GotoNotEqual {
                condition: value, ..
            } => {
                if !$for_each(value) {
                    return false;
                }
            }

            MIRStatement::FunctionCall(MIRFnCall { source, args, .. }) => {
                if let MIRFnSource::Indirect(expr) = source {
                    if !$for_each(expr) {
                        return false;
                    }
                }

                for arg in args {
                    if !$for_each(arg) {
                        return false;
                    }
                }
            }

            MIRStatement::Return { expr, .. } => {
                if let Some(expr) = expr {
                    if !$for_each(expr) {
                        return false;
                    }
                }
            }
        }

        true
    }};
}

/// Extracts all expressions from a statement
/// and runs the for_each function on them.
pub fn find_exprs<'a, 'b>(
    statement: &'b MIRStatement<'a>,
    for_each: &mut impl FnMut(&'b MIRExpression<'a>) -> bool,
) -> bool {
    extract_expr_body!(statement, for_each)
}

/// Extracts all expressions from a statement
/// and runs the rewrite function on them.
pub fn find_exprs_mut<'a, 'b>(
    statement: &'b mut MIRStatement<'a>,
    rewrite: &mut impl FnMut(&'b mut MIRExpression<'a>) -> bool,
) -> bool {
    extract_expr_body!(statement, rewrite)
}
