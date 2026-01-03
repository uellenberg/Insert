use crate::codegen::c::token::{
    CToken, CTokens, INDENT, LEFT_PAREN, LEFT_SQUIGGLE, NEWLINE, RIGHT_PAREN, RIGHT_SQUIGGLE, SEMI,
    escape_string,
};
use crate::codegen::{LowerOptions, Token, Tokens, merge_tokens, spread, strip_fancy_tokens};
use crate::mir::{
    MIRExpression, MIRExpressionInner, MIRFnSource, MIRFunction, MIRProgram, MIRStatement,
    MIRStatic, MIRType, MIRTypeInner,
};
use std::borrow::Cow;

/// Converts a program from MIR to C.
pub fn mir_to_c(program: &MIRProgram, options: LowerOptions) -> String {
    let info = CWriterInfo::default();
    let mut output = spread![];

    for val in program.statics.values() {
        output.extend(lower_static(val, info));
    }

    for val in program.functions.values() {
        output.extend(lower_function(val, info));
    }

    if !options.fancy {
        strip_fancy_tokens(&mut output);
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
        if let Some(next) = iter.peek()
            && token.needs_space_between(next)
        {
            output_str.push(' ');
        }
    }
    output_str
}

#[derive(Default, Debug, Copy, Clone)]
struct CWriterInfo {
    /// The current indentation level.
    /// This represents the number of tabs of indentation (not the number of spaces).
    indent_level: u32,
}

/// Converts a function from MIR to C.
fn lower_function<'a>(func: &MIRFunction<'a>, info: CWriterInfo) -> CTokens<'a> {
    let decorated = decorate_with_type(func.name.clone(), &func.ret_ty, info);
    let args = func
        .args
        .iter()
        .map(|arg| decorate_with_type(arg.name.clone(), &arg.ty, info))
        .intersperse(spread![CToken::new(",".into())])
        .flatten()
        .collect::<CTokens<'a>>();
    let block = lower_block(&func.body, info);

    spread![...decorated, LEFT_PAREN, ...args, RIGHT_PAREN, LEFT_SQUIGGLE, NEWLINE, ...block, RIGHT_SQUIGGLE, NEWLINE]
}

/// Returns the indentation for the given level (level = number of tabs).
fn indent_tokens<'a>(indent_level: u32) -> CTokens<'a> {
    (0..indent_level).map(|_| INDENT).collect::<CTokens<'a>>()
}

/// Converts a block of statements from MIR to C.
fn lower_block<'a>(block: &Vec<MIRStatement<'a>>, info: CWriterInfo) -> CTokens<'a> {
    // Items inside a block ({ ... }) should be indented.
    let mut inner_info = info;
    inner_info.indent_level += 1;

    let indent = indent_tokens(inner_info.indent_level);

    block
        .iter()
        // Remove None values.
        .filter_map(|v| lower_statement(v, inner_info))
        .flat_map(|v| spread![...indent.clone(), ...v, NEWLINE])
        .collect::<CTokens<'a>>()
}

/// Converts a statement from MIR to C.
/// Returns None if the statement is not valid C (i.e., it should be ignored).
fn lower_statement<'a>(stmt: &MIRStatement<'a>, info: CWriterInfo) -> Option<CTokens<'a>> {
    match stmt {
        // Just for analysis, no real codegen.
        MIRStatement::CreateVariable { arg: true, .. } | MIRStatement::DropVariable(..) => None,

        MIRStatement::CreateVariable { var, value, .. } => {
            let decorated = decorate_with_type(var.name.clone(), &var.ty, info);

            if let Some(value) = value {
                let expr = lower_expression(value, info);

                Some(spread![
                    ...decorated,
                    CToken::new("=".into()),
                    ...expr,
                    SEMI,
                ])
            } else {
                Some(spread![
                    ...decorated,
                    SEMI,
                ])
            }
        }

        MIRStatement::SetVariable { name, value, .. } => {
            let expr = lower_expression(value, info);

            Some(spread![
                CToken::new(name.clone()),
                CToken::new("=".into()),
                ...expr,
                SEMI,
            ])
        }

        MIRStatement::FunctionCall(call) => {
            let fn_src = lower_fn_source(&call.source, info);
            let args = call
                .args
                .iter()
                .map(|v| lower_expression(v, info))
                .intersperse(spread![CToken::new(",".into())])
                .flatten()
                .collect::<CTokens<'a>>();

            Some(spread![
                ...fn_src,
                LEFT_PAREN,
                ...args,
                RIGHT_PAREN,
                SEMI,
            ])
        }

        MIRStatement::Return { expr, .. } => {
            if let Some(expr) = expr {
                let ret_expr = lower_expression(expr, info);

                Some(spread![
                    CToken::new("return".into()),
                    ...ret_expr,
                    SEMI,
                ])
            } else {
                Some(spread![CToken::new("return".into()), SEMI])
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
            let cond = lower_expression(condition, info);
            let true_block = lower_block(on_true, info);

            if on_false.is_empty() {
                Some(spread![
                    CToken::new("if".into()),
                    LEFT_PAREN,
                    ...cond,
                    RIGHT_PAREN,
                    LEFT_SQUIGGLE,
                    NEWLINE,
                    ...true_block,
                    ...indent_tokens(info.indent_level),
                    RIGHT_SQUIGGLE
                ])
            } else {
                let false_block = lower_block(on_false, info);

                Some(spread![
                    CToken::new("if".into()),
                    LEFT_PAREN,
                    ...cond,
                    RIGHT_PAREN,
                    LEFT_SQUIGGLE,
                    NEWLINE,
                    ...true_block,
                    ...indent_tokens(info.indent_level),
                    RIGHT_SQUIGGLE,
                    CToken::new("else".into()),
                    LEFT_SQUIGGLE,
                    NEWLINE,
                    ...false_block,
                    ...indent_tokens(info.indent_level),
                    RIGHT_SQUIGGLE
                ])
            }
        }

        MIRStatement::LoopStatement { body, .. } => {
            let loop_body = lower_block(body, info);

            Some(spread![
                CToken::new("while".into()),
                LEFT_PAREN,
                CToken::new("1".into()),
                RIGHT_PAREN,
                LEFT_SQUIGGLE,
                NEWLINE,
                ...loop_body,
                ...indent_tokens(info.indent_level),
                RIGHT_SQUIGGLE
            ])
        }

        MIRStatement::ContinueStatement { .. } => {
            Some(spread![CToken::new("continue".into()), SEMI])
        }

        MIRStatement::BreakStatement { .. } => Some(spread![CToken::new("break".into()), SEMI]),
    }
}

/// Converts a static variable from MIR to C.
fn lower_static<'a>(val: &MIRStatic<'a>, info: CWriterInfo) -> CTokens<'a> {
    let decorated = decorate_with_type(val.name.clone(), &val.ty, info);
    let expr = lower_expression(&val.value, info);

    spread![CToken::new("static".into()), ...decorated, CToken::new("=".into()), ...expr, SEMI]
}

/// Converts an expression from MIR to C.
fn lower_expression<'a>(expr: &MIRExpression<'a>, info: CWriterInfo) -> CTokens<'a> {
    macro_rules! lower_binary {
        ($left:expr, $op:tt, $right:expr) => {{
            let left = lower_wrap_expression($left, expr, info);
            let right = lower_wrap_expression($right, expr, info);

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
        // This MUST be a single token, as we cannot insert spaces between the quotes and the string content.
        MIRExpressionInner::String(val) => spread![CToken::new(
            ("\"".to_string() + &escape_string(val) + "\"").into()
        )],
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
                .map(|v| lower_expression(v, info))
                .intersperse(spread![CToken::new(", ".into())])
                .flatten()
                .collect::<CTokens<'a>>();
            let src = lower_fn_source(&call.source, info);

            spread![...src, LEFT_PAREN, ...args, RIGHT_PAREN]
        }
    }
}

/// Lowers a function source into a callable string (i.e., () can be added after it).
fn lower_fn_source<'a>(src: &MIRFnSource<'a>, info: CWriterInfo) -> CTokens<'a> {
    match src {
        MIRFnSource::Direct(src, _) => spread![CToken::new(src.clone())],
        MIRFnSource::Indirect(name) => {
            let lowered = lower_expression(name, info);
            spread![LEFT_PAREN, ...lowered, RIGHT_PAREN]
        }
    }
}

/// Converts a datatype from MIR to C.
/// Returns the type as (prefix, postfix), where
/// a variable can be constructed as:
/// {PREFIX} name{POSTFIX} (e.g., char* strings[]).
fn lower_datatype<'a>(ty: &MIRType<'a>, _info: CWriterInfo) -> (CTokens<'a>, CTokens<'a>) {
    match &ty.ty {
        MIRTypeInner::U32 => (
            spread![CToken::new("unsigned".into()), CToken::new("int".into())],
            [].into(),
        ),
        MIRTypeInner::String => (
            spread![CToken::new("char".into()), CToken::new("*".into())],
            [].into(),
        ),
        MIRTypeInner::Bool => (spread![CToken::new("bool".into())], [].into()),
        MIRTypeInner::Unit => (spread![CToken::new("void".into())], [].into()),
        MIRTypeInner::FunctionPtr(_args, _ret) => todo!(),
        MIRTypeInner::Named(name) => (spread![CToken::new(name.clone())], [].into()),
    }
}

/// Adds a datatype to a variable/function name.
fn decorate_with_type<'a>(name: Cow<'a, str>, ty: &MIRType<'a>, info: CWriterInfo) -> CTokens<'a> {
    let (prefix, postfix) = lower_datatype(ty, info);
    spread![...prefix, CToken::new(name), ...postfix]
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
        | MIRExpressionInner::String(_)
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
fn lower_wrap_expression<'a>(
    expr: &MIRExpression<'a>,
    outer: &MIRExpression<'a>,
    info: CWriterInfo,
) -> CTokens<'a> {
    let lowered = lower_expression(expr, info);

    let outer_precedence = precedence(&outer.inner);
    let inner_precedence = precedence(&expr.inner);
    let needs_wrap = match (outer_precedence, inner_precedence) {
        (Some(outer), Some(inner)) if inner > outer => true,
        _ => false,
    };

    if needs_wrap {
        spread![LEFT_PAREN, ...lowered, RIGHT_PAREN]
    } else {
        lowered
    }
}
