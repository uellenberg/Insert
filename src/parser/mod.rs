pub mod file_cache;
pub mod span;

use crate::mir::{
    MIRConstant, MIRContext, MIRDeclaration, MIRExpression, MIRExpressionInner, MIRFnCall,
    MIRFnSource, MIRFunction, MIRFunctionArgs, MIRFunctionType, MIRStatement, MIRStatic, MIRType,
    MIRTypeInner, MIRVariable,
};
use pest::Parser;
use pest::iterators::Pair;
use pest_derive::Parser;
use span::to_span;
use std::borrow::Cow;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[grammar = "parser/program.pest"]
struct InsertParser;

/// Parses a file into MIR,
/// returning whether it was successful.
pub fn parse_file<'a>(location: &'a Path, ctx: &mut MIRContext<'a>) -> bool {
    let data = ctx.file_cache.get(location).unwrap();
    let Some(decls) = parse_data(location, data, ctx) else {
        return false;
    };

    for decl in decls {
        let Some(key) = ctx.register(decl) else {
            // Registration failed (duplicate identifier).
            // Any error was already printed.
            return false;
        };

        ctx.push_decl(key);
    }

    true
}

/// Parses some data file into MIR,
/// returning whether it was successful.
fn parse_data<'a>(
    location: &'a Path,
    data: &'a str,
    ctx: &MIRContext<'a>,
) -> Option<Vec<MIRDeclaration<'a>>> {
    let ast = match InsertParser::parse(Rule::program, data) {
        Ok(ast) => ast,
        Err(err) => {
            eprintln!("{err}");
            return None;
        }
    };

    for pair in ast {
        match pair.as_rule() {
            Rule::declarations => {
                return parse_declarations(location, pair, ctx);
            }
            Rule::EOI => {}
            _ => unreachable!(),
        }
    }

    unreachable!("No declarations found!");
}

fn parse_declarations<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &MIRContext<'a>,
) -> Option<Vec<MIRDeclaration<'a>>> {
    assert_eq!(value.as_rule(), Rule::declarations);

    let mut res = vec![];

    for pair in value.into_inner() {
        match pair.as_rule() {
            Rule::constDeclaration => {
                res.push(MIRDeclaration::Constant(parse_constant(location, pair)));
            }
            Rule::staticDeclaration => {
                res.push(MIRDeclaration::Static(parse_static(location, pair)));
            }
            Rule::functionDeclaration => {
                res.push(MIRDeclaration::Function(parse_function(location, pair)));
            }
            Rule::externFunctionDeclaration => {
                res.push(MIRDeclaration::Function(parse_extern_function(
                    location, pair,
                )));
            }
            Rule::importDeclaration => {
                res.extend(parse_import(location, pair, ctx)?);
            }
            Rule::targetDeclaration => {
                res.extend(parse_target(location, pair, ctx)?);
            }
            _ => unreachable!(),
        }
    }

    Some(res)
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
                    args_ty: MIRFunctionArgs {
                        args: args.iter().map(|v| v.ty.ty.clone()).collect(),
                        variadic: false,
                    },
                    args,
                    ret_ty: ret,
                    body: parse_function_body(location, pair),
                    span,
                    extern_import: None,
                };
            }
            _ => unreachable!(),
        }
    }

    // No function body.
    unreachable!();
}

fn parse_extern_function<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRFunction<'a> {
    assert_eq!(value.as_rule(), Rule::externFunctionDeclaration);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let identifier = data.next().unwrap().as_str();

    let mut args = vec![];
    let mut variadic = false;
    let mut ret = MIRType {
        ty: MIRTypeInner::Unit,
        span: None,
    };

    for pair in data {
        match pair.as_rule() {
            Rule::externFunctionArgs => {
                (args, variadic) = parse_extern_function_args(location, pair);
            }
            Rule::functionReturn => {
                ret = parse_type(location, pair.into_inner().next().unwrap());
            }
            Rule::string => {
                // Import path is the last item.
                let import = parse_string(pair);

                return MIRFunction {
                    name: Cow::Borrowed(identifier),
                    fn_type: MIRFunctionType::Extern,
                    args_ty: MIRFunctionArgs {
                        args: args.iter().map(|v| v.ty.ty.clone()).collect(),
                        variadic,
                    },
                    args,
                    ret_ty: ret,
                    body: vec![],
                    span,
                    extern_import: Some(Cow::Owned(import)),
                };
            }
            _ => unreachable!(),
        }
    }

    unreachable!();
}

fn parse_extern_function_args<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
) -> (Vec<MIRVariable<'a>>, bool) {
    assert_eq!(value.as_rule(), Rule::externFunctionArgs);

    let mut args = vec![];
    let mut variadic = false;

    for pair in value.into_inner() {
        match pair.as_rule() {
            Rule::functionArg => {
                let span = to_span(location, pair.as_span());
                let mut data = pair.into_inner();

                let identifier = data.next().unwrap().as_str();
                let ty = parse_type(location, data.next().unwrap());

                args.push(MIRVariable {
                    name: Cow::Borrowed(identifier),
                    ty,
                    span,
                    var_idx: None,
                    arg: true,
                });
            }
            Rule::variadic => {
                variadic = true;
            }
            _ => {}
        }
    }

    (args, variadic)
}

fn parse_import<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &MIRContext<'a>,
) -> Option<Vec<MIRDeclaration<'a>>> {
    assert_eq!(value.as_rule(), Rule::importDeclaration);

    let value = PathBuf::from(parse_string(value.into_inner().next().unwrap()));
    let import_path = if value.starts_with("./") || value.starts_with("../") || value.is_absolute()
    {
        // Relative or absolute path.
        // These are standard fs path imports, and should be imported relative to the
        // current file.
        location
            .parent()
            .expect("File path had no parent!")
            .join(value)
    } else {
        // Module import path.
        value
    };

    // Normalize the import, since join won't resolve "../" etc, and this
    // could create duplicate declarations.
    // We can't canonicalize the path, since we want to preserve "std/..." paths.
    let import_path = import_path
        .normalize_lexically()
        .expect("Failed to normalize import path!");

    if ctx.file_cache.exists(&import_path) {
        // We already imported this file, so return
        // nothing here to avoid duplicating declarations.
        return Some(vec![]);
    }

    let data = ctx.file_cache.get(&import_path).unwrap();
    // Leaking is okay here, since we only do it once per file.
    parse_data(import_path.leak(), data, ctx)
}

fn parse_target<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &MIRContext<'a>,
) -> Option<Vec<MIRDeclaration<'a>>> {
    assert_eq!(value.as_rule(), Rule::targetDeclaration);

    let mut values = value.into_inner();

    let name = parse_string(values.next().unwrap());
    if name != ctx.target.name() {
        // Not the right target.
        return Some(vec![]);
    }

    parse_declarations(location, values.next().unwrap(), ctx)
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
                        var_idx: None,
                        arg: false,
                    },
                    value: None,
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
                        var_idx: None,
                        arg: false,
                    },
                    value: Some(value),
                    span: span.clone(),
                });
            }
            Rule::setVariable => {
                let mut data = pair.into_inner();

                let place = parse_place_expr(location, data.next().unwrap());
                let value = parse_expression(location, data.next().unwrap());

                body.push(MIRStatement::SetVariable { place, value, span });
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
                    args_ty: None,
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
                    args_ty: None,
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
                    var_idx: None,
                    arg: true,
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
                args_ty: None,
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
                args_ty: None,
                ret_ty: None,
                span: span.clone(),
            }))
        }
        Rule::placeExpr => {
            return parse_place_expr(location, data);
        }
        Rule::boolLiteral => MIRExpressionInner::Bool(data.as_str() == "true"),
        Rule::arrayExpr => {
            let mut data = data.into_inner();
            let inner = data
                .map(|data| parse_expression(location, data))
                .collect::<Vec<_>>();

            MIRExpressionInner::Array(inner)
        }
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

fn parse_place_expr<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::placeExpr);

    let span_orig = value.as_span();
    let span = to_span(location, span_orig);
    let mut data = value.into_inner();

    let first = data.next().unwrap();

    // Prefix expression (*expr or &expr).
    // This is handled left-to-right in the grammar, so the left-most expression
    // is at the top of the tree (evaluates last).
    if first.as_rule() == Rule::placePrefix {
        let inner = parse_primary(location, data.next().unwrap());
        let expr = match first.as_str() {
            "*" => MIRExpressionInner::Deref(Box::new(inner)),
            "&" => MIRExpressionInner::Ref(Box::new(inner)),
            _ => unreachable!(),
        };

        return MIRExpression {
            inner: expr,
            ty: None,
            span,
        };
    }

    // Otherwise, we have a base (identifier or parenthesized expression) followed by postfixes.
    let mut current = match first.as_rule() {
        Rule::identifier => MIRExpression {
            inner: MIRExpressionInner::Variable(Cow::Borrowed(first.as_str()), None),
            ty: None,
            span: to_span(location, first.as_span()),
        },
        Rule::primary => parse_primary(location, first),
        _ => unreachable!(),
    };

    // The MIR should have the right-most postfix at the top of the tree (evaluate it last),
    // so we need to build it up from left-to-right.
    for postfix in data {
        assert_eq!(postfix.as_rule(), Rule::placePostfix);
        let postfix_span = to_span(
            location,
            // We need to construct a span of the full expression, which
            // starts from the placeExpr and ends at this postfix.
            //
            // For example, if we're looking at:
            // a.x[y].z
            //    ^^^
            // We want the span to be "a.x[y]", not "[y]".
            pest::Span::new(
                span_orig.get_input(),
                span_orig.start(),
                postfix.as_span().end(),
            )
            .unwrap(),
        );
        let inner = postfix.into_inner().next().unwrap();

        current = match inner.as_rule() {
            Rule::memberAccess => {
                let field = inner.into_inner().next().unwrap().as_str();
                MIRExpression {
                    inner: MIRExpressionInner::Member(Box::new(current), Cow::Borrowed(field)),
                    ty: None,
                    span: postfix_span,
                }
            }
            Rule::indexAccess => {
                let index_expr = parse_expression(location, inner.into_inner().next().unwrap());
                MIRExpression {
                    inner: MIRExpressionInner::Index(Box::new(current), Box::new(index_expr)),
                    ty: None,
                    span: postfix_span,
                }
            }
            _ => unreachable!(),
        };
    }

    current
}

fn parse_string<'a>(value: Pair<'a, Rule>) -> String {
    assert_eq!(value.as_rule(), Rule::string);

    let mut output = String::new();
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

    let ty_str = value.as_str();
    let span = Some(to_span(location, value.as_span()));

    let ty = if let Some(inner) = value.into_inner().next() {
        match inner.as_rule() {
            Rule::refType => MIRTypeInner::Ref(Box::new(
                parse_type(location, inner.into_inner().next().unwrap()).ty,
            )),
            Rule::arrayType => MIRTypeInner::Array(Box::new(
                parse_type(location, inner.into_inner().next().unwrap()).ty,
            )),
            Rule::fixedArrayType => {
                let mut inner = inner.into_inner();
                let inner_ty = parse_type(location, inner.next().unwrap()).ty;
                let array_size = parse_number(inner.next().unwrap()).0;

                MIRTypeInner::ArrayFixed(Box::new(inner_ty), array_size as usize)
            }
            Rule::fnType => {
                let inner = inner.into_inner();

                let mut variadic = false;
                let mut args = vec![];
                let mut ret = None;

                // Optional args, then a return type.
                for v in inner {
                    match v.as_rule() {
                        Rule::fnArgsType => {
                            // Zero or more args, then an optional variadic.
                            args = v
                                .into_inner()
                                .filter(|v| {
                                    if v.as_rule() == Rule::variadic {
                                        variadic = true;
                                        false
                                    } else {
                                        true
                                    }
                                })
                                .map(|v| parse_type(location, v).ty)
                                .collect::<Vec<_>>();
                        }
                        Rule::variableType => {
                            // Return type.
                            ret = Some(parse_type(location, v).ty);
                        }
                        _ => unreachable!(),
                    }
                }

                MIRTypeInner::FunctionPtr(
                    MIRFunctionArgs { args, variadic },
                    Box::new(ret.unwrap()),
                )
            }
            Rule::variableType => parse_type(location, inner).ty,
            _ => unreachable!(),
        }
    } else {
        match ty_str {
            "i32" => MIRTypeInner::I32,
            "u32" => MIRTypeInner::U32,
            "bool" => MIRTypeInner::Bool,
            "()" => MIRTypeInner::Unit,
            "string" => MIRTypeInner::String,
            "char" => MIRTypeInner::Char,
            _ => unreachable!(),
        }
    };

    MIRType { ty, span }
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
