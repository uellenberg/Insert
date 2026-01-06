use crate::mir::function::get_fn_type;
use crate::mir::scope::{Scope, StatementExplorer};
use crate::mir::{
    MIRConstant, MIRContext, MIRExpression, MIRExpressionInner, MIRFnCall, MIRFnSource,
    MIRFunction, MIRStatement, MIRStatic, MIRType, MIRTypeInner,
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

    if expr_type.ty != constant.ty.ty {
        print_unexpected_expr_ty(
            ctx,
            constant.ty.clone(),
            expr_type,
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

    if expr_type.ty != static_data.ty.ty {
        print_unexpected_expr_ty(
            ctx,
            static_data.ty.clone(),
            expr_type,
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
                MIRStatement::CreateVariable { value: None, .. } => {}
                MIRStatement::DropVariable(..) => {}
                MIRStatement::Goto { .. } => {}
                MIRStatement::Label { .. } => {}
                MIRStatement::ContinueStatement { .. } => {}
                MIRStatement::BreakStatement { .. } => {}
                MIRStatement::LoopStatement { .. } => {}

                MIRStatement::CreateVariable {
                    var,
                    value: Some(value),
                    ..
                } => {
                    let Some(ty) = check_expression(ctx, value, Some(scope)) else {
                        return false;
                    };

                    if ty.ty != var.ty.ty {
                        print_unexpected_expr_ty(ctx, var.ty.clone(), ty, value.span.clone());

                        return false;
                    }
                }

                MIRStatement::SetVariable { value, name, span } => {
                    let var_ty;

                    if let Some(var) = scope.get_variable(name) {
                        var_ty = var.ty.clone();
                    } else if let Some(var) = ctx.program.statics.get(name) {
                        var_ty = var.ty.clone();
                    } else if let Some(_var) = ctx.program.constants.get(name) {
                        eprintln!("Cannot set constants!");
                        return false;
                    } else {
                        print_var_does_not_exist(ctx, name.clone(), span.clone());
                        return false;
                    }

                    let Some(ty) = check_expression(ctx, value, Some(scope)) else {
                        return false;
                    };

                    if ty.ty != var_ty.ty {
                        print_unexpected_expr_ty(ctx, var_ty, ty, value.span.clone());

                        return false;
                    }
                }

                MIRStatement::FunctionCall(MIRFnCall {
                    source,
                    args,
                    ret_ty,
                    span,
                    ..
                }) => {
                    if !check_fn_call(ctx, Some(scope), source, args, ret_ty, span) {
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

                    if cond_ty.ty != MIRTypeInner::Bool {
                        print_unexpected_expr_ty(
                            ctx,
                            MIRType {
                                ty: MIRTypeInner::Bool,
                                span: Some(span.clone()),
                            },
                            cond_ty,
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

                        if cond_ty.ty != function.ret_ty.ty {
                            print_unexpected_expr_ty(
                                ctx,
                                function.ret_ty.clone(),
                                cond_ty,
                                expr.span.clone(),
                            );

                            return false;
                        }
                    }
                    None => {
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
    ret_ty: &mut Option<MIRType<'a>>,
    span: &Span<'a>,
) -> bool {
    let expected_ty = match source {
        MIRFnSource::Direct(name, _span) => {
            let Some(fn_data) = ctx.program.functions.get(name) else {
                // TODO: Function not found error.
                eprintln!("Function not found: {name:?}");
                return false;
            };

            get_fn_type(fn_data)
        }
        MIRFnSource::Indirect(expr) => {
            let Some(ty) = check_expression(ctx, expr, scope) else {
                return false;
            };

            ty
        }
    };

    // Add type information to arguments.
    for arg in args.iter_mut() {
        if check_expression(ctx, arg, scope).is_none() {
            return false;
        }
    }

    let actual_args = args
        .iter()
        .map(|arg| {
            arg.ty
                .clone()
                .expect("Function argument didn't have type info!")
        })
        .collect::<Vec<_>>();

    let mut actual_ty = MIRType {
        ty: MIRTypeInner::FunctionPtr(
            actual_args.iter().map(|arg| arg.ty.clone()).collect(),
            // Default to unit type for error messages
            // when unmatched function, since we only
            // know the return type once we have a valid
            // function.
            Box::new(MIRTypeInner::Unit),
        ),
        span: None,
    };

    // Ensure that we have a function type.
    let MIRTypeInner::FunctionPtr(expected_args, expected_ret_ty) = &expected_ty.ty else {
        print_unexpected_expr_ty(ctx, expected_ty, actual_ty, span.clone());
        return false;
    };

    // Give actual_ty the correct return type.
    // We have more complete info in actual_args, so
    // no need to extract it here (_).
    let MIRTypeInner::FunctionPtr(_, actual_ret_ty) = &mut actual_ty.ty else {
        unreachable!();
    };

    **actual_ret_ty = (**expected_ret_ty).clone();

    // Ensure that both function types have the
    // same arg length.
    if actual_args.len() != expected_args.len() {
        print_unexpected_expr_ty(ctx, expected_ty, actual_ty, span.clone());
        return false;
    }

    // Ensure that individual arg types match,
    // for more granular errors.
    for (actual, expected) in actual_args.iter().zip(expected_args.iter()) {
        if &actual.ty != expected {
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
    if expected_ty.ty != actual_ty.ty {
        print_unexpected_expr_ty(ctx, expected_ty, actual_ty, span.clone());
        return false;
    }

    // Store the computed return type.
    *ret_ty = Some(MIRType {
        ty: (**expected_ret_ty).clone(),
        span: expected_ty.span,
    });

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

/// Checks whether the expression is valid,
/// and modifies its type information to match.
/// If it isn't, errors are reported.
/// If it is, the expression's type is returned.
fn check_expression<'a>(
    ctx: &MIRContext<'a>,
    expr: &mut MIRExpression<'a>,
    scope: Option<&Scope<'a>>,
) -> Option<MIRType<'a>> {
    macro_rules! simple_binary {
        ($left:expr, $right:expr, $name:literal, internal) => {{
            let t_left = check_expression(ctx, $left, scope)?;
            let t_right = check_expression(ctx, $right, scope)?;

            if t_left.ty != t_right.ty {
                print_left_right_unequal(ctx, $name, t_left, t_right, expr.span.clone());
                return None;
            }

            Some(t_left)
        }};
        ($left:expr, $right:expr, $name:literal) => {
            simple_binary!($left, $right, $name, internal)
        };
        ($left:expr, $right:expr, $name:literal, $ty:expr) => {{
            simple_binary!($left, $right, $name, internal);

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

                if let Some(var) = ctx.program.constants.get(name) {
                    return Some(var.ty.clone());
                }

                if let Some(var) = ctx.program.statics.get(name) {
                    return Some(var.ty.clone());
                }

                if ctx.program.functions.get(name).is_some() {
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
            MIRExpressionInner::Number(num) => {
                // TODO: Handle negatives.
                assert!(*num >= 0);

                Some(MIRType {
                    ty: MIRTypeInner::U32,
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
        }
    })()?;

    // Ensure the type covers the
    // whole span.
    ty.span = Some(expr.span.clone());

    // Save the type for later
    // phases.
    expr.ty = Some(ty.clone());

    // Ensure that we return a type
    // whose span covers the entire expression.
    expr.ty.clone()
}
