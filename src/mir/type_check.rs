use crate::mir::expr::explore_outer_place;
use crate::mir::function::get_fn_type;
use crate::mir::scope::{Scope, StatementExplorer};
use crate::mir::{
    MIRConstant, MIRContext, MIRExpression, MIRExpressionInner, MIRFnCall, MIRFnSource,
    MIRFunction, MIRFunctionArgs, MIRFunctionKey, MIRStatement, MIRStatic, MIRType, MIRTypeInner,
};
use crate::parser::file_cache::file_cache;
use crate::parser::span::{Span, eprintln_span};
use crate::targets::Target;
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

    // All of these push type information towards children, then base their
    // final type on the child, and error on a discrepancy between what their
    // parent said and what the child said.
    // This allows it to all be completed in one pass.

    for constant in constants.values_mut() {
        if !check_constant(ctx, constant) {
            return false;
        }
    }

    for static_data in statics.values_mut() {
        if !check_static(ctx, static_data) {
            return false;
        }
    }

    for function in functions.values_mut() {
        if !check_function(ctx, function) {
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
        .eprint(file_cache())
        .unwrap();
}

/// Prints an error for when an expression
/// returns an unexpected type.
fn print_var_does_not_exist(var_name: Cow<'_, str>, var_span: Span<'_>) {
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
        .eprint(file_cache())
        .unwrap();
}

/// Checks whether the given constant is valid,
/// and modifies its type information to match.
fn check_constant<'a>(ctx: &MIRContext<'a>, constant: &mut MIRConstant<'a>) -> bool {
    constant.value.ty = Some(constant.ty.clone());
    if check_expression(ctx, &mut constant.value, None).is_none() {
        return false;
    }

    true
}

/// Checks whether the given static is valid,
/// and modifies its type information to match.
fn check_static<'a>(ctx: &MIRContext<'a>, static_data: &mut MIRStatic<'a>) -> bool {
    static_data.value.ty = Some(static_data.ty.clone());
    if check_expression(ctx, &mut static_data.value, None).is_none() {
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
        &mut |statement, scope| {
            match statement {
                // No expressions.
                MIRStatement::DropVariable(..) => {}
                MIRStatement::Goto { .. } => {}
                MIRStatement::Label { .. } => {}
                MIRStatement::ContinueStatement { .. } => {}
                MIRStatement::BreakStatement { .. } => {}
                MIRStatement::LoopStatement { .. } => {}
                MIRStatement::MarkerStatement { .. } => {}

                MIRStatement::CreateVariable {
                    var, value, span, ..
                } => {
                    convert_types(ctx.target, &mut var.ty.ty);

                    // Disallow shadowing.
                    // (Phantom) arg variables shouldn't get checked against locals, since
                    // they might be added to the scope automatically.
                    if (!var.arg && scope.get_variable(&var.name).is_some())
                        || ctx.program.static_names.contains_key(&var.name)
                        || ctx.program.const_names.contains_key(&var.name)
                    {
                        eprintln_span!(
                            Some(span.clone()),
                            "Cannot shadow existing variable {}",
                            var.name
                        );
                        return false;
                    }

                    if let Some(value) = value {
                        value.ty = Some(var.ty.clone());
                        if check_expression(ctx, value, Some(scope)).is_none() {
                            return false;
                        };
                    }
                }

                MIRStatement::SetVariable { value, place, .. } => {
                    // Make sure we aren't trying to modify a const.
                    // If a const appears inside a place expression (e.g., a[const]), then
                    // we aren't modifying the const.
                    // If it appears outside (e.g., const[a]), then we are, so should error.
                    if !explore_outer_place(place, &mut |expr| {
                        if let MIRExpressionInner::Variable(var, _) = &expr.inner
                            && ctx.program.const_names.contains_key(var)
                        {
                            eprintln_span!(Some(expr.span.clone()), "Cannot set constants!");
                            return false;
                        }

                        true
                    }) {
                        return false;
                    }

                    // When we allocate variables, we want to use args before creating new variables.
                    // Therefore, it's useful for args to be simplified, as it's tricky to allocate into
                    // a variable which has a complex lifetime. There's basically no need to anyway, since
                    // variable allocation will make efficient use of the space.
                    //
                    // However, setting the data inside a variable is a legitimate operation, so we need
                    // to allow it. Therefore, any such operation is considered a read and a write for
                    // optimization. This is important because a write with no read after it will just
                    // be removed.
                    //
                    // So, the only case we need to disallow is directly setting a variable.
                    if let MIRExpressionInner::Variable(var, _) = &place.inner
                        && let Some(scope_var) = scope.get_variable(var)
                        && scope_var.arg
                    {
                        eprintln_span!(Some(place.span.clone()), "Cannot set args!");
                        return false;
                    }

                    // This will handle the types of locals/statics the same way as normal expressions,
                    // which works for our purposes here.
                    let Some(var_ty) = check_expression(ctx, place, Some(scope)) else {
                        return false;
                    };

                    value.ty = Some(var_ty.clone());
                    if check_expression(ctx, value, Some(scope)).is_none() {
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
                    condition.ty = Some(MIRType {
                        ty: MIRTypeInner::Bool,
                        span: Some(span.clone()),
                    });
                    if check_expression(ctx, condition, Some(scope)).is_none() {
                        return false;
                    }
                }

                MIRStatement::Return { expr, span, .. } => match expr {
                    Some(expr) => {
                        expr.ty = Some(function.ret_ty.clone());
                        if check_expression(ctx, expr, Some(scope)).is_none() {
                            return false;
                        }
                    }
                    None => {
                        // No need for type_equal since unit can't resolve numbers.
                        if function.ret_ty.ty != MIRTypeInner::Unit {
                            print_unexpected_expr_ty(
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
        &mut |_, _| true,
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
        MIRFnSource::Direct(name, span) => {
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
            let Some(candidate) = get_fn_candidate(ctx, name, &args_ty, Some(&span)) else {
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
            MIRFunctionArgs {
                args: actual_args.iter().map(|arg| arg.ty.clone()).collect(),
                variadic: false,
            },
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
        print_unexpected_expr_ty(expected_ty, actual_ty, span.clone());
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

    // Ensure that both function types have compatible arg lengths.
    let fixed_arg_count = expected_args.args.len();
    if expected_args.variadic {
        // Variadic functions need at least as many args as fixed params.
        if actual_args.len() < fixed_arg_count {
            print_unexpected_expr_ty(expected_ty, actual_ty, span.clone());
            return false;
        }
    } else {
        // Non-variadic functions need exactly the right number of args.
        if actual_args.len() != fixed_arg_count {
            print_unexpected_expr_ty(expected_ty, actual_ty, span.clone());
            return false;
        }
    }

    // Ensure that individual fixed arg types match,
    // for more granular errors.
    for (actual, expected) in actual_args
        .iter_mut()
        .take(fixed_arg_count)
        .zip(expected_args.args.iter_mut())
    {
        if !types_equal_inner(&mut actual.ty, expected) {
            print_unexpected_expr_ty(
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

    // Store the computed types.
    // For variadic calls, store the full arg list (including variadic args).
    *out_ret_ty = Some(MIRType {
        ty: expected_ret_ty,
        span: expected_ty.span,
    });
    *out_args_ty = Some(MIRFunctionArgs {
        args: actual_args.iter().map(|arg| arg.ty.clone()).collect(),
        // The call itself is not variadic, only the function signature is
        variadic: false,
    });

    true
}

/// Prints an error for when an expression
/// requires left and right operands to
/// be equal, but they aren't.
fn print_left_right_unequal(
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
        .eprint(file_cache())
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
        | (from @ (MIRTypeInner::I32 | MIRTypeInner::U32), to @ MIRTypeInner::UnknownNumber)
        | (to @ MIRTypeInner::NotConstructed, from)
        | (from, to @ MIRTypeInner::NotConstructed) => {
            *to = from.clone();

            true
        }
        // Downgrading fixed array to dynamic array.
        (to @ MIRTypeInner::Array(..), from @ MIRTypeInner::ArrayFixed(..))
        | (to @ MIRTypeInner::ArrayFixed(..), from @ MIRTypeInner::Array(..)) => {
            if let MIRTypeInner::Array(val1) = to
                && let MIRTypeInner::ArrayFixed(val2, _) = from
                && !types_equal_inner(val1, val2)
            {
                return false;
            }
            if let MIRTypeInner::ArrayFixed(val1, _) = to
                && let MIRTypeInner::Array(val2) = from
                && !types_equal_inner(val1, val2)
            {
                return false;
            }

            *to = from.clone();

            true
        }
        (MIRTypeInner::Array(val1), MIRTypeInner::Array(val2)) => types_equal_inner(val1, val2),
        (MIRTypeInner::ArrayFixed(val1, len1), MIRTypeInner::ArrayFixed(val2, len2)) => {
            len1 == len2 && types_equal_inner(val1, val2)
        }
        // Recursive types need special handling to fully resolve.
        (MIRTypeInner::FunctionPtr(args1, ret1), MIRTypeInner::FunctionPtr(args2, ret2)) => {
            if args1.args.len() != args2.args.len() || args1.variadic != args2.variadic {
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

            for (arg1, arg2) in new_args1.args.iter_mut().zip(new_args2.args.iter_mut()) {
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

/// Checks if two types could match (considering inference).
/// from and to refer to the direction of the types.
/// For example, when calling a function, from refers to the
/// function arg type, and to refers to the type of the expression
/// passed to the function arg.
/// For variable sets, from is the type of the variable, to is the type
/// of the expression.
pub fn types_could_match_ordered<'a>(from: &MIRTypeInner<'a>, to: &MIRTypeInner<'a>) -> bool {
    if from == to {
        return true;
    }

    match (from, to) {
        (MIRTypeInner::UnknownNumber, MIRTypeInner::I32 | MIRTypeInner::U32)
        | (MIRTypeInner::I32 | MIRTypeInner::U32, MIRTypeInner::UnknownNumber)
        | (MIRTypeInner::NotConstructed, _)
        | (_, MIRTypeInner::NotConstructed) => true,
        // We can't convert an unknown sized array to a fixed sized array, but we can
        // convert a fixed size array to an unknown size array.
        (MIRTypeInner::Array(ty1), MIRTypeInner::ArrayFixed(ty2, _))
        | (MIRTypeInner::Array(ty1), MIRTypeInner::Array(ty2))
            if types_could_match_ordered(ty1, ty2) =>
        {
            true
        }
        // Array sizes MUST match.
        (MIRTypeInner::ArrayFixed(ty1, count1), MIRTypeInner::ArrayFixed(ty2, count2))
            if count1 == count2 && types_could_match_ordered(ty1, ty2) =>
        {
            true
        }
        _ => false,
    }
}

/// Checks if two types could match (considering inference).
/// This is true in more cases than types_could_match, and should
/// only be used to prevent conflicts.
pub fn types_could_match<'a>(from: &MIRTypeInner<'a>, to: &MIRTypeInner<'a>) -> bool {
    if from == to {
        return true;
    }

    match (from, to) {
        (MIRTypeInner::UnknownNumber, MIRTypeInner::I32 | MIRTypeInner::U32)
        | (MIRTypeInner::I32 | MIRTypeInner::U32, MIRTypeInner::UnknownNumber)
        | (MIRTypeInner::NotConstructed, _)
        | (_, MIRTypeInner::NotConstructed) => true,
        // Allow fixed size <-> unknown size, since these conflict with each other.
        (MIRTypeInner::ArrayFixed(ty1, _), MIRTypeInner::Array(ty2))
        | (MIRTypeInner::Array(ty1), MIRTypeInner::ArrayFixed(ty2, _))
        | (MIRTypeInner::Array(ty1), MIRTypeInner::Array(ty2))
            if types_could_match(ty1, ty2) =>
        {
            true
        }
        // Array sizes MUST match.
        (MIRTypeInner::ArrayFixed(ty1, count1), MIRTypeInner::ArrayFixed(ty2, count2))
            if count1 == count2 && types_could_match(ty1, ty2) =>
        {
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
    caller_span: Option<&Span<'a>>,
) -> Option<MIRFunctionKey> {
    let Some(overloads) = ctx.program.function_names.get(name) else {
        // TODO: No function found error.
        eprintln_span!(caller_span.cloned(), "No function found with name {name:?}");
        return None;
    };

    match overloads.find_compatible(args) {
        Some(key) => Some(key),
        None => {
            // Could be no matches or ambiguous (multiple)
            let count = overloads.count_compatible(args);

            if count == 0 {
                println!("{args:?} {overloads:?}");
                eprintln_span!(
                    caller_span.cloned(),
                    "No compatible function found with name {name:?} (other overloads exist)"
                );
            } else {
                // TODO: Multiple functions found error.
                eprintln_span!(
                    caller_span.cloned(),
                    "Multiple functions found with name {name:?}. Disambiguate arguments with type annotations."
                );
            }
            None
        }
    }
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
                $left.ty = Some(inherit_ty.clone());
                $right.ty = Some(inherit_ty.clone());
            }

            let t_left = check_expression(ctx, $left, scope)?;
            let t_right = check_expression(ctx, $right, scope)?;

            if !types_equal(t_left, t_right) {
                print_left_right_unequal($name, t_left.clone(), t_right.clone(), expr.span.clone());
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
            simple_binary!(
                $left,
                $right,
                $name,
                &mut (None as Option<MIRType<'a>>),
                internal
            );

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
                    eprintln_span!(
                        Some(expr.span.clone()),
                        "Cannot directly access function as value (use a reference): {expr:?}"
                    );
                    return None;
                }

                print_var_does_not_exist(name.clone(), expr.span.clone());
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
            MIRExpressionInner::Char(_) => Some(MIRType {
                ty: MIRTypeInner::Char,
                span: None,
            }),
            MIRExpressionInner::Unit => Some(MIRType {
                ty: MIRTypeInner::Unit,
                // Span is added after.
                span: None,
            }),
            MIRExpressionInner::Ref(inner) => {
                if !matches!(
                    inner.inner,
                    MIRExpressionInner::Variable(..)
                        | MIRExpressionInner::Index(..)
                        | MIRExpressionInner::Member(..)
                ) {
                    // Some languages will inject temporaries.
                    // Maybe we can do this automatically as well.
                    eprintln_span!(
                        Some(inner.span.clone()),
                        "References can only be made to variables, array indexes, or member access: {inner:?}"
                    );
                    return None;
                }

                // Resolve the inner (non-reference) type.
                if let Some(MIRType {
                    ty: MIRTypeInner::Ref(inherit_ty),
                    span,
                }) = &mut expr.ty
                {
                    inner.ty = Some(MIRType {
                        ty: (**inherit_ty).clone(),
                        span: span.clone(),
                    });
                }
                let inner_ty = check_expression(ctx, inner, scope)?.clone();

                // Extract the type of the reference (outer).
                Some(MIRType {
                    ty: MIRTypeInner::Ref(Box::new(inner_ty.ty)),
                    span: inner_ty.span,
                })
            }
            MIRExpressionInner::Deref(inner) => {
                // Resolve the inner (reference) type.
                if let Some(inherit_ty) = &mut expr.ty {
                    inner.ty = Some(MIRType {
                        ty: MIRTypeInner::Ref(Box::new(inherit_ty.ty.clone())),
                        span: inherit_ty.span.clone(),
                    });
                }
                let mut inner_ty = check_expression(ctx, inner, scope)?.clone();

                // Extract the type of the Deref by unwrapping the reference.
                match inner_ty.ty {
                    MIRTypeInner::Ref(value) => {
                        inner_ty.ty = *value;
                    }
                    _ => {
                        eprintln_span!(
                            Some(inner.span.clone()),
                            "Cannot dereference non-reference type: {inner:?}"
                        );
                        return None;
                    }
                }

                Some(inner_ty)
            }
            MIRExpressionInner::Array(elems) => {
                if elems.is_empty() {
                    return Some(MIRType {
                        ty: MIRTypeInner::ArrayFixed(Box::new(MIRTypeInner::NotConstructed), 0),
                        span: None,
                    });
                }

                let (first, rest) = elems.split_at_mut(1);

                if let Some(MIRType {
                    ty: MIRTypeInner::Array(box inner) | MIRTypeInner::ArrayFixed(box inner, _),
                    span,
                }) = &mut expr.ty
                {
                    first[0].ty = Some(MIRType {
                        ty: inner.clone(),
                        span: span.clone(),
                    });
                }
                let first_ty = check_expression(ctx, &mut first[0], scope)?;

                for other in rest {
                    other.ty = Some(first_ty.clone());
                    check_expression(ctx, other, scope)?;
                }

                Some(MIRType {
                    ty: MIRTypeInner::ArrayFixed(Box::new(first_ty.ty.clone()), elems.len()),
                    span: None,
                })
            }
            MIRExpressionInner::Index(base, index) => {
                let index_ty = check_expression(ctx, index, scope)?;
                if matches!(index_ty.ty, MIRTypeInner::UnknownNumber) {
                    index_ty.ty = MIRTypeInner::U32;
                }

                if let Some(inherit_ty) = &mut expr.ty {
                    base.ty = Some(MIRType {
                        ty: MIRTypeInner::Array(Box::new(inherit_ty.ty.clone())),
                        span: inherit_ty.span.clone(),
                    });
                }
                let MIRType {
                    ty: MIRTypeInner::Array(inner),
                    span,
                } = check_expression(ctx, base, scope)?.clone()
                else {
                    panic!("Indexing a non-array!");
                };

                Some(MIRType { ty: *inner, span })
            }
            MIRExpressionInner::Quine => Some(MIRType {
                ty: MIRTypeInner::Array(Box::new(MIRTypeInner::String)),
                span: None,
            }),
            MIRExpressionInner::QuineLen => Some(MIRType {
                ty: MIRTypeInner::UnknownNumber,
                span: None,
            }),

            // TODO: Implement type checking for place expressions.
            MIRExpressionInner::Member(_, _) => todo!(),
        }
    })()?;

    // Lower strings and others based on target.
    convert_types(ctx.target, &mut ty.ty);

    // Ensure the type covers the
    // whole span.
    ty.span = Some(expr.span.clone());

    // If the expression already has a type, it cannot disagree with itself.
    // This is used for, e.g., throwing an error if "-10u32" is written, and
    // propagating the explicitly written type if it is valid.
    if let Some(existing_ty) = &mut expr.ty
        && !types_equal(existing_ty, &mut ty)
    {
        print_unexpected_expr_ty(existing_ty.clone(), ty.clone(), expr.span.clone());
        return None;
    }

    // Save the type for later
    // phases.
    expr.ty = Some(ty);

    // Ensure that we return a type
    // whose span covers the entire expression.
    expr.ty.as_mut()
}

/// Converts types to their target versions, if necessary.
/// For example, C strings are lowered to `&[char]`.
pub fn convert_types(target: &dyn Target, ty: &mut MIRTypeInner) {
    match ty {
        MIRTypeInner::String if target.str_char_arr() => {
            *ty = MIRTypeInner::Array(Box::new(MIRTypeInner::Char));
        }
        MIRTypeInner::Array(box inner)
        | MIRTypeInner::ArrayFixed(box inner, _)
        | MIRTypeInner::Ref(box inner) => {
            convert_types(target, inner);
        }
        MIRTypeInner::FunctionPtr(args, ret) => {
            for arg in &mut args.args {
                convert_types(target, arg);
            }

            convert_types(target, ret);
        }
        MIRTypeInner::Named(_) => todo!(),
        _ => {}
    }
}
