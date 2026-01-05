use crate::mir::interpreter::Interpreter;
use crate::mir::scope::StatementExplorer;
use crate::mir::{
    MIRConstant, MIRContext, MIRExpression, MIRExpressionInner, MIRFnCall, MIRFnSource,
    MIRStatement,
};
use indexmap::IndexMap;
use std::borrow::Cow;

/// Attempts to evaluate all constants and statics, returning
/// whether it was successful.
pub fn const_eval<'a>(ctx: &mut MIRContext<'a>, interpreter: &mut Interpreter<'a>) -> bool {
    let const_names = ctx.program.constants.keys().cloned().collect::<Vec<_>>();
    let static_names = ctx.program.statics.keys().cloned().collect::<Vec<_>>();

    for constant in const_names {
        let Ok(value) = interpreter.eval_const(&constant) else {
            return false;
        };

        ctx.program
            .constants
            .get_mut(&constant)
            .unwrap()
            .value
            .inner = value.into();
    }

    for static_name in static_names {
        let Ok(value) = interpreter.eval_static(&static_name) else {
            return false;
        };

        ctx.program
            .statics
            .get_mut(&static_name)
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
                match statement {
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
                        *value = reduce_expr_simple(&ctx.program.constants, value);
                    }

                    MIRStatement::FunctionCall(MIRFnCall { source, args, .. }) => {
                        if let MIRFnSource::Indirect(expr) = source {
                            *expr = reduce_expr_simple(&ctx.program.constants, expr);
                        }

                        for arg in args {
                            *arg = reduce_expr_simple(&ctx.program.constants, arg);
                        }
                    }

                    MIRStatement::Return { expr, .. } => {
                        if let Some(expr) = expr {
                            *expr = reduce_expr_simple(&ctx.program.constants, expr);
                        }
                    }
                }

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

/// Expression reduction that uses
/// the values inside constants.
/// This MUST be run after constant evaluation.
fn reduce_expr_simple<'a>(
    constants: &IndexMap<Cow<'a, str>, MIRConstant<'a>>,
    expr: &MIRExpression<'a>,
) -> MIRExpression<'a> {
    reduce_expr(expr, &mut |name| {
        // Ensure the constant exists.
        if !constants.contains_key(&name) {
            return None;
        }

        // Constants are guaranteed to already be evaluated.

        // No need to validate that this is a primitive here,
        // since eval_constant already does that.
        Some(constants[&name].value.clone())
    })
}

/// Attempts to reduce an expression
/// using simple constant evaluation.
fn reduce_expr<'a>(
    expr: &MIRExpression<'a>,
    get_const: &mut impl FnMut(Cow<'a, str>) -> Option<MIRExpression<'a>>,
) -> MIRExpression<'a> {
    macro_rules! simple_binary {
        ($left:expr, $right:expr, $($red_i:path)|+, $red_o:path, $op:tt, $ret:path) => {{
            use MIRExpressionInner::*;

            let left = reduce_expr($left, get_const);
            let right = reduce_expr($right, get_const);

            $(if let $red_i(left, ..) = left.inner {
                if let $red_i(right, ..) = right.inner {
                    return $red_o(left $op right);
                }
            })+

            $ret(Box::new(left), Box::new(right))
        }};
    }

    let new_expr = (|| match &expr.inner {
        MIRExpressionInner::Add(left, right) => {
            simple_binary!(left, right, Number, Number, +, Add)
        }
        MIRExpressionInner::Sub(left, right) => {
            simple_binary!(left, right, Number, Number, -, Sub)
        }
        MIRExpressionInner::Mul(left, right) => {
            simple_binary!(left, right, Number, Number, *, Mul)
        }
        MIRExpressionInner::Div(left, right) => {
            simple_binary!(left, right, Number, Number, /, Div)
        }
        MIRExpressionInner::Equal(left, right) => {
            simple_binary!(left, right, Number | Bool, Bool, ==, Equal)
        }
        MIRExpressionInner::NotEqual(left, right) => {
            simple_binary!(left, right, Number | Bool, Bool, !=, NotEqual)
        }
        MIRExpressionInner::Greater(left, right) => {
            simple_binary!(left, right, Number | Bool, Bool, >, Greater)
        }
        MIRExpressionInner::Less(left, right) => {
            simple_binary!(left, right, Number | Bool, Bool, <, Less)
        }
        MIRExpressionInner::GreaterEq(left, right) => {
            simple_binary!(left, right, Number | Bool, Bool, >=, GreaterEq)
        }
        MIRExpressionInner::LessEq(left, right) => {
            simple_binary!(left, right, Number | Bool, Bool, <=, LessEq)
        }
        MIRExpressionInner::BoolAnd(left, right) => {
            simple_binary!(left, right, Bool, Bool, &&, BoolAnd)
        }
        MIRExpressionInner::BoolOr(left, right) => {
            simple_binary!(left, right, Bool, Bool, ||, BoolOr)
        }
        MIRExpressionInner::Number(val) => MIRExpressionInner::Number(*val),
        MIRExpressionInner::String(val) => MIRExpressionInner::String(val.clone()),
        MIRExpressionInner::Bool(val) => MIRExpressionInner::Bool(*val),
        MIRExpressionInner::Variable(name) => get_const(name.clone())
            .map(|v| v.inner)
            .unwrap_or(MIRExpressionInner::Variable(name.clone())),
        MIRExpressionInner::FunctionCall(fn_data) => {
            MIRExpressionInner::FunctionCall(fn_data.clone())
        }
    })();

    MIRExpression {
        inner: new_expr,
        ty: expr.ty.clone(),
        span: expr.span.clone(),
    }
}
