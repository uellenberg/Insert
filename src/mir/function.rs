use crate::mir::expr::{explore_expr, explore_expr_mut, find_exprs, find_exprs_mut};
use crate::mir::scope::{Scope, StatementExplorer};
use crate::mir::{
    MIRContext, MIRDeclarationKey, MIRExpression, MIRExpressionInner, MIRFnCall, MIRFnSource,
    MIRFunction, MIRFunctionArgs, MIRFunctionKey, MIRFunctionType, MIRStatement, MIRType,
    MIRTypeInner, MIRVariable,
};
use crate::parser::span::{Span, eprintln_span};
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
            &mut |statement, scope| {
                match statement {
                    // No expressions.
                    MIRStatement::CreateVariable { value: None, .. } => {}
                    MIRStatement::DropVariable(..) => {}
                    MIRStatement::Goto { .. } => {}
                    MIRStatement::Label { .. } => {}
                    MIRStatement::ContinueStatement { .. } => {}
                    MIRStatement::BreakStatement { .. } => {}
                    MIRStatement::LoopStatement { .. } => {}
                    MIRStatement::MarkerStatement { .. } => {}

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
            &mut |_, _| true,
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
                inner: MIRExpressionInner::Variable(name.clone(), None),
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
                    // arg is already set to true here.
                    var: arg.clone(),
                    value: None,
                    span: arg.span.clone(),
                }),
        );
    }
}

/// Marks all declarations reachable from exported functions:
/// - Helper functions get exported.
/// - Required imports get cataloged (and later emitted).
///
/// Ideally, this should be used as early as possible, to benefit
/// from as many optimizations.
pub fn mark_reachable(ctx: &mut MIRContext<'_>) {
    let mut visited = HashSet::new();
    let mut imports = HashSet::new();

    // Visit all declarations.

    for function in ctx.program.functions.keys() {
        if ctx.program.functions[function].fn_type == MIRFunctionType::Export {
            mark_visited(ctx, function, &mut visited, &mut imports);
        }
    }

    // Update the MIR based on what's reachable.

    for key in visited {
        let func = &mut ctx.program.functions[key];
        if func.fn_type == MIRFunctionType::Helper {
            func.fn_type = MIRFunctionType::Export;
        }
    }

    ctx.program.required_imports.extend(imports);
    ctx.program.required_imports.sort();
}

/// Recursively tracks all functions that are called from
/// the given function, collecting imports along the way.
fn mark_visited<'a>(
    ctx: &MIRContext<'a>,
    func: MIRFunctionKey,
    visited: &mut HashSet<MIRFunctionKey>,
    imports: &mut HashSet<Cow<'a, str>>,
) {
    if visited.contains(&func) {
        return;
    }
    visited.insert(func);

    let function = &ctx.program.functions[func];

    if let Some(import) = &function.extern_import {
        // This extern function is reachable, so we must import it.
        imports.insert(import.clone());
    }

    <StatementExplorer>::explore_block(
        &function.body,
        &mut |statement, _| {
            if let MIRStatement::FunctionCall(MIRFnCall {
                source: MIRFnSource::Direct(name, ..),
                args_ty,
                ..
            }) = statement
            {
                // Statement call.
                mark_visited(ctx, get_fn_key(ctx, name, args_ty), visited, imports);
            } else {
                // TODO: Visit function pointers in expressions.
                find_exprs(statement, &mut |expr, _| {
                    explore_expr(expr, &mut |expr| {
                        if let MIRExpressionInner::FunctionCall(box MIRFnCall {
                            source,
                            args_ty,
                            ..
                        }) = &expr.inner
                            && let MIRFnSource::Direct(name, ..) = source
                        {
                            // Expression call.
                            mark_visited(ctx, get_fn_key(ctx, name, args_ty), visited, imports);
                        }

                        true
                    });

                    true
                });
            }

            true
        },
        &|_, _| true,
        &|_, _| true,
    );
}

/// Copies the source of inline functions directly into their callers.
///
/// Ideally, this should be called early in the process, to allow optimizations further
/// down to have as much information as possible.
///
/// Returns true on success.
pub fn inline_functions(ctx: &mut MIRContext<'_>) -> bool {
    let function_names = ctx.program.functions.keys().collect::<Vec<_>>();

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
    func: MIRFunctionKey,
    visited: &mut HashSet<MIRFunctionKey>,
    inline_var_idx: &mut u32,
) -> bool {
    if visited.contains(&func) {
        eprintln_span!(
            ctx,
            Some(ctx.program.functions[func].span.clone()),
            "Inline cycle detected: {:?}",
            visited
        );
        return false;
    }
    visited.insert(func);

    let mut new_statements = ctx.program.functions[func].body.clone();
    if !<StatementExplorer>::rewrite_block(
        &mut new_statements,
        &mut |mut statement, _scope, block| {
            // Handle calls within expressions.
            // This needs to go first because function call arguments might have
            // inline function calls themselves.
            // For there, we need to add their code before this statement,
            // then substitute the output variable.
            if !find_exprs_mut(&mut statement, &mut |expr, _| {
                explore_expr_mut(expr, &mut |expr| {
                    if let MIRExpressionInner::FunctionCall(box MIRFnCall {
                        source: MIRFnSource::Direct(name, ..),
                        args,
                        args_ty,
                        ..
                    }) = expr.inner.clone()
                    {
                        let key = get_fn_key(ctx, &name, &args_ty);
                        if ctx.program.functions[key].fn_type == MIRFunctionType::Inline {
                            let Ok(mut res) =
                                rewrite_inline_function(ctx, key, &args, visited, inline_var_idx)
                            else {
                                return false;
                            };

                            block.append(&mut res.0);
                            *expr = res.1;
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
                let key = get_fn_key(ctx, &name, &args_ty);
                if ctx.program.functions[key].fn_type == MIRFunctionType::Inline {
                    let Ok(mut res) =
                        rewrite_inline_function(ctx, key, &args, visited, inline_var_idx)
                    else {
                        return false;
                    };

                    // The output expression doesn't matter since we don't use it.
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
        return false;
    }

    ctx.program.functions[func].body = new_statements;

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
    func: MIRFunctionKey,
    args: &[MIRExpression<'a>],
    visited: &mut HashSet<MIRFunctionKey>,
    inline_var_idx: &mut u32,
) -> Result<(Vec<MIRStatement<'a>>, MIRExpression<'a>), ()> {
    // We need to ensure that we're fully resolved first, before inlining ourselves
    // into a parent function.
    if !inline_function(ctx, func, visited, inline_var_idx) {
        return Err(());
    }

    // Original var name -> new var name.
    let mut var_map: HashMap<Cow<'a, str>, Cow<'a, str>> = HashMap::new();
    for arg in &ctx.program.functions[func].args {
        let new_name = format!("$inline_{}", inline_var_idx);
        *inline_var_idx += 1;

        var_map.insert(arg.name.clone(), new_name.into());
    }

    // This needs to be separated, because these variables run from the scope
    // of the parent function and thus must not have their expressions re-written.
    let mut header = vec![];

    // Arg variables.
    for (arg_value, arg_info) in args.iter().zip(ctx.program.functions[func].args.iter()) {
        let new_name = var_map[&arg_info.name].clone();

        header.push(MIRStatement::CreateVariable {
            var: MIRVariable {
                name: new_name,
                // Not a phantom variable - this is a real variable with data!
                arg: false,
                ..arg_info.clone()
            },
            value: Some(arg_value.clone()),
            span: arg_info.span.clone(),
        });
    }

    // Main body.
    let mut body = ctx.program.functions[func].body.clone();
    // The body will have phantom arg variables, but we've just materialized them above,
    // so remove them to avoid duplicates.
    body.retain(|statement| {
        !matches!(
            statement,
            MIRStatement::CreateVariable {
                var: MIRVariable { arg: true, .. },
                ..
            }
        )
    });

    let mut out_expr;

    if let Some(MIRStatement::Return {
        expr: Some(expr), ..
    }) = body.last().cloned()
    {
        // Explicit return with value.
        body.pop();
        out_expr = expr;
    } else {
        // Implicit return or explicit return with no value.
        // This also means the function returns unit.
        out_expr = MIRExpression {
            inner: MIRExpressionInner::Unit,
            ty: Some(MIRType {
                ty: MIRTypeInner::Unit,
                span: None,
            }),
            span: Span::empty(),
        };
    }

    // Rewrite everything to map old variables to new ones.
    if !<StatementExplorer>::rewrite_block(
        &mut body,
        &mut |mut statement, _scope, block| {
            match &mut statement {
                // There shouldn't be any returns anymore.
                // If so, they're invalid, since we can't properly inline functions
                // with complex control flow.
                MIRStatement::Return { span, .. } => {
                    eprintln_span!(
                        ctx,
                        Some(span.clone()),
                        "Return statement found in inline function body!"
                    );
                    return false;
                }

                // We need to rename local variables to avoid conflicts.
                MIRStatement::CreateVariable { var, .. } => {
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
                MIRStatement::DropVariable(name, ..) => {
                    if let Some(new_name) = var_map.get(name) {
                        *name = new_name.clone();
                    }
                }

                _ => {}
            }

            if !find_exprs_mut(&mut statement, &mut |expr, _| {
                remap_expr_vars(expr, &var_map);

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

    // This needs to be remapped separately, since it isn't
    // part of the header/body.
    // It must occur after we rewrite above, so that all the
    // variables have new names assigned in the map.
    remap_expr_vars(&mut out_expr, &var_map);

    Ok((header, out_expr))
}

/// Remaps al inline variables in an expression according to the var map.
fn remap_expr_vars<'a>(
    expr: &mut MIRExpression<'a>,
    var_map: &HashMap<Cow<'a, str>, Cow<'a, str>>,
) {
    explore_expr_mut(expr, &mut |expr| {
        if let MIRExpressionInner::Variable(name, _) = &mut expr.inner
            && let Some(new_name) = var_map.get(name)
        {
            *name = new_name.clone();
        }

        true
    });
}

/// Removes all functions from the final output that aren't marked as export.
pub fn prune_functions(ctx: &mut MIRContext<'_>) {
    ctx.retain(|ctx, key| match key {
        MIRDeclarationKey::Function(func) => {
            ctx.program.functions[*func].fn_type == MIRFunctionType::Export
        }
        _ => true,
    });
}

/// Gets the [MIRFunctionKey] for the given (name, args_ty) combo.
/// Panics if the function doesn't exist.
fn get_fn_key(
    ctx: &MIRContext<'_>,
    name: &str,
    args_ty: &Option<MIRFunctionArgs>,
) -> MIRFunctionKey {
    let call_args = &args_ty
        .as_ref()
        .expect("Function args type didn't exist!")
        .args;

    ctx.program
        .function_names
        .get(name)
        // The function's existence must be verified in type checking.
        .expect("Function name doesn't exist!")
        .find_compatible(call_args)
        .expect("No matching function found (none or ambiguous)!")
}
