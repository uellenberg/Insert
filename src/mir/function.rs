use crate::mir::expr::{explore_expr, explore_expr_mut, find_exprs, find_exprs_mut};
use crate::mir::scope::{Scope, StatementExplorer};
use crate::mir::{
    MIRContext, MIRExpression, MIRExpressionInner, MIRFnCall, MIRFnSource, MIRFunction,
    MIRFunctionKey, MIRFunctionType, MIRStatement, MIRType, MIRTypeInner, MIRVariable,
};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

/// Changes direct calls to indirect calls
/// when a variable with the name is available.
/// This needs to run before type checking, so
/// that type checking can accurately understand
/// a function's source.
pub fn resolve_fns_to_vars(ctx: &mut MIRContext<'_>) {
    let mut functions = ctx.program.functions.clone();

    for function in functions.values_mut() {
        <StatementExplorer>::explore_block_mut(
            &mut function.body,
            &|statement, scope| {
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
    explore_expr_mut(expr, &mut |expr| {
        if let MIRExpressionInner::FunctionCall(fn_data) = &mut expr.inner {
            resolve_fn_to_var(ctx, scope, &mut *fn_data);
        }

        true
    })
}

/// Gets the type for a function as a function pointer.
pub fn get_fn_type<'a>(fn_data: &MIRFunction<'a>) -> MIRType<'a> {
    MIRType {
        ty: MIRTypeInner::FunctionPtr(fn_data.args_ty.clone(), Box::new(fn_data.ret_ty.ty.clone())),
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

/// Turns all used helpers into export functions.
/// A helper is considered used if there's some path between
/// it and an exported function, so static/const initial values
/// aren't counted.
///
/// Ideally, this should be used as early as possible, to benefit
/// from as many optimizations.
pub fn export_helpers(ctx: &mut MIRContext<'_>) {
    let mut visited = HashSet::new();
    for function in ctx.program.functions.keys().cloned() {
        if ctx.program.functions[&function].fn_type == MIRFunctionType::Export {
            mark_visited(ctx, function, &mut visited);
        }
    }

    for name in visited {
        let func = &mut ctx.program.functions[&name];
        if func.fn_type == MIRFunctionType::Helper {
            func.fn_type = MIRFunctionType::Export;
        }
    }
}

/// Recursively tracks all functions that are called from
/// the given function.
fn mark_visited<'a>(
    ctx: &MIRContext<'a>,
    func: MIRFunctionKey<'a>,
    visited: &mut HashSet<MIRFunctionKey<'a>>,
) {
    if visited.contains(&func) {
        return;
    }
    visited.insert(func.clone());

    for statement in &ctx.program.functions[&func].body {
        if let MIRStatement::FunctionCall(MIRFnCall {
            source: MIRFnSource::Direct(name, ..),
            args_ty,
            ..
        }) = statement
        {
            // Statement call.
            mark_visited(
                ctx,
                MIRFunctionKey(
                    name.clone(),
                    // args_ty must be resolved from type checking.
                    args_ty.clone().expect("Functions args type didn't exist!"),
                ),
                visited,
            );
        } else {
            // TODO: Visit function pointers in expressions.
            find_exprs(statement, &mut |expr| {
                explore_expr(expr, &mut |expr| {
                    if let MIRExpressionInner::FunctionCall(box MIRFnCall {
                        source, args_ty, ..
                    }) = &expr.inner
                    {
                        if let MIRFnSource::Direct(name, ..) = source {
                            // Expression call.
                            mark_visited(
                                ctx,
                                MIRFunctionKey(
                                    name.clone(),
                                    // args_ty must be resolved from type checking.
                                    args_ty.clone().expect("Functions args type didn't exist!"),
                                ),
                                visited,
                            );
                        }
                    }

                    true
                });

                true
            });
        }
    }
}

/// Copies the source of inline functions directly into their callers.
///
/// Ideally, this should be called early in the process, to allow optimizations further
/// down to have as much information as possible.
///
/// Returns true on success.
pub fn inline_functions(ctx: &mut MIRContext<'_>) -> bool {
    let function_names = ctx.program.functions.keys().cloned().collect::<Vec<_>>();

    let mut inline_var_idx = 0;

    for func in function_names {
        if !inline_function(ctx, func, &mut HashSet::new(), &mut inline_var_idx) {
            return false;
        }
    }

    true
}

/// Inlines all the calls to inline functions within this function.
/// Returns true on success.
///
/// visited must be an empty HashSet when this is called initially.
/// It should only be used internally by this function and rewrite_inline_function.
///
/// inline_var_idx must maintain its state across the whole compiler pipeline.
fn inline_function<'a>(
    ctx: &mut MIRContext<'a>,
    func: MIRFunctionKey<'a>,
    visited: &mut HashSet<MIRFunctionKey<'a>>,
    inline_var_idx: &mut u32,
) -> bool {
    if visited.contains(&func) {
        eprintln!("Inline cycle detected: {:?}", visited);
        return false;
    }
    visited.insert(func.clone());

    let mut new_statements = ctx.program.functions[&func].body.clone();
    if !<StatementExplorer>::rewrite_block(
        &mut new_statements,
        &mut |mut statement, _scope, block| {
            // Handle calls within expressions.
            // This needs to go first because function call arguments might have
            // inline function calls themselves.
            // For there, we need to add their code before this statement,
            // then substitute the output variable.
            if !find_exprs_mut(&mut statement, &mut |expr| {
                explore_expr_mut(expr, &mut |expr| {
                    if let MIRExpressionInner::FunctionCall(box MIRFnCall {
                        source: MIRFnSource::Direct(name, ..),
                        args,
                        args_ty,
                        ..
                    }) = expr.inner.clone()
                    {
                        // args_ty must be resolved from type checking.
                        let key = MIRFunctionKey(
                            name,
                            args_ty.expect("Functions args type didn't exist!"),
                        );
                        if ctx.program.functions[&key].fn_type == MIRFunctionType::Inline {
                            let Ok(mut res) =
                                rewrite_inline_function(ctx, key, &args, visited, inline_var_idx)
                            else {
                                return false;
                            };

                            block.append(&mut res.0);
                            expr.inner = MIRExpressionInner::Variable(res.1);
                        }
                    }

                    true
                })
            }) {
                return false;
            }

            // If this is a statement function call, we can directly replace it.
            // Inline functions can't be taken as references, so are always called directly.
            // TODO: Disallow taking function references to inline functions.
            if let MIRStatement::FunctionCall(MIRFnCall {
                source: MIRFnSource::Direct(name, ..),
                args,
                args_ty,
                ..
            }) = statement.clone()
            {
                // args_ty must be resolved from type checking.
                let key = MIRFunctionKey(name, args_ty.expect("Functions args type didn't exist!"));
                if ctx.program.functions[&key].fn_type == MIRFunctionType::Inline {
                    let Ok(mut res) =
                        rewrite_inline_function(ctx, key, &args, visited, inline_var_idx)
                    else {
                        return false;
                    };

                    // The output variable doesn't matter since we don't use it.
                    block.append(&mut res.0);

                    // No pushing statement, since we don't need it anymore.
                    return true;
                }
            }

            block.push(statement);
            true
        },
        &mut |_, _| true,
        &|_, _, _| true,
    ) {
        panic!("inline_function rewrite returned false!");
    }

    ctx.program.functions[&func].body = new_statements;

    // Allow multiple calls of the same function.
    visited.remove(&func);

    true
}

/// Rewrites an inline function so that it can be inserted into the body of another function.
/// The result is a list of statements that have to replace the current statement (statement function call)
/// or go above it (expression function call), plus the name of the output variable.
///
/// Returns Err on failure.
fn rewrite_inline_function<'a>(
    ctx: &mut MIRContext<'a>,
    func: MIRFunctionKey<'a>,
    args: &[MIRExpression<'a>],
    visited: &mut HashSet<MIRFunctionKey<'a>>,
    inline_var_idx: &mut u32,
) -> Result<(Vec<MIRStatement<'a>>, Cow<'a, str>), ()> {
    // We need to ensure that we're fully resolved first, before inlining ourselves
    // into a parent function.
    if !inline_function(ctx, func.clone(), visited, inline_var_idx) {
        return Err(());
    }

    // Original var name -> new var name.
    let mut var_map: HashMap<Cow<'a, str>, Cow<'a, str>> = HashMap::new();
    for arg in &ctx.program.functions[&func].args {
        let new_name = format!("$inline_{}", inline_var_idx);
        *inline_var_idx += 1;

        var_map.insert(arg.name.clone(), new_name.into());
    }

    // This needs to be separated, because these variables run from the scope
    // of the parent function and thus must not have their expressions re-written.
    let mut header = vec![];

    // Arg variables.
    for (arg_value, arg_info) in args.iter().zip(ctx.program.functions[&func].args.iter()) {
        let new_name = var_map[&arg_info.name].clone();

        header.push(MIRStatement::CreateVariable {
            var: MIRVariable {
                name: new_name,
                ..arg_info.clone()
            },
            value: Some(arg_value.clone()),
            // Not a phantom variable - this is a real variable with data!
            arg: false,
            span: arg_info.span.clone(),
        });
    }

    // Main body.
    let mut body = ctx.program.functions[&func].body.clone();

    // Output variable.
    let output_var = format!("$inline_{}", inline_var_idx);
    *inline_var_idx += 1;

    // The only return will be in the last statement.
    // If there's no return there, then it's implicit.
    let output_var_info = MIRVariable {
        name: output_var.clone().into(),
        ty: ctx.program.functions[&func].ret_ty.clone(),
        span: ctx.program.functions[&func].span.clone(),
    };

    if let Some(MIRStatement::Return { expr, span }) = body.last().cloned() {
        // Explicit return.
        body.pop();
        body.push(MIRStatement::CreateVariable {
            var: output_var_info,
            value: expr,
            arg: false,
            span,
        });
    } else {
        // Implicit return.
        // This also means the function returns unit.
        body.push(MIRStatement::CreateVariable {
            value: Some(MIRExpression {
                inner: MIRExpressionInner::Unit,
                ty: Some(ctx.program.functions[&func].ret_ty.clone()),
                span: output_var_info.span.clone(),
            }),
            arg: false,
            span: output_var_info.span.clone(),
            var: output_var_info,
        });
    }

    // Rewrite everything to map old variables to new ones.
    if !<StatementExplorer>::rewrite_block(
        &mut body,
        &mut |mut statement, _scope, block| {
            match &mut statement {
                // There shouldn't be any returns anymore.
                // If so, they're invalid, since we can't properly inline functions
                // with complex control flow.
                MIRStatement::Return { .. } => {
                    eprintln!("Return statement found in inline function body!");
                    return false;
                }

                // We need to rename local variables to avoid conflicts.
                MIRStatement::CreateVariable { var, .. } => {
                    // We'll see the output_var which was injected above, and must not
                    // rewrites name.
                    // We should still explore its expressions though.
                    if var.name != output_var {
                        // This is for robustness, e.g., phantom variables might be inserted
                        // for args, and in general, it will always be safe to map the same
                        // name to a different name, unique to the original (even with shadowing).
                        if let Some(new_name) = var_map.get(&var.name) {
                            var.name = new_name.clone();
                        } else {
                            let new_name = format!("$inline_{}", inline_var_idx);
                            *inline_var_idx += 1;

                            var_map.insert(var.name.clone(), new_name.clone().into());
                            var.name = new_name.into();
                        }
                    }
                }
                MIRStatement::SetVariable { place, .. } => {
                    explore_expr_mut(place, &mut |expr| {
                        if let MIRExpressionInner::Variable(name) = &mut expr.inner {
                            if let Some(new_name) = var_map.get(name) {
                                *name = new_name.clone();
                            }
                        }

                        true
                    });
                }
                MIRStatement::DropVariable(name, ..) => {
                    if let Some(new_name) = var_map.get(name) {
                        *name = new_name.clone();
                    }
                }

                _ => {}
            }

            if !find_exprs_mut(&mut statement, &mut |expr| {
                explore_expr_mut(expr, &mut |expr| {
                    if let MIRExpressionInner::Variable(name) = &mut expr.inner {
                        if let Some(new_name) = var_map.get(name) {
                            *name = new_name.clone();
                        }
                    }

                    true
                });

                true
            }) {
                return false;
            }

            block.push(statement);
            true
        },
        &mut |_, _| true,
        &|_, _, _| true,
    ) {
        return Err(());
    }

    // Now, header and body need to be merged together
    // to form the final output for the function.
    header.append(&mut body);

    Ok((header, output_var.into()))
}

/// Removes all functions that aren't marked as export.
pub fn export_functions(ctx: &mut MIRContext<'_>) {
    ctx.program
        .functions
        .retain(|_, func| func.fn_type == MIRFunctionType::Export);
}
