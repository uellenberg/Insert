pub mod file_cache;
pub mod span;

use crate::mir::{
    MIRConstant, MIRContext, MIRExpression, MIRExpressionInner, MIRFnCall, MIRFnSource,
    MIRFunction, MIRFunctionType, MIRStatement, MIRStatic, MIRType, MIRTypeInner, MIRVariable,
};
use ariadne::{ColorGenerator, Label, Report, ReportKind};
use pest::Parser;
use pest::iterators::Pair;
use pest_derive::Parser;
use span::{Span, to_span};
use std::borrow::Cow;
use std::path::Path;

#[derive(Parser)]
#[grammar = "parser/program.pest"]
struct InsertParser;

/// Parses a file into MIR,
/// returning whether it was successful.
pub fn parse_file<'a>(location: &'a Path, ctx: &mut MIRContext<'a>) -> bool {
    let data = ctx.file_cache.get(location).unwrap();

    parse_data(location, data, ctx)
}

/// Parses some data file into MIR,
/// returning whether it was successful.
fn parse_data<'a>(location: &'a Path, data: &'a str, ctx: &mut MIRContext<'a>) -> bool {
    let ast = match InsertParser::parse(Rule::program, data) {
        Ok(ast) => ast,
        Err(err) => {
            eprintln!("{err}");
            return false;
        }
    };

    for pair in ast {
        match pair.as_rule() {
            Rule::constDeclaration => {
                let constant = parse_constant(location, pair);
                if !check_no_duplicates(ctx, constant.name.clone(), &constant.span) {
                    return false;
                }

                ctx.program
                    .constants
                    .insert(constant.name.clone(), constant);
            }
            Rule::staticDeclaration => {
                let static_data = parse_static(location, pair);
                if !check_no_duplicates(ctx, static_data.name.clone(), &static_data.span) {
                    return false;
                }

                ctx.program
                    .statics
                    .insert(static_data.name.clone(), static_data);
            }
            Rule::functionDeclaration => {
                let function_data = parse_function(location, pair);
                if !check_no_duplicates(ctx, function_data.name.clone(), &function_data.span) {
                    return false;
                }

                ctx.program
                    .functions
                    .insert(function_data.name.clone(), function_data);
            }
            Rule::EOI => {}
            _ => unreachable!(),
        }
    }

    true
}

fn check_no_duplicates<'a>(ctx: &MIRContext<'a>, name: Cow<'a, str>, span: &Span<'a>) -> bool {
    let defined_span;
    if let Some(var) = ctx.program.statics.get(&name) {
        defined_span = var.span.clone();
    } else if let Some(var) = ctx.program.constants.get(&name) {
        defined_span = var.span.clone();
    } else if let Some(var) = ctx.program.functions.get(&name) {
        defined_span = var.span.clone();
    } else {
        // No duplicates.
        return true;
    }

    let mut colors = ColorGenerator::new();

    let prev = colors.next();
    let cur = colors.next();

    Report::build(ReportKind::Error, span.clone())
        .with_message("Duplicate identifier".to_string())
        .with_label(
            Label::new(defined_span)
                .with_message(format!("Item with name {name} previously defined here"))
                .with_color(prev),
        )
        .with_label(
            Label::new(span.clone())
                .with_message("Redeclaration here".to_string())
                .with_color(cur),
        )
        .finish()
        .eprint(ctx.file_cache.clone())
        .unwrap();

    false
}

fn parse_static<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRStatic<'a> {
    assert_eq!(value.as_rule(), Rule::staticDeclaration);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let identifier = data.next().unwrap().as_str();
    let ty = parse_type(location, data.next().unwrap());
    let expr = parse_expression(location, data.next().unwrap());

    MIRStatic {
        name: Cow::Borrowed(identifier),
        ty,
        value: expr,
        span,
    }
}

fn parse_constant<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRConstant<'a> {
    assert_eq!(value.as_rule(), Rule::constDeclaration);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let identifier = data.next().unwrap().as_str();
    let ty = parse_type(location, data.next().unwrap());
    let expr = parse_expression(location, data.next().unwrap());

    MIRConstant {
        name: Cow::Borrowed(identifier),
        ty,
        value: expr,
        span,
    }
}

fn parse_function<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRFunction<'a> {
    assert_eq!(value.as_rule(), Rule::functionDeclaration);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let fn_type;
    let identifier;

    let first_pair = data.next().unwrap();
    if first_pair.as_rule() == Rule::identifier {
        fn_type = MIRFunctionType::Export;
        identifier = first_pair.as_str();
    } else {
        fn_type = match first_pair.as_rule() {
            Rule::inlineOut => MIRFunctionType::Inline,
            Rule::helperOut => MIRFunctionType::Helper,
            _ => unreachable!(),
        };
        identifier = data.next().unwrap().as_str();
    }

    let mut args = vec![];
    let mut ret = MIRType {
        ty: MIRTypeInner::Unit,
        span: None,
    };

    for pair in data {
        match pair.as_rule() {
            Rule::functionArgs => {
                args = parse_function_args(location, pair);
            }
            Rule::functionReturn => {
                // functionReturn([type])
                ret = parse_type(location, pair.into_inner().next().unwrap());
            }
            Rule::functionBody => {
                // Function body is the last item.
                return MIRFunction {
                    name: Cow::Borrowed(identifier),
                    fn_type,
                    args,
                    ret_ty: ret,
                    body: parse_function_body(location, pair),
                    span,
                };
            }
            _ => unreachable!(),
        }
    }

    // No function body.
    unreachable!();
}

fn parse_function_body<'a>(location: &'a Path, value: Pair<'a, Rule>) -> Vec<MIRStatement<'a>> {
    assert_eq!(value.as_rule(), Rule::functionBody);

    let mut body = vec![];

    for pair in value.into_inner() {
        let span = to_span(location, pair.as_span());

        match pair.as_rule() {
            Rule::createVariable => {
                let mut data = pair.into_inner();

                let identifier = data.next().unwrap().as_str();
                let ty = parse_type(location, data.next().unwrap());

                body.push(MIRStatement::CreateVariable {
                    var: MIRVariable {
                        name: Cow::Borrowed(identifier),
                        ty,
                        span: span.clone(),
                    },
                    value: None,
                    arg: false,
                    span,
                });
            }
            Rule::createSetVariable => {
                let mut data = pair.into_inner();

                let identifier = data.next().unwrap().as_str();
                let ty = parse_type(location, data.next().unwrap());
                let value = parse_expression(location, data.next().unwrap());

                body.push(MIRStatement::CreateVariable {
                    var: MIRVariable {
                        name: Cow::Borrowed(identifier),
                        ty,
                        span: span.clone(),
                    },
                    value: Some(value),
                    arg: false,
                    span: span.clone(),
                });
            }
            Rule::setVariable => {
                let mut data = pair.into_inner();

                let identifier = data.next().unwrap().as_str();
                let value = parse_expression(location, data.next().unwrap());

                body.push(MIRStatement::SetVariable {
                    name: Cow::Borrowed(identifier),
                    value,
                    span,
                });
            }
            Rule::functionCallDirect => {
                let mut data = pair.into_inner();

                let name_data = data.next().unwrap();
                let name = name_data.as_str();
                let args = data
                    .next()
                    .map_or(vec![], |args| parse_function_call_args(location, args));

                body.push(MIRStatement::FunctionCall(MIRFnCall {
                    source: MIRFnSource::Direct(
                        Cow::Borrowed(name),
                        to_span(location, name_data.as_span()),
                    ),
                    args,
                    ret_ty: None,
                    span,
                }));
            }
            Rule::functionCallIndirect => {
                let mut data = pair.into_inner();

                let ptr = parse_expression(location, data.next().unwrap());
                let args = data
                    .next()
                    .map_or(vec![], |args| parse_function_call_args(location, args));

                body.push(MIRStatement::FunctionCall(MIRFnCall {
                    source: MIRFnSource::Indirect(ptr),
                    args,
                    ret_ty: None,
                    span,
                }));
            }
            Rule::returnStmt => body.push(MIRStatement::Return {
                expr: pair
                    .into_inner()
                    .next()
                    .map(|v| parse_expression(location, v)),
                span,
            }),
            Rule::ifStatement => {
                body.push(parse_if_statement(location, pair));
            }
            Rule::continueStatement => {
                body.push(MIRStatement::ContinueStatement { span });
            }
            Rule::breakStatement => {
                body.push(MIRStatement::BreakStatement { span });
            }
            Rule::loopStatement => {
                let mut data = pair.into_inner();

                let loop_body = parse_function_body(location, data.next().unwrap());

                body.push(MIRStatement::LoopStatement {
                    body: loop_body,
                    span,
                });
            }
            _ => unreachable!(),
        }
    }

    body
}

fn parse_if_statement<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRStatement<'a> {
    assert_eq!(value.as_rule(), Rule::ifStatement);

    let span = to_span(location, value.as_span());

    let mut data = value.into_inner();

    let condition = parse_expression(location, data.next().unwrap());
    let on_true = parse_function_body(location, data.next().unwrap());
    let on_false = data.next().map_or(vec![], |v| parse_if_else(location, v));

    MIRStatement::IfStatement {
        condition,
        on_true,
        on_false,
        span,
    }
}

fn parse_if_else<'a>(location: &'a Path, value: Pair<'a, Rule>) -> Vec<MIRStatement<'a>> {
    assert_eq!(value.as_rule(), Rule::ifElse);

    let data = value.into_inner().next().unwrap();

    match data.as_rule() {
        Rule::ifStatement => vec![parse_if_statement(location, data)],
        Rule::functionBody => parse_function_body(location, data),
        _ => unreachable!(),
    }
}

fn parse_function_args<'a>(location: &'a Path, value: Pair<'a, Rule>) -> Vec<MIRVariable<'a>> {
    assert_eq!(value.as_rule(), Rule::functionArgs);

    let mut args = vec![];

    for pair in value.into_inner() {
        let span = to_span(location, pair.as_span());

        match pair.as_rule() {
            Rule::functionArg => {
                let mut data = pair.into_inner();

                let identifier = data.next().unwrap().as_str();
                let ty = parse_type(location, data.next().unwrap());

                args.push(MIRVariable {
                    name: Cow::Borrowed(identifier),
                    ty,
                    span,
                });
            }
            _ => unreachable!(),
        }
    }

    args
}

fn parse_function_call_args<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
) -> Vec<MIRExpression<'a>> {
    assert_eq!(value.as_rule(), Rule::functionCallArgs);

    let mut exprs = vec![];

    for pair in value.into_inner() {
        let _span = to_span(location, pair.as_span());

        match pair.as_rule() {
            Rule::expression => {
                exprs.push(parse_expression(location, pair));
            }
            _ => unreachable!(),
        }
    }

    exprs
}

fn parse_expression<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::expression);

    parse_logical(location, value.into_inner().next().unwrap())
}

fn parse_logical<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::logical);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let comp = parse_comparison(location, data.next().unwrap());

    let Some(op) = data.next() else {
        return comp;
    };

    let log = parse_logical(location, data.next().unwrap());

    let expr = match op.as_str() {
        "&&" => MIRExpressionInner::BoolAnd(Box::new(comp), Box::new(log)),
        "||" => MIRExpressionInner::BoolOr(Box::new(comp), Box::new(log)),
        _ => unreachable!(),
    };

    MIRExpression {
        inner: expr,
        ty: None,
        span,
    }
}

fn parse_comparison<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::comparison);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let add = parse_addition(location, data.next().unwrap());

    let Some(op) = data.next() else {
        return add;
    };

    let comp = parse_comparison(location, data.next().unwrap());

    let expr = match op.as_str() {
        "==" => MIRExpressionInner::Equal(Box::new(add), Box::new(comp)),
        "!=" => MIRExpressionInner::NotEqual(Box::new(add), Box::new(comp)),
        ">" => MIRExpressionInner::Greater(Box::new(add), Box::new(comp)),
        "<" => MIRExpressionInner::Less(Box::new(add), Box::new(comp)),
        ">=" => MIRExpressionInner::GreaterEq(Box::new(add), Box::new(comp)),
        "<=" => MIRExpressionInner::LessEq(Box::new(add), Box::new(comp)),
        _ => unreachable!(),
    };

    MIRExpression {
        inner: expr,
        ty: None,
        span,
    }
}

fn parse_addition<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::addition);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let mul = parse_multiplication(location, data.next().unwrap());

    let Some(op) = data.next() else {
        return mul;
    };

    let add = parse_addition(location, data.next().unwrap());

    let expr = match op.as_str() {
        "+" => MIRExpressionInner::Add(Box::new(mul), Box::new(add)),
        "-" => MIRExpressionInner::Sub(Box::new(mul), Box::new(add)),
        _ => unreachable!(),
    };

    MIRExpression {
        inner: expr,
        ty: None,
        span,
    }
}

fn parse_multiplication<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::multiplication);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let pri = parse_primary(location, data.next().unwrap());

    let Some(op) = data.next() else {
        return pri;
    };

    let mul = parse_multiplication(location, data.next().unwrap());

    let expr = match op.as_str() {
        "*" => MIRExpressionInner::Mul(Box::new(pri), Box::new(mul)),
        "/" => MIRExpressionInner::Div(Box::new(pri), Box::new(mul)),
        _ => unreachable!(),
    };

    MIRExpression {
        inner: expr,
        ty: None,
        span,
    }
}

fn parse_primary<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::primary);

    let span = to_span(location, value.as_span());
    let data = value.into_inner().next().unwrap();

    let mut ty = None;

    let expr = match data.as_rule() {
        Rule::number => {
            let res = parse_number(data);
            // Type ascription from the number literal.
            ty = res.1;

            MIRExpressionInner::Number(res.0)
        }
        Rule::string => MIRExpressionInner::String(Cow::Owned(parse_string(data))),
        Rule::functionCallDirect => {
            let mut data = data.into_inner();

            let name_data = data.next().unwrap();
            let name = name_data.as_str();
            let args = data
                .next()
                .map_or(vec![], |args| parse_function_call_args(location, args));

            MIRExpressionInner::FunctionCall(Box::new(MIRFnCall {
                source: MIRFnSource::Direct(
                    Cow::Borrowed(name),
                    to_span(location, name_data.as_span()),
                ),
                args,
                ret_ty: None,
                span: span.clone(),
            }))
        }
        Rule::functionCallIndirect => {
            let mut data = data.into_inner();

            let ptr = parse_expression(location, data.next().unwrap());
            let args = data
                .next()
                .map_or(vec![], |args| parse_function_call_args(location, args));

            MIRExpressionInner::FunctionCall(Box::new(MIRFnCall {
                source: MIRFnSource::Indirect(ptr),
                args,
                ret_ty: None,
                span: span.clone(),
            }))
        }
        Rule::identifier => MIRExpressionInner::Variable(Cow::Borrowed(data.as_str())),
        Rule::boolLiteral => MIRExpressionInner::Bool(data.as_str() == "true"),
        Rule::expression => {
            // Expand expression span to include parenthases.
            let mut expr = parse_expression(location, data);
            expr.span = span;

            return expr;
        }
        _ => unreachable!(),
    };

    MIRExpression {
        inner: expr,
        ty: ty.map(|ty| MIRType {
            ty,
            span: Some(span.clone()),
        }),
        span,
    }
}

fn parse_string<'a>(value: Pair<'a, Rule>) -> String {
    assert_eq!(value.as_rule(), Rule::string);

    let mut output = String::new();
    println!("{:?}", value);
    let parts = value.into_inner();

    for part in parts {
        match part.as_rule() {
            Rule::stringInner => {
                // No escape sequences.
                output += part.as_str();
            }
            Rule::stringEscape => {
                // Skip the "\"
                let escaped = part.into_inner().next().unwrap();
                match escaped.as_rule() {
                    Rule::stringUnicode => {
                        // Skip the "u"
                        let code = u32::from_str_radix(&escaped.as_str()[1..], 16).unwrap();
                        output.push(char::from_u32(code).unwrap());
                    }
                    Rule::stringNormal => {
                        // "\"" | "\\" | "/" | "b" | "f" | "n" | "r" | "t"
                        let c = match escaped.as_str() {
                            "\"" => '"',
                            "\\" => '\\',
                            "/" => '/',
                            "n" => '\n',
                            "r" => '\r',
                            "t" => '\t',
                            "0" => '\0',
                            _ => unreachable!(),
                        };

                        output.push(c);
                    }
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        }
    }

    output
}

fn parse_type<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRType<'a> {
    assert_eq!(value.as_rule(), Rule::variableType);

    let ty = match value.as_str() {
        "i32" => MIRTypeInner::I32,
        "u32" => MIRTypeInner::U32,
        "bool" => MIRTypeInner::Bool,
        "()" => MIRTypeInner::Unit,
        "string" => MIRTypeInner::String,
        _ => unreachable!(),
    };

    MIRType {
        ty,
        span: Some(to_span(location, value.as_span())),
    }
}

/// Parses a number, including type ascription.
/// Returns the number and a data type, if specified.
fn parse_number(value: Pair<'_, Rule>) -> (i128, Option<MIRTypeInner<'_>>) {
    assert_eq!(value.as_rule(), Rule::number);

    let mut value = value.as_str();
    let mut ty = None;

    if value.ends_with("i32") {
        value = &value[..value.len() - 3];
        ty = Some(MIRTypeInner::I32);
    } else if value.ends_with("u32") {
        value = &value[..value.len() - 3];
        ty = Some(MIRTypeInner::U32);
    }

    (value.parse::<i128>().unwrap(), ty)
}
