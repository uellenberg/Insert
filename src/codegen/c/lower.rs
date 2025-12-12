use crate::codegen::c::token::{CToken, CTokens};
use crate::codegen::{Token, Tokens, merge_tokens, spread};
use crate::mir::{
    MIRExpression, MIRExpressionInner, MIRFnSource, MIRFunction, MIRProgram, MIRStatement,
    MIRStatic, MIRType, MIRTypeInner,
};
use std::borrow::Cow;

/// Converts a program from MIR to C.
pub fn mir_to_c(program: &MIRProgram) -> String {
    let mut output = spread![];

    for val in program.statics.values() {
        output.extend(lower_static(val));
    }

    for val in program.functions.values() {
        output.extend(lower_function(val));
    }

    merge_tokens(&mut output);
    println!("{:#?}", output);

    let mut output_str = String::new();
    let mut iter = output.iter().peekable();
    while let Some(token) = iter.next() {
        let Some(token_text) = token.text() else {
            continue;
        };

        output_str.push_str(token_text);

        // Add space if needed to allow compilation.
        if let Some(next) = iter.peek() {
            if token.needs_space_between(next) {
                output_str.push(' ');
            }
        }
    }
    output_str
}

/// Converts a function from MIR to C.
fn lower_function<'a>(func: &MIRFunction<'a>) -> CTokens<'a> {
    let decorated = decorate_with_type(func.name.clone(), &func.ret_ty);
    let args = func
        .args
        .iter()
        .map(|arg| decorate_with_type(arg.name.clone(), &arg.ty))
        .intersperse(spread![CToken::new(",".into())])
        .flatten()
        .collect::<CTokens<'a>>();
    let block = lower_block(&func.body);

    spread![...decorated, CToken::new("(".into()), ...args, CToken::new(")".into()), CToken::new("{".into()), ...block, CToken::new("}".into())]
}

/// Converts a block of statements from MIR to C.
fn lower_block<'a>(block: &Vec<MIRStatement<'a>>) -> CTokens<'a> {
    block
        .iter()
        .map(lower_statement)
        // Remove None values.
        .flatten()
        .flatten()
        .collect::<CTokens<'a>>()
}

/// Converts a statement from MIR to C.
/// Returns None if the statement is not valid C (i.e., it should be ignored).
fn lower_statement<'a>(stmt: &MIRStatement<'a>) -> Option<CTokens<'a>> {
    match stmt {
        // Just for analysis, no real codegen.
        MIRStatement::CreateVariable { arg: true, .. } | MIRStatement::DropVariable(..) => None,

        MIRStatement::CreateVariable { var, value, .. } => {
            let decorated = decorate_with_type(var.name.clone(), &var.ty);

            if let Some(value) = value {
                let expr = lower_expression(value);

                Some(spread![
                    ...decorated,
                    CToken::new("=".into()),
                    ...expr,
                    CToken::new(";".into())
                ])
            } else {
                Some(spread![
                    ...decorated,
                    CToken::new(";".into())
                ])
            }
        }

        MIRStatement::SetVariable { name, value, .. } => {
            let expr = lower_expression(value);

            Some(spread![
                CToken::new(name.clone()),
                CToken::new("=".into()),
                ...expr,
                CToken::new(";".into())
            ])
        }

        MIRStatement::FunctionCall(call) => {
            let fn_src = lower_fn_source(&call.source);
            let args = call
                .args
                .iter()
                .map(lower_expression)
                .intersperse(spread![CToken::new(",".into())])
                .flatten()
                .collect::<CTokens<'a>>();

            Some(spread![
                ...fn_src,
                CToken::new("(".into()),
                ...args,
                CToken::new(");".into())
            ])
        }

        MIRStatement::Return { expr, .. } => {
            if let Some(expr) = expr {
                let ret_expr = lower_expression(expr);

                Some(spread![
                    CToken::new("return".into()),
                    ...ret_expr,
                    CToken::new(";".into())
                ])
            } else {
                Some(spread![
                    CToken::new("return".into()),
                    CToken::new(";".into())
                ])
            }
        }

        MIRStatement::Label { .. }
        | MIRStatement::Goto { .. }
        | MIRStatement::GotoNotEqual { .. } => todo!("Should these be removed?"),

        MIRStatement::IfStatement {
            condition,
            on_true,
            on_false,
            ..
        } => {
            let cond = lower_expression(condition);
            let true_block = lower_block(on_true);

            if on_false.is_empty() {
                Some(spread![
                    CToken::new("if".into()),
                    CToken::new("(".into()),
                    ...cond,
                    CToken::new(")".into()),
                    CToken::new("{".into()),
                    ...true_block,
                    CToken::new("}".into())
                ])
            } else {
                let false_block = lower_block(on_false);

                Some(spread![
                    CToken::new("if".into()),
                    CToken::new("(".into()),
                    ...cond,
                    CToken::new(")".into()),
                    CToken::new("{".into()),
                    ...true_block,
                    CToken::new("}".into()),
                    CToken::new("else".into()),
                    CToken::new("{".into()),
                    ...false_block,
                    CToken::new("}".into())
                ])
            }
        }

        MIRStatement::LoopStatement { body, .. } => {
            let loop_body = lower_block(body);

            Some(spread![
                CToken::new("while".into()),
                CToken::new("(".into()),
                CToken::new("1".into()),
                CToken::new(")".into()),
                CToken::new("{".into()),
                ...loop_body,
                CToken::new("}".into())
            ])
        }

        MIRStatement::ContinueStatement { .. } => Some(spread![
            CToken::new("continue".into()),
            CToken::new(";".into())
        ]),

        MIRStatement::BreakStatement { .. } => Some(spread![
            CToken::new("break".into()),
            CToken::new(";".into())
        ]),
    }
}

/// Converts a static variable from MIR to C.
fn lower_static<'a>(val: &MIRStatic<'a>) -> CTokens<'a> {
    let decorated = decorate_with_type(val.name.clone(), &val.ty);
    let expr = lower_expression(&val.value);

    spread![CToken::new("static".into()), ...decorated, CToken::new("=".into()), ...expr, CToken::new(";".into())]
}

/// Converts an expression from MIR to C.
fn lower_expression<'a>(expr: &MIRExpression<'a>) -> CTokens<'a> {
    macro_rules! lower_binary {
        ($left:expr, $op:tt, $right:expr) => {{
            let left = lower_wrap_expression($left, expr);
            let right = lower_wrap_expression($right, expr);

            spread![...left, CToken::new($op.into()), ...right]
        }}
    }

    match &expr.inner {
        MIRExpressionInner::Add(left, right) => lower_binary!(left, "+", right),
        MIRExpressionInner::Sub(left, right) => lower_binary!(left, "-", right),
        MIRExpressionInner::Mul(left, right) => lower_binary!(left, "*", right),
        MIRExpressionInner::Div(left, right) => lower_binary!(left, "/", right),
        MIRExpressionInner::Equal(left, right) => lower_binary!(left, "==", right),
        MIRExpressionInner::NotEqual(left, right) => lower_binary!(left, "!=", right),
        MIRExpressionInner::Less(left, right) => lower_binary!(left, "<", right),
        MIRExpressionInner::Greater(left, right) => lower_binary!(left, ">", right),
        MIRExpressionInner::LessEq(left, right) => lower_binary!(left, "<=", right),
        MIRExpressionInner::GreaterEq(left, right) => lower_binary!(left, ">=", right),
        MIRExpressionInner::BoolAnd(left, right) => lower_binary!(left, "&&", right),
        MIRExpressionInner::BoolOr(left, right) => lower_binary!(left, "||", right),
        MIRExpressionInner::Number(num) => spread![CToken::new(num.to_string().into())],
        MIRExpressionInner::Bool(val) => {
            if *val {
                spread![CToken::new("true".into())]
            } else {
                spread![CToken::new("false".into())]
            }
        }
        MIRExpressionInner::Variable(name) => spread![CToken::new(name.clone())],
        MIRExpressionInner::FunctionCall(call) => {
            let args = call
                .args
                .iter()
                .map(lower_expression)
                .intersperse(spread![CToken::new(", ".into())])
                .flatten()
                .collect::<CTokens<'a>>();
            let src = lower_fn_source(&call.source);

            spread![...src, CToken::new("(".into()), ...args, CToken::new(")".into())]
        }
    }
}

/// Lowers a function source into a callable string (i.e., () can be added after it).
fn lower_fn_source<'a>(src: &MIRFnSource<'a>) -> CTokens<'a> {
    match src {
        MIRFnSource::Direct(src, _) => spread![CToken::new(src.clone())],
        MIRFnSource::Indirect(name) => {
            let lowered = lower_expression(name);
            spread![CToken::new("(".into()), ...lowered, CToken::new(")".into())]
        }
    }
}

/// Converts a datatype from MIR to C.
/// Returns the type as (prefix, postfix), where
/// a variable can be constructed as:
/// {PREFIX} name{POSTFIX} (e.g., char* strings[]).
fn lower_datatype<'a>(ty: &MIRType<'a>) -> (CTokens<'a>, CTokens<'a>) {
    match &ty.ty {
        MIRTypeInner::U32 => (
            spread![CToken::new("unsigned".into()), CToken::new("int".into())],
            [].into(),
        ),
        MIRTypeInner::Bool => (spread![CToken::new("bool".into())], [].into()),
        MIRTypeInner::Unit => (spread![CToken::new("void".into())], [].into()),
        MIRTypeInner::FunctionPtr(args, ret) => todo!(),
        MIRTypeInner::Named(name) => (spread![CToken::new(name.clone())], [].into()),
    }
}

/// Adds a datatype to a variable/function name.
fn decorate_with_type<'a>(name: Cow<'a, str>, ty: &MIRType<'a>) -> CTokens<'a> {
    let (prefix, postfix) = lower_datatype(ty);
    spread![...prefix, CToken::new(name.into()), ...postfix]
}

/// Returns the precedence of an operator, or None if precedence
/// doesn't apply to it (e.g., variable / literals).
///
/// Given outer(a, inner(b, c)), inner must be wrapped if its precedence
/// number is higher than outer's. If inner/outer has no precedence, then it
/// needs no wrapping.
fn precedence(op: &MIRExpressionInner) -> Option<usize> {
    // https://en.cppreference.com/w/c/language/operator_precedence.html
    match op {
        MIRExpressionInner::Variable(..)
        | MIRExpressionInner::Number(_)
        | MIRExpressionInner::Bool(_)
        | MIRExpressionInner::FunctionCall(_) => None,

        MIRExpressionInner::Mul(..) | MIRExpressionInner::Div(..) => Some(3),

        MIRExpressionInner::Add(..) | MIRExpressionInner::Sub(..) => Some(4),

        MIRExpressionInner::Less(..)
        | MIRExpressionInner::Greater(..)
        | MIRExpressionInner::LessEq(..)
        | MIRExpressionInner::GreaterEq(..) => Some(6),

        MIRExpressionInner::Equal(..) | MIRExpressionInner::NotEqual(..) => Some(7),

        MIRExpressionInner::BoolAnd(..) => Some(8),

        MIRExpressionInner::BoolOr(..) => Some(9),
    }
}

/// Lowers a child expression and correctly wraps it in parentheses
/// if needed.
fn lower_wrap_expression<'a>(expr: &MIRExpression<'a>, outer: &MIRExpression<'a>) -> CTokens<'a> {
    let lowered = lower_expression(expr);

    let outer_precedence = precedence(&outer.inner);
    let inner_precedence = precedence(&expr.inner);
    let needs_wrap = match (outer_precedence, inner_precedence) {
        (Some(outer), Some(inner)) if inner > outer => true,
        _ => false,
    };

    if needs_wrap {
        spread![CToken::new("(".into()), ...lowered, CToken::new(")".into())]
    } else {
        lowered
    }
}
