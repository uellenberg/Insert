use crate::mir::expr::explore_outer_place;
use crate::mir::function::get_fn_type;
use crate::mir::scope::{Scope, StatementExplorer};
use crate::mir::{
    MIRConstant, MIRContext, MIRExpression, MIRExpressionInner, MIRFnCall, MIRFnSource,
    MIRFunction, MIRFunctionArgs, MIRFunctionKey, MIRStatement, MIRStatic, MIRType, MIRTypeInner,
};
use crate::parser::span::Span;
use ariadne::{ColorGenerator, Fmt, Label, Report, ReportKind};
use std::borrow::Cow;

/// Finds and reports type errors, returning
/// whether type check succeeded.
/// Also modifies the MIR to contain
/// type information.
pub fn type_check(ctx: &mut MIRContext<'_>) -> bool {
    let mut constants = ctx.program.constants.clone();
    let mut statics = ctx.program.statics.clone();
    let mut functions = ctx.program.functions.clone();

    // All of these have two passes to fully resolve types.
    // Both passes pull type information from bottom to top and
    // from top to bottom.
    // In effect, the first pass will propagate type information
    // upwards, and the second pass will push it downwards.
    // This lets us resolve code like:
    // let val: u32 = 1 + 2;
    // Here, the expression 1 + 2 will normally be given the type u32,
    // but we need the downwards pass to give 1 and 2 that type as well.

    for constant in constants.values_mut() {
        if !check_constant(ctx, constant) || !check_constant(ctx, constant) {
            return false;
        }
    }

    for static_data in statics.values_mut() {
        if !check_static(ctx, static_data) || !check_static(ctx, static_data) {
            return false;
        }
    }

    for function in functions.values_mut() {
        if !check_function(ctx, function) || !check_function(ctx, function) {
            return false;
        }
    }

    ctx.program.constants = constants;
    ctx.program.statics = statics;
    ctx.program.functions = functions;

    true
}

/// Prints an error for when an expression
/// returns an unexpected type.
fn print_unexpected_expr_ty(
    ctx: &MIRContext<'_>,
    expected_ty: MIRType<'_>,
    actual_ty: MIRType<'_>,
    error_expr_span: Span<'_>,
) {
    let mut colors = ColorGenerator::new();

    let expected = colors.next();
    let actual = colors.next();

    let expected_ty_str: Cow<str> = expected_ty.ty.clone().into();
    let expected_ty_str = expected_ty_str.fg(expected);

    let actual_ty_str: Cow<str> = actual_ty.ty.clone().into();
    let actual_ty_str = actual_ty_str.fg(actual);

    let mut report = Report::build(ReportKind::Error, error_expr_span.clone()).with_message(
        format!("Expected type {expected_ty_str}, found {actual_ty_str}"),
    );

    if let Some(expected_ty_span) = &expected_ty.span {
        report = report.with_label(
            Label::new(expected_ty_span.clone())
                .with_message(format!("Expected {expected_ty_str} because of this"))
                .with_color(expected),
        )
    }

    report
        .with_label(
            Label::new(error_expr_span)
                .with_message(format!("This expression returns {actual_ty_str}"))
                .with_color(actual),
        )
        .finish()
        .eprint(ctx.file_cache.clone())
        .unwrap();
}

/// Prints an error for when an expression
/// returns an unexpected type.
fn print_var_does_not_exist(ctx: &MIRContext<'_>, var_name: Cow<'_, str>, var_span: Span<'_>) {
    let mut colors = ColorGenerator::new();

    let var_color = colors.next();

    let var_name_str = var_name.fg(var_color);

    Report::build(ReportKind::Error, var_span.clone())
        .with_label(
            Label::new(var_span)
                .with_message("Could not find variable")
                .with_color(var_color),
        )
        .with_message(format!("Variable {var_name_str} does not exist"))
        .with_help("Maybe this variable was defined in a different scope?")
        .finish()
        .eprint(ctx.file_cache.clone())
        .unwrap();
}

/// Checks whether the given constant is valid,
/// and modifies its type information to match.
fn check_constant<'a>(ctx: &MIRContext<'a>, constant: &mut MIRConstant<'a>) -> bool {
    let Some(expr_type) = check_expression(ctx, &mut constant.value, None) else {
        return false;
    };

    if !types_equal(expr_type, &mut constant.ty) {
        print_unexpected_expr_ty(
            ctx,
            constant.ty.clone(),
            expr_type.clone(),
            constant.value.span.clone(),
        );

        return false;
    }

    true
}

/// Checks whether the given static is valid,
/// and modifies its type information to match.
fn check_static<'a>(ctx: &MIRContext<'a>, static_data: &mut MIRStatic<'a>) -> bool {
    let Some(expr_type) = check_expression(ctx, &mut static_data.value, None) else {
        return false;
    };

    if !types_equal(expr_type, &mut static_data.ty) {
        print_unexpected_expr_ty(
            ctx,
            static_data.ty.clone(),
            expr_type.clone(),
            static_data.value.span.clone(),
        );

        return false;
    }

    true
}

/// Checks whether the given function is valid,
/// and modifies its type information to match.
fn check_function<'a>(ctx: &MIRContext<'a>, function: &mut MIRFunction<'a>) -> bool {
    // TODO: Check return types.

    <StatementExplorer>::explore_block_mut(
        &mut function.body,
        &|statement, scope| {
            match statement {
                // No expressions.
                MIRStatement::DropVariable(..) => {}
                MIRStatement::Goto { .. } => {}
                MIRStatement::Label { .. } => {}
                MIRStatement::ContinueStatement { .. } => {}
                MIRStatement::BreakStatement { .. } => {}
                MIRStatement::LoopStatement { .. } => {}

                MIRStatement::CreateVariable {
                    var, value, arg, ..
                } => {
                    // Disallow shadowing.
                    // (Phantom) arg variables shouldn't get checked against locals, since
                    // they might be added to the scope automatically.
                    if (!*arg && scope.get_variable(&var.name).is_some())
                        || ctx.program.static_names.contains_key(&var.name)
                        || ctx.program.const_names.contains_key(&var.name)
                    {
                        eprintln!("Cannot shadow existing variable {}", var.name);
                        return false;
                    }

                    if let Some(value) = value {
                        let Some(ty) = check_expression(ctx, value, Some(scope)) else {
                            return false;
                        };

                        if !types_equal(ty, &mut var.ty) {
                            print_unexpected_expr_ty(
                                ctx,
                                var.ty.clone(),
                                ty.clone(),
                                value.span.clone(),
                            );

                            return false;
                        }
                    }
                }

                MIRStatement::SetVariable { value, place, .. } => {
                    // Make sure we aren't trying to modify a const.
                    // If a const appears inside a place expression (e.g., a[const]), then
                    // we aren't modifying the const.
                    // If it appears outside (e.g., const[a]), then we are, so should error.
                    if !explore_outer_place(place, &mut |expr| {
                        if let MIRExpressionInner::Variable(var) = &expr.inner
                            && ctx.program.const_names.contains_key(var)
                        {
                            eprintln!("Cannot set constants!");
                            return false;
                        }

                        true
                    }) {
                        return false;
                    }

                    // This will handle the types of locals/statics the same way as normal expressions,
                    // which works for our purposes here.
                    let Some(var_ty) = check_expression(ctx, place, Some(scope)) else {
                        return false;
                    };

                    let Some(ty) = check_expression(ctx, value, Some(scope)) else {
                        return false;
                    };

                    if !types_equal(ty, var_ty) {
                        print_unexpected_expr_ty(
                            ctx,
                            var_ty.clone(),
                            ty.clone(),
                            value.span.clone(),
                        );

                        return false;
                    }
                }

                MIRStatement::FunctionCall(MIRFnCall {
                    source,
                    args,
                    args_ty,
                    ret_ty,
                    span,
                    ..
                }) => {
                    if !check_fn_call(ctx, Some(scope), source, args, args_ty, ret_ty, span) {
                        return false;
                    }
                }

                MIRStatement::IfStatement {
                    condition, span, ..
                }
                | MIRStatement::GotoNotEqual {
                    condition, span, ..
                } => {
                    let Some(cond_ty) = check_expression(ctx, condition, Some(scope)) else {
                        return false;
                    };

                    // No need for type_equal since bool can't resolve numbers.
                    if cond_ty.ty != MIRTypeInner::Bool {
                        print_unexpected_expr_ty(
                            ctx,
                            MIRType {
                                ty: MIRTypeInner::Bool,
                                span: Some(span.clone()),
                            },
                            cond_ty.clone(),
                            condition.span.clone(),
                        );
                        return false;
                    }
                }

                MIRStatement::Return { expr, span, .. } => match expr {
                    Some(expr) => {
                        let Some(cond_ty) = check_expression(ctx, expr, Some(scope)) else {
                            return false;
                        };

                        if !types_equal(cond_ty, &mut function.ret_ty.clone()) {
                            print_unexpected_expr_ty(
                                ctx,
                                function.ret_ty.clone(),
                                cond_ty.clone(),
                                expr.span.clone(),
                            );

                            return false;
                        }
                    }
                    None => {
                        // No need for type_equal since unit can't resolve numbers.
                        if function.ret_ty.ty != MIRTypeInner::Unit {
                            print_unexpected_expr_ty(
                                ctx,
                                function.ret_ty.clone(),
                                MIRType {
                                    ty: MIRTypeInner::Unit,
                                    span: Some(span.clone()),
                                },
                                span.clone(),
                            );

                            return false;
                        }
                    }
                },
            }

            true
        },
        &|_, _| true,
        &|_, _| true,
    )
}

/// Checks the validity of a function call,
/// assigning a value to the function's return
/// type.
fn check_fn_call<'a>(
    ctx: &MIRContext<'a>,
    scope: Option<&Scope<'a>>,
    source: &mut MIRFnSource<'a>,
    args: &mut Vec<MIRExpression<'a>>,
    out_args_ty: &mut Option<MIRFunctionArgs<'a>>,
    out_ret_ty: &mut Option<MIRType<'a>>,
    span: &Span<'a>,
) -> bool {
    // Add type information to arguments.
    for arg in args.iter_mut() {
        if check_expression(ctx, arg, scope).is_none() {
            return false;
        }
    }

    let mut expected_ty = match source {
        MIRFnSource::Direct(name, _span) => {
            let args_ty = args
                .iter()
                .map(|arg| {
                    arg.ty
                        .as_ref()
                        .expect("Function argument didn't have type info!")
                        .ty
                        .clone()
                })
                .collect::<Vec<_>>();

            // Ensure there's no ambiguity in which overloaded function we're trying to call.
            // This makes it possible for new functions to be breaking changes, but that's more
            // predictable than picking one at random.
            let Some(candidate) = get_fn_candidate(ctx, name, &args_ty) else {
                // Error already printed by get_fn_candidate.
                return false;
            };

            get_fn_type(&ctx.program.functions[candidate])
        }
        MIRFnSource::Indirect(expr) => {
            let Some(ty) = check_expression(ctx, expr, scope) else {
                return false;
            };

            ty.clone()
        }
    };

    // This needs to be a reference to the real type stored
    // in the expression to ensure that any updates are properly
    // saved.
    let mut actual_args = args
        .iter_mut()
        .map(|arg| {
            arg.ty
                .as_mut()
                .expect("Function argument didn't have type info!")
        })
        .collect::<Vec<_>>();

    let mut actual_ty = MIRType {
        ty: MIRTypeInner::FunctionPtr(
            MIRFunctionArgs(actual_args.iter().map(|arg| arg.ty.clone()).collect()),
            // Default to unit type for error messages
            // when unmatched function, since we only
            // know the return type once we have a valid
            // function.
            Box::new(MIRTypeInner::Unit),
        ),
        span: None,
    };

    // Ensure that we have a function type.
    let MIRTypeInner::FunctionPtr(expected_args, expected_ret_ty) = &mut expected_ty.ty else {
        print_unexpected_expr_ty(ctx, expected_ty, actual_ty, span.clone());
        return false;
    };
    let mut expected_args = expected_args.clone();
    let expected_ret_ty = (**expected_ret_ty).clone();

    // Give actual_ty the correct return type.
    // We have more complete info in actual_args, so
    // no need to extract it here (_).
    let MIRTypeInner::FunctionPtr(_, actual_ret_ty) = &mut actual_ty.ty else {
        unreachable!();
    };
    **actual_ret_ty = expected_ret_ty.clone();

    // Ensure that both function types have the
    // same arg length.
    if actual_args.len() != expected_args.0.len() {
        print_unexpected_expr_ty(ctx, expected_ty, actual_ty, span.clone());
        return false;
    }

    // Ensure that individual arg types match,
    // for more granular errors.
    for (actual, expected) in actual_args.iter_mut().zip(expected_args.0.iter_mut()) {
        if !types_equal_inner(&mut actual.ty, expected) {
            print_unexpected_expr_ty(
                ctx,
                MIRType {
                    ty: expected.clone(),
                    span: expected_ty.span.clone(),
                },
                actual.clone(),
                actual.span.clone().unwrap_or_else(|| span.clone()),
            );
            return false;
        }
    }

    // Ensure that actual matches expected.
    if !types_equal(&mut expected_ty, &mut actual_ty) {
        print_unexpected_expr_ty(ctx, expected_ty, actual_ty, span.clone());
        return false;
    }

    // Store the computed types.
    *out_ret_ty = Some(MIRType {
        ty: expected_ret_ty,
        span: expected_ty.span,
    });
    *out_args_ty = Some(expected_args);

    true
}

/// Prints an error for when an expression
/// requires left and right operands to
/// be equal, but they aren't.
fn print_left_right_unequal(
    ctx: &MIRContext<'_>,
    op_name: &str,
    left_ty: MIRType<'_>,
    right_ty: MIRType<'_>,
    error_expr_span: Span<'_>,
) {
    let mut colors = ColorGenerator::new();

    let left = colors.next();
    let right = colors.next();

    let left_ty_str: Cow<str> = left_ty.ty.clone().into();
    let left_ty_str = left_ty_str.fg(left);

    let right_ty_str: Cow<str> = right_ty.ty.clone().into();
    let right_ty_str = right_ty_str.fg(right);

    let mut report = Report::build(ReportKind::Error, error_expr_span.clone())
        .with_message("Left and right operands have different types".to_string());

    if let Some(left_ty_span) = &left_ty.span {
        report = report.with_label(
            Label::new(left_ty_span.clone())
                .with_message(format!("This expression has type {left_ty_str}"))
                .with_color(left),
        )
    }

    if let Some(right_ty_span) = &right_ty.span {
        report = report.with_label(
            Label::new(right_ty_span.clone())
                .with_message(format!("This expression has type {right_ty_str}"))
                .with_color(right),
        )
    }

    report
        .with_note(format!(
            "{op_name} requires the left and right operands to have the same type."
        ))
        .finish()
        .eprint(ctx.file_cache.clone())
        .unwrap();
}

/// If ty1 == ty2, returns true, otherwise false.
///
/// This correctly resolves number types, so
/// if UnknownNumber can be resolved, it will be.
/// After calling this function, it is guaranteed that
/// ty1 == ty2.
fn types_equal<'a>(ty1: &mut MIRType<'a>, ty2: &mut MIRType<'a>) -> bool {
    types_equal_inner(&mut ty1.ty, &mut ty2.ty)
}

/// This is the same as [types_equal] except for inner types.
fn types_equal_inner<'a>(ty1: &mut MIRTypeInner<'a>, ty2: &mut MIRTypeInner<'a>) -> bool {
    if ty1 == ty2 {
        return true;
    }

    match (ty1, ty2) {
        (to @ MIRTypeInner::UnknownNumber, from @ (MIRTypeInner::I32 | MIRTypeInner::U32))
        | (from @ (MIRTypeInner::I32 | MIRTypeInner::U32), to @ MIRTypeInner::UnknownNumber) => {
            *to = from.clone();

            true
        }
        // Recursive types need special handling to fully resolve.
        (MIRTypeInner::FunctionPtr(args1, ret1), MIRTypeInner::FunctionPtr(args2, ret2)) => {
            if args1.0.len() != args2.0.len() {
                return false;
            }

            // We need to be careful here, since we don't want to
            // actually modify the types unless they fully match.
            let mut new_ret1 = (**ret1).clone();
            let mut new_ret2 = (**ret2).clone();

            if !types_equal_inner(&mut new_ret1, &mut new_ret2) {
                return false;
            }

            let mut new_args1 = args1.clone();
            let mut new_args2 = args2.clone();

            for (arg1, arg2) in new_args1.0.iter_mut().zip(new_args2.0.iter_mut()) {
                if !types_equal_inner(arg1, arg2) {
                    return false;
                }
            }

            // The types are equal, so we can update them.
            **ret1 = new_ret1;
            **ret2 = new_ret2;

            *args1 = new_args1;
            *args2 = new_args2;

            true
        }
        _ => false,
    }
}

/// Tries to find a function that matches the given name and arguments.
/// If none exists or it's ambiguous, it prints out an error and returns None.
fn get_fn_candidate<'a>(
    ctx: &MIRContext<'a>,
    name: &str,
    args: &[MIRTypeInner<'a>],
) -> Option<MIRFunctionKey> {
    let mut res = ctx
        .program
        .function_names
        .get(name)
        .into_iter()
        .flat_map(|v| v.values())
        .filter(|func| {
            let func = &ctx.program.functions[**func];

            if args.len() != func.args.len() {
                return false;
            }

            for (arg1, arg2) in args.iter().zip(func.args_ty.0.iter()) {
                if arg1 == arg2 {
                    continue;
                }

                match (arg1, arg2) {
                    // Allow unknown numbers to resolve, but only if there's
                    // no ambiguity (we'll check that below).
                    (MIRTypeInner::UnknownNumber, MIRTypeInner::I32 | MIRTypeInner::U32)
                    | (MIRTypeInner::I32 | MIRTypeInner::U32, MIRTypeInner::UnknownNumber) => {
                        continue;
                    }
                    _ => return false,
                }
            }

            true
        });

    let Some(candidate) = res.next() else {
        // No candidates.
        // TODO: No function found error.
        eprintln!("No function found with name {name:?}");
        return None;
    };

    if res.next().is_some() {
        // Multiple candidates = ambiguity!
        // TODO: Multiple functions found error.
        eprintln!(
            "Multiple functions found with name {name:?}. Disambiguate arguments with type annotations."
        );
        return None;
    }

    // Only one candidate found.
    Some(*candidate)
}

/// Checks whether the expression is valid,
/// and modifies its type information to match.
/// If it isn't, errors are reported.
/// If it is, the expression's type is returned.
fn check_expression<'a, 'b>(
    ctx: &MIRContext<'a>,
    expr: &'b mut MIRExpression<'a>,
    scope: Option<&Scope<'a>>,
) -> Option<&'b mut MIRType<'a>> {
    macro_rules! simple_binary {
        ($left:expr, $right:expr, $name:literal, $inherit_ty:expr, internal) => {{
            // If we have inherit_ty, that means the expression's type should
            // equal the left and right operands.
            // This lets us propagate type information downwards, which is
            // useful if the parent expression has context that inner one doesn't.
            //
            // This is done before the recursive step to allow it to fully propagate
            // upwards in one pass. To be used effectively, we still need to run
            // check_expression twice: once to propagate upwards and once to propagate
            // downwards.
            if let Some(inherit_ty) = $inherit_ty {
                if let Some(left_ty) = &mut $left.ty
                    && !types_equal(inherit_ty, left_ty)
                {
                    print_unexpected_expr_ty(
                        ctx,
                        inherit_ty.clone(),
                        left_ty.clone(),
                        $left.span.clone(),
                    );
                    return None;
                }

                if let Some(right_ty) = &mut $right.ty
                    && !types_equal(inherit_ty, right_ty)
                {
                    print_unexpected_expr_ty(
                        ctx,
                        inherit_ty.clone(),
                        right_ty.clone(),
                        $right.span.clone(),
                    );
                    return None;
                }
            }

            let t_left = check_expression(ctx, $left, scope)?;
            let t_right = check_expression(ctx, $right, scope)?;

            if !types_equal(t_left, t_right) {
                print_left_right_unequal(
                    ctx,
                    $name,
                    t_left.clone(),
                    t_right.clone(),
                    expr.span.clone(),
                );
                return None;
            }

            // Left vs right doesn't matter.
            Some(t_left.clone())
        }};
        ($left:expr, $right:expr, $name:literal) => {
            simple_binary!($left, $right, $name, &mut expr.ty, internal)
        };
        ($left:expr, $right:expr, $name:literal, $ty:expr) => {{
            // Types don't get pushed downwards from here (i.e., parent type
            // has no significance to the children).
            simple_binary!($left, $right, $name, &mut None, internal);

            Some(MIRType {
                ty: $ty,
                // Span will get set below.
                span: None,
            })
        }};
    }

    let mut ty = (|| {
        match &mut expr.inner {
            MIRExpressionInner::Add(left, right, ..) => {
                simple_binary!(left, right, "Addition")
            }
            MIRExpressionInner::Sub(left, right, ..) => {
                simple_binary!(left, right, "Subtraction")
            }
            MIRExpressionInner::Mul(left, right, ..) => {
                simple_binary!(left, right, "Multiplication")
            }
            MIRExpressionInner::Div(left, right, ..) => {
                simple_binary!(left, right, "Division")
            }
            MIRExpressionInner::Equal(left, right, ..) => {
                simple_binary!(left, right, "Equals", MIRTypeInner::Bool)
            }
            MIRExpressionInner::NotEqual(left, right, ..) => {
                simple_binary!(left, right, "Not equals", MIRTypeInner::Bool)
            }
            MIRExpressionInner::Less(left, right, ..) => {
                simple_binary!(left, right, "Less than", MIRTypeInner::Bool)
            }
            MIRExpressionInner::Greater(left, right, ..) => {
                simple_binary!(left, right, "Greater than", MIRTypeInner::Bool)
            }
            MIRExpressionInner::LessEq(left, right, ..) => {
                simple_binary!(left, right, "Less than or equals", MIRTypeInner::Bool)
            }
            MIRExpressionInner::GreaterEq(left, right, ..) => {
                simple_binary!(left, right, "Greater than or equals", MIRTypeInner::Bool)
            }
            MIRExpressionInner::BoolAnd(left, right, ..) => {
                simple_binary!(left, right, "Binary and", MIRTypeInner::Bool)
            }
            MIRExpressionInner::BoolOr(left, right, ..) => {
                simple_binary!(left, right, "Binary or", MIRTypeInner::Bool)
            }
            MIRExpressionInner::Variable(name, ..) => {
                if let Some(scope) = scope
                    && let Some(var) = scope.get_variable(name)
                {
                    return Some(var.ty.clone());
                }

                if let Some(var) = ctx.program.const_names.get(name) {
                    return Some(ctx.program.constants[*var].ty.clone());
                }

                if let Some(var) = ctx.program.static_names.get(name) {
                    return Some(ctx.program.statics[*var].ty.clone());
                }

                if ctx
                    .program
                    .function_names
                    .get(name)
                    .is_some_and(|v| !v.is_empty())
                {
                    eprintln!(
                        "Cannot directly access function as value (use a reference): {expr:?}"
                    );
                    return None;
                }

                print_var_does_not_exist(ctx, name.clone(), expr.span.clone());
                None
            }
            MIRExpressionInner::FunctionCall(fn_data) => {
                if !check_fn_call(
                    ctx,
                    scope,
                    &mut fn_data.source,
                    &mut fn_data.args,
                    &mut fn_data.args_ty,
                    &mut fn_data.ret_ty,
                    &fn_data.span,
                ) {
                    return None;
                }

                Some(
                    fn_data
                        .ret_ty
                        .clone()
                        .expect("Function was not given a return type!"),
                )
            }
            MIRExpressionInner::Number(val) => {
                Some(MIRType {
                    ty: if *val < 0 {
                        // Negative numbers must be signed.
                        MIRTypeInner::I32
                    } else if *val > i32::MAX as i128 {
                        // Overflowing numbers must be unsigned.
                        MIRTypeInner::U32
                    } else {
                        MIRTypeInner::UnknownNumber
                    },
                    // Span is added after.
                    span: None,
                })
            }
            MIRExpressionInner::String(_) => Some(MIRType {
                ty: MIRTypeInner::String,
                // Span is added after.
                span: None,
            }),
            MIRExpressionInner::Bool(_) => Some(MIRType {
                ty: MIRTypeInner::Bool,
                // Span is added after.
                span: None,
            }),
            MIRExpressionInner::Unit => Some(MIRType {
                ty: MIRTypeInner::Unit,
                // Span is added after.
                span: None,
            }),

            // TODO: Implement type checking for place expressions.
            MIRExpressionInner::Ref(_)
            | MIRExpressionInner::Deref(_)
            | MIRExpressionInner::Member(_, _)
            | MIRExpressionInner::Index(_, _) => todo!(),
        }
    })()?;

    // Ensure the type covers the
    // whole span.
    ty.span = Some(expr.span.clone());

    // If the expression already has a type, it cannot disagree with itself.
    // This is used for, e.g., throwing an error if "-10u32" is written, and
    // propagating the explicitly written type if it is valid.
    if let Some(existing_ty) = &mut expr.ty
        && !types_equal(existing_ty, &mut ty)
    {
        print_unexpected_expr_ty(ctx, existing_ty.clone(), ty.clone(), expr.span.clone());
        return None;
    }

    // Save the type for later
    // phases.
    expr.ty = Some(ty);

    // Ensure that we return a type
    // whose span covers the entire expression.
    expr.ty.as_mut()
}
