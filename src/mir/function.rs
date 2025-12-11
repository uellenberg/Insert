use crate::mir::scope::{Scope, StatementExplorer, explore_expression_mut};
use crate::mir::{
    MIRContext, MIRExpression, MIRExpressionInner, MIRFnCall, MIRFnSource, MIRFunction,
    MIRStatement, MIRType, MIRTypeInner,
};

/// Changes direct calls to indirect calls
/// when a variable with the name is available.
/// This needs to run before type checking, so
/// that type checking can accurately understand
/// a function's source.
pub fn resolve_fns_to_vars<'a>(ctx: &mut MIRContext<'_>) {
    let mut functions = ctx.program.functions.clone();

    for function in functions.values_mut() {
        <StatementExplorer>::explore_block_mut(
            &mut function.body,
            &|statement, scope| {
                match statement {
                    // No expressions.
                    MIRStatement::CreateVariable { value: None, .. } => {}
                    MIRStatement::DropVariable(_, ..) => {}
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
                        resolve_expr_fn_to_vars(ctx, scope, value);
                    }

                    MIRStatement::FunctionCall(fn_call) => {
                        resolve_fn_to_var(ctx, scope, fn_call);
                    }

                    MIRStatement::Return { expr, .. } => {
                        if let Some(expr) = expr {
                            resolve_expr_fn_to_vars(ctx, scope, expr);
                        }
                    }
                }

                true
            },
            &|_, _| true,
            &|_, _| true,
        );
    }
}

/// Changes direct calls to indirect calls
/// when a variable with the name is available.
/// Runs on a single function.
fn resolve_fn_to_var<'a>(ctx: &MIRContext<'a>, scope: &Scope<'a>, fn_call: &mut MIRFnCall<'a>) {
    for arg in &mut fn_call.args {
        resolve_expr_fn_to_vars(ctx, scope, arg);
    }

    if let MIRFnSource::Direct(name, span) = &fn_call.source {
        // Direct function calls are only valid
        // if the name points to a function.
        // If name points to a variable, then
        // it needs to be turned into indirect.
        if scope.get_variable(name).is_some() {
            fn_call.source = MIRFnSource::Indirect(MIRExpression {
                inner: MIRExpressionInner::Variable(name.clone()),
                span: span.clone(),
                ty: None,
            });
        }
    }
}

/// Changes direct function calls to indirect
/// when it points to a variable.
fn resolve_expr_fn_to_vars<'a>(
    ctx: &MIRContext<'a>,
    scope: &Scope<'a>,
    expr: &mut MIRExpression<'a>,
) -> bool {
    explore_expression_mut(expr, &mut |expr| {
        match &mut expr.inner {
            MIRExpressionInner::FunctionCall(fn_data) => {
                resolve_fn_to_var(ctx, scope, &mut *fn_data);
            }
            _ => {}
        }

        true
    })
}

/// Gets the type for a function as a function pointer.
pub fn get_fn_type<'a>(fn_data: &MIRFunction<'a>) -> MIRType<'a> {
    MIRType {
        ty: MIRTypeInner::FunctionPtr(
            fn_data.args.iter().map(|arg| arg.ty.ty.clone()).collect(),
            Box::new(fn_data.ret_ty.ty.clone()),
        ),
        span: Some(fn_data.span.clone()),
    }
}

/// Inserts phantom variables to represent
/// function arguments.
pub fn insert_fn_arg_args(ctx: &mut MIRContext<'_>) {
    for function in ctx.program.functions.values_mut() {
        function.body.splice(
            0..0,
            function
                .args
                .iter()
                .map(|arg| MIRStatement::CreateVariable {
                    var: arg.clone(),
                    value: None,
                    arg: true,
                    span: arg.span.clone(),
                }),
        );
    }
}
