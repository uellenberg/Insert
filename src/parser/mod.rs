pub mod file_cache;
pub mod span;

use crate::mir::{
    MIRConstant, MIRContext, MIRDeclaration, MIRExpression, MIRExpressionInner, MIRFnCall,
    MIRFnSource, MIRFunction, MIRFunctionArgs, MIRFunctionType, MIRMarker, MIRStatement, MIRStatic,
    MIRType, MIRTypeInner, MIRVariable,
};
use crate::parser::file_cache::file_cache;
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
    let data = file_cache().get(location).unwrap();
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
    ctx: &mut MIRContext<'a>,
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
    ctx: &mut MIRContext<'a>,
) -> Option<Vec<MIRDeclaration<'a>>> {
    assert_eq!(value.as_rule(), Rule::declarations);

    let mut res = vec![];

    for pair in value.into_inner() {
        match pair.as_rule() {
            Rule::constDeclaration => {
                res.push(MIRDeclaration::Constant(parse_constant(
                    location, pair, ctx,
                )));
            }
            Rule::staticDeclaration => {
                res.push(MIRDeclaration::Static(parse_static(location, pair, ctx)));
            }
            Rule::functionDeclaration => {
                res.push(MIRDeclaration::Function(parse_function(
                    location, pair, ctx,
                )?));
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
            Rule::markerStatement => {
                res.push(MIRDeclaration::Marker(parse_marker(location, pair)));
            }
            _ => unreachable!(),
        }
    }

    Some(res)
}

fn parse_static<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> MIRStatic<'a> {
    assert_eq!(value.as_rule(), Rule::staticDeclaration);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let identifier = data.next().unwrap().as_str();
    let ty = parse_type(location, data.next().unwrap());
    let expr = parse_expression(location, data.next().unwrap(), ctx);

    MIRStatic {
        name: Cow::Borrowed(identifier),
        ty,
        value: expr,
        span,
    }
}

fn parse_marker<'a>(location: &'a Path, value: Pair<'a, Rule>) -> MIRMarker<'a> {
    assert_eq!(value.as_rule(), Rule::markerStatement);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let identifier = data.next().unwrap().as_str();

    MIRMarker {
        name: Cow::Borrowed(identifier),
        span,
    }
}

fn parse_constant<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> MIRConstant<'a> {
    assert_eq!(value.as_rule(), Rule::constDeclaration);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let identifier = data.next().unwrap().as_str();
    let ty = parse_type(location, data.next().unwrap());
    let expr = parse_expression(location, data.next().unwrap(), ctx);

    MIRConstant {
        name: Cow::Borrowed(identifier),
        ty,
        value: expr,
        span,
    }
}

fn parse_function<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> Option<MIRFunction<'a>> {
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
                args = parse_function_args(location, pair, ctx);
            }
            Rule::functionReturn => {
                // functionReturn([type])
                ret = parse_type(location, pair.into_inner().next().unwrap());
            }
            Rule::functionBody => {
                // Function body is the last item.
                return Some(MIRFunction {
                    name: Cow::Borrowed(identifier),
                    fn_type,
                    args_ty: MIRFunctionArgs {
                        args: args.iter().map(|v| v.ty.ty.clone()).collect(),
                        variadic: false,
                    },
                    args,
                    ret_ty: ret,
                    body: parse_function_body(location, pair, ctx)?,
                    span,
                    extern_import: None,
                });
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
    ctx: &mut MIRContext<'a>,
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

    if file_cache().exists(&import_path) {
        // We already imported this file, so return
        // nothing here to avoid duplicating declarations.
        return Some(vec![]);
    }

    let data = file_cache().get(&import_path).unwrap();
    // Leaking is okay here, since we only do it once per file.
    parse_data(import_path.leak(), data, ctx)
}

fn parse_target<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
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

fn parse_function_body<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> Option<Vec<MIRStatement<'a>>> {
    assert_eq!(value.as_rule(), Rule::functionBody);

    let mut body = vec![];

    for pair in value.into_inner() {
        body.push(parse_statement(location, pair, ctx)?);
    }
    Some(body)
}

fn parse_statement<'a>(
    location: &'a Path,
    pair: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> Option<MIRStatement<'a>> {
    let span = to_span(location, pair.as_span());

    match pair.as_rule() {
        Rule::createVariable => {
            let mut data = pair.into_inner();

            let identifier = data.next().unwrap().as_str();
            let ty = parse_type(location, data.next().unwrap());

            Some(MIRStatement::CreateVariable {
                var: MIRVariable {
                    name: Cow::Borrowed(identifier),
                    ty,
                    span: span.clone(),
                    var_idx: None,
                    arg: false,
                },
                value: None,
                span,
            })
        }
        Rule::createSetVariable => {
            let mut data = pair.into_inner();

            let identifier = data.next().unwrap().as_str();
            let ty = parse_type(location, data.next().unwrap());
            let value = parse_expression(location, data.next().unwrap(), ctx);

            Some(MIRStatement::CreateVariable {
                var: MIRVariable {
                    name: Cow::Borrowed(identifier),
                    ty,
                    span: span.clone(),
                    var_idx: None,
                    arg: false,
                },
                value: Some(value),
                span: span.clone(),
            })
        }
        Rule::setVariable => {
            let mut data = pair.into_inner();

            let place = parse_place_expr(location, data.next().unwrap(), ctx);
            let value = parse_expression(location, data.next().unwrap(), ctx);

            Some(MIRStatement::SetVariable { place, value, span })
        }
        Rule::functionCallDirect => {
            let mut data = pair.into_inner();

            let name_data = data.next().unwrap();
            let name = name_data.as_str();
            let args = data
                .next()
                .map_or(vec![], |args| parse_function_call_args(location, args, ctx));

            Some(MIRStatement::FunctionCall(MIRFnCall {
                source: MIRFnSource::Direct(
                    Cow::Borrowed(name),
                    to_span(location, name_data.as_span()),
                ),
                args,
                args_ty: None,
                ret_ty: None,
                span,
            }))
        }
        Rule::functionCallIndirect => {
            let mut data = pair.into_inner();

            let ptr = parse_expression(location, data.next().unwrap(), ctx);
            let args = data
                .next()
                .map_or(vec![], |args| parse_function_call_args(location, args, ctx));

            Some(MIRStatement::FunctionCall(MIRFnCall {
                source: MIRFnSource::Indirect(ptr),
                args,
                args_ty: None,
                ret_ty: None,
                span,
            }))
        }
        Rule::returnStmt => Some(MIRStatement::Return {
            expr: pair
                .into_inner()
                .next()
                .map(|v| parse_expression(location, v, ctx)),
            span,
        }),
        Rule::ifStatement => parse_if_statement(location, pair, ctx),
        Rule::continueStatement => Some(MIRStatement::ContinueStatement { span }),
        Rule::breakStatement => Some(MIRStatement::BreakStatement { span }),
        Rule::loopStatement => {
            let mut data = pair.into_inner();
            let loop_body = parse_function_body(location, data.next().unwrap(), ctx)?;

            Some(MIRStatement::LoopStatement {
                condition: None,
                body: loop_body,
                iterate: vec![],
                span,
            })
        }
        Rule::whileStatement => {
            let mut data = pair.into_inner();
            let condition = parse_expression(location, data.next().unwrap(), ctx);
            let loop_body = parse_function_body(location, data.next().unwrap(), ctx)?;

            Some(MIRStatement::LoopStatement {
                condition: Some(condition),
                body: loop_body,
                iterate: vec![],
                span,
            })
        }
        Rule::forStatement => {
            let mut data = pair.into_inner();
            let init_pair = data.next().unwrap();
            let cond_pair = data.next().unwrap();
            let iterate_pair = data.next().unwrap();
            let body_pair = data.next().unwrap();

            let init_stmt = match init_pair.as_rule() {
                Rule::forLoopEmpty => None,
                _ => Some(parse_statement(location, init_pair, ctx)?),
            };

            let condition = match cond_pair.as_rule() {
                Rule::forLoopEmpty => None,
                Rule::expression => Some(parse_expression(location, cond_pair, ctx)),
                _ => unreachable!(),
            };

            let iterate_stmt = match iterate_pair.as_rule() {
                Rule::forLoopEmpty => None,
                _ => Some(parse_statement(location, iterate_pair, ctx)?),
            };

            let loop_body = parse_function_body(location, body_pair, ctx)?;

            // For loops are desugared to a scope containing their initializer
            // and a while loop.
            // This makes MIR much simpler, with a small cost during codegen
            // to get it properly optimized.
            //
            // for let i: u32 = 0; i < 10; i = i + 1 {
            //     i = i + 2;
            // }
            //
            // Desugars to
            //
            // scope {
            //     let i: u32 = 0;
            //     while i < 10 {
            //         i = i + 2;
            //     } iterate { i = i + 1 }
            // }
            let mut scope_body = vec![];
            if let Some(init) = init_stmt {
                scope_body.push(init);
            }
            scope_body.push(MIRStatement::LoopStatement {
                condition,
                body: loop_body,
                iterate: iterate_stmt.into_iter().collect(),
                span: span.clone(),
            });

            Some(MIRStatement::ScopeStatement {
                body: scope_body,
                span,
            })
        }
        Rule::markerStatement => {
            let marker = parse_marker(location, pair);
            // Even though markers can live inside functions, they're
            // always global/unique.
            //
            // We need to manually register it here, since only
            // global/outer declarations are automatically registered.
            ctx.register(MIRDeclaration::Marker(marker.clone()))?;
            Some(MIRStatement::MarkerStatement {
                name: marker.name,
                span: marker.span,
            })
        }
        _ => unreachable!(),
    }
}

fn parse_if_statement<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> Option<MIRStatement<'a>> {
    assert_eq!(value.as_rule(), Rule::ifStatement);

    let span = to_span(location, value.as_span());

    let mut data = value.into_inner();

    let condition = parse_expression(location, data.next().unwrap(), ctx);
    let on_true = parse_function_body(location, data.next().unwrap(), ctx)?;
    let on_false = data
        .next()
        .and_then(|v| parse_if_else(location, v, ctx))
        .unwrap_or(vec![]);

    Some(MIRStatement::IfStatement {
        condition,
        on_true,
        on_false,
        span,
    })
}

fn parse_if_else<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> Option<Vec<MIRStatement<'a>>> {
    assert_eq!(value.as_rule(), Rule::ifElse);

    let data = value.into_inner().next().unwrap();

    match data.as_rule() {
        Rule::ifStatement => Some(vec![parse_if_statement(location, data, ctx)?]),
        Rule::functionBody => parse_function_body(location, data, ctx),
        _ => unreachable!(),
    }
}

fn parse_function_args<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    _ctx: &mut MIRContext<'a>,
) -> Vec<MIRVariable<'a>> {
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
    ctx: &mut MIRContext<'a>,
) -> Vec<MIRExpression<'a>> {
    assert_eq!(value.as_rule(), Rule::functionCallArgs);

    let mut exprs = vec![];

    for pair in value.into_inner() {
        let _span = to_span(location, pair.as_span());

        match pair.as_rule() {
            Rule::expression => {
                exprs.push(parse_expression(location, pair, ctx));
            }
            _ => unreachable!(),
        }
    }

    exprs
}

fn parse_expression<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::expression);

    parse_ternary(location, value.into_inner().next().unwrap(), ctx)
}

fn parse_ternary<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::ternary);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    // This is either a ternary or just a normal expression.
    let condition = parse_logical(location, data.next().unwrap(), ctx);

    if let Some(on_true_pair) = data.next() {
        let on_true = parse_expression(location, on_true_pair, ctx);
        let on_false = parse_expression(location, data.next().unwrap(), ctx);

        MIRExpression {
            inner: MIRExpressionInner::Ternary(
                Box::new(condition),
                Box::new(on_true),
                Box::new(on_false),
            ),
            ty: None,
            span,
        }
    } else {
        condition
    }
}

fn parse_logical<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::logical);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let mut lhs = parse_comparison(location, data.next().unwrap(), ctx);

    while let Some(op) = data.next() {
        let rhs = parse_comparison(location, data.next().unwrap(), ctx);
        let expr = match op.as_str() {
            "&&" => MIRExpressionInner::BoolAnd(Box::new(lhs), Box::new(rhs)),
            "||" => MIRExpressionInner::BoolOr(Box::new(lhs), Box::new(rhs)),
            _ => unreachable!(),
        };
        lhs = MIRExpression {
            inner: expr,
            ty: None,
            span: span.clone(),
        };
    }

    lhs
}

fn parse_comparison<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::comparison);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let mut lhs = parse_addition(location, data.next().unwrap(), ctx);

    while let Some(op) = data.next() {
        let rhs = parse_addition(location, data.next().unwrap(), ctx);
        let expr = match op.as_str() {
            "==" => MIRExpressionInner::Equal(Box::new(lhs), Box::new(rhs)),
            "!=" => MIRExpressionInner::NotEqual(Box::new(lhs), Box::new(rhs)),
            ">" => MIRExpressionInner::Greater(Box::new(lhs), Box::new(rhs)),
            "<" => MIRExpressionInner::Less(Box::new(lhs), Box::new(rhs)),
            ">=" => MIRExpressionInner::GreaterEq(Box::new(lhs), Box::new(rhs)),
            "<=" => MIRExpressionInner::LessEq(Box::new(lhs), Box::new(rhs)),
            _ => unreachable!(),
        };
        lhs = MIRExpression {
            inner: expr,
            ty: None,
            span: span.clone(),
        };
    }

    lhs
}

fn parse_addition<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::addition);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let mut lhs = parse_multiplication(location, data.next().unwrap(), ctx);

    while let Some(op) = data.next() {
        let rhs = parse_multiplication(location, data.next().unwrap(), ctx);
        let expr = match op.as_str() {
            "+" => MIRExpressionInner::Add(Box::new(lhs), Box::new(rhs)),
            "-" => MIRExpressionInner::Sub(Box::new(lhs), Box::new(rhs)),
            _ => unreachable!(),
        };
        lhs = MIRExpression {
            inner: expr,
            ty: None,
            span: span.clone(),
        };
    }

    lhs
}

fn parse_multiplication<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::multiplication);

    let span = to_span(location, value.as_span());
    let mut data = value.into_inner();

    let mut lhs = parse_primary(location, data.next().unwrap(), ctx);

    while let Some(op) = data.next() {
        let rhs = parse_primary(location, data.next().unwrap(), ctx);
        let expr = match op.as_str() {
            "*" => MIRExpressionInner::Mul(Box::new(lhs), Box::new(rhs)),
            "/" => MIRExpressionInner::Div(Box::new(lhs), Box::new(rhs)),
            _ => unreachable!(),
        };
        lhs = MIRExpression {
            inner: expr,
            ty: None,
            span: span.clone(),
        };
    }

    lhs
}

fn parse_primary<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> MIRExpression<'a> {
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
        Rule::char => MIRExpressionInner::Char(parse_char(data)),
        Rule::functionCallDirect => {
            let mut data = data.into_inner();

            let name_data = data.next().unwrap();
            let name = name_data.as_str();
            let args = data
                .next()
                .map_or(vec![], |args| parse_function_call_args(location, args, ctx));

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

            let ptr = parse_expression(location, data.next().unwrap(), ctx);
            let args = data
                .next()
                .map_or(vec![], |args| parse_function_call_args(location, args, ctx));

            MIRExpressionInner::FunctionCall(Box::new(MIRFnCall {
                source: MIRFnSource::Indirect(ptr),
                args,
                args_ty: None,
                ret_ty: None,
                span: span.clone(),
            }))
        }
        Rule::placeExpr => {
            return parse_place_expr(location, data, ctx);
        }
        Rule::boolLiteral => MIRExpressionInner::Bool(data.as_str() == "true"),
        Rule::quine => MIRExpressionInner::Quine,
        Rule::quineLen => MIRExpressionInner::QuineLen,
        Rule::quineSpace => MIRExpressionInner::QuineSpace,
        Rule::quineLine => MIRExpressionInner::QuineLine,
        Rule::bindingExpr => {
            let mut inner = data.into_inner();
            let name = inner.next().unwrap().as_str();
            let expr = parse_expression(location, inner.next().unwrap(), ctx);

            let left = MIRMarker {
                name: name.into(),
                span: span.clone(),
            };
            let right = MIRMarker {
                name: Cow::Owned(format!("$binding_right_{name}")),
                span: span.clone(),
            };

            ctx.register(MIRDeclaration::Marker(left.clone()));
            ctx.register(MIRDeclaration::Marker(right.clone()));

            MIRExpressionInner::Binding(left, Box::new(expr), right)
        }
        Rule::arrayExpr => {
            let inner = data
                .into_inner()
                .map(|data| parse_expression(location, data, ctx))
                .collect::<Vec<_>>();

            MIRExpressionInner::Array(inner)
        }
        Rule::expression => {
            // Expand expression span to include parenthases.
            let mut expr = parse_expression(location, data, ctx);
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

fn parse_place_expr<'a>(
    location: &'a Path,
    value: Pair<'a, Rule>,
    ctx: &mut MIRContext<'a>,
) -> MIRExpression<'a> {
    assert_eq!(value.as_rule(), Rule::placeExpr);

    let span_orig = value.as_span();
    let span = to_span(location, span_orig);
    let mut data = value.into_inner();

    let first = data.next().unwrap();

    // Prefix expression (*expr or &expr).
    // This is handled left-to-right in the grammar, so the left-most expression
    // is at the top of the tree (evaluates last).
    if first.as_rule() == Rule::placePrefix {
        let inner = parse_primary(location, data.next().unwrap(), ctx);
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
        Rule::primary => parse_primary(location, first, ctx),
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
                let index_expr =
                    parse_expression(location, inner.into_inner().next().unwrap(), ctx);
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
            Rule::charEscape => {
                output.push(parse_char_escape(part));
            }
            _ => unreachable!(),
        }
    }

    output
}

fn parse_char<'a>(value: Pair<'a, Rule>) -> char {
    assert_eq!(value.as_rule(), Rule::char);

    let mut parts = value.into_inner();

    let part = parts.next().unwrap();
    match part.as_rule() {
        Rule::charInner => {
            // No escape sequences.
            part.as_str().chars().nth(0).unwrap()
        }
        Rule::charEscape => parse_char_escape(part),
        _ => unreachable!(),
    }
}

fn parse_char_escape<'a>(value: Pair<'a, Rule>) -> char {
    assert_eq!(value.as_rule(), Rule::charEscape);

    // Skip the "\"
    let escaped = value.into_inner().next().unwrap();
    match escaped.as_rule() {
        Rule::charUnicode => {
            // Skip the "u"
            let code = u32::from_str_radix(&escaped.as_str()[1..], 16).unwrap();
            char::from_u32(code).unwrap()
        }
        Rule::charNormal => {
            // "\"" | "'" | "\\" | "/" | "b" | "f" | "n" | "r" | "t"
            match escaped.as_str() {
                "\"" => '"',
                "'" => '\'',
                "\\" => '\\',
                "/" => '/',
                "n" => '\n',
                "r" => '\r',
                "t" => '\t',
                "0" => '\0',
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
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
