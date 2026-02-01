use crate::codegen::Codegen;
use crate::codegen::LowerOptions;
use crate::codegen::c::token::{
    INDENT, LEFT_BRACKET, LEFT_PAREN, LEFT_SQUIGGLE, NEWLINE, NEWLINE_REQUIRED, RIGHT_BRACKET,
    RIGHT_PAREN, RIGHT_SQUIGGLE, SEMI, escape_string,
};
use crate::codegen::token::{Token, TokenInfo, Tokens, spread, strip_fancy_tokens};
use crate::mir::{
    MIRDeclarationKey, MIRExpression, MIRExpressionInner, MIRFnSource, MIRFunction,
    MIRFunctionType, MIRProgram, MIRStatement, MIRStatic, MIRType, MIRTypeInner, MIRVariable,
};
use std::borrow::Cow;

pub const C: &'static dyn Codegen = &CLowerer { indent_level: 0 };

#[derive(Default, Debug, Clone)]
pub struct CLowerer {
    /// The current indentation level.
    /// This represents the number of tabs of indentation (not the number of spaces).
    indent_level: u32,
}

impl Codegen for CLowerer {
    fn new(&self) -> Box<dyn Codegen> {
        Box::new(CLowerer::default())
    }

    fn lower_program(&mut self, program: &MIRProgram, options: LowerOptions) -> String {
        let mut output = spread![];

        output.extend(self.lower_imports(&program.required_imports));

        for val in &program.decls {
            match val {
                MIRDeclarationKey::Static(val) => {
                    output.extend(self.lower_static(&program.statics[*val]))
                }
                MIRDeclarationKey::Function(val) => {
                    // Skip extern functions (they have no body to emit)
                    if program.functions[*val].fn_type != MIRFunctionType::Extern {
                        output.extend(self.lower_function(&program.functions[*val]))
                    }
                }
                // Constants are never exported.
                MIRDeclarationKey::Constant(_) => {}
            }
        }

        if !options.fancy {
            strip_fancy_tokens(&mut output);
        }
        self.merge_tokens(&mut output);

        let mut output_str = String::new();
        let mut iter = output.iter().peekable();
        while let Some(token) = iter.next() {
            let Some(token_text) = &token.text else {
                continue;
            };

            output_str.push_str(token_text);

            // Add space if needed to allow compilation.
            if let Some(next) = iter.peek()
                && self.needs_space_between(token, next)
            {
                output_str.push(' ');
            }
        }
        output_str
    }

    fn lower_function<'a>(&mut self, func: &MIRFunction<'a>) -> Tokens<'a> {
        let decorated = self.decorate_with_type(func.name.clone(), &func.ret_ty);
        let args = func
            .args
            .iter()
            .map(|arg| self.decorate_with_type(arg.name.clone(), &arg.ty))
            .intersperse(spread![Token::new(",".into())])
            .flatten()
            .collect::<Tokens<'a>>();
        let block = self.lower_block(&func.body);

        spread![...decorated, LEFT_PAREN, ...args, RIGHT_PAREN, LEFT_SQUIGGLE, NEWLINE, ...block, RIGHT_SQUIGGLE, NEWLINE]
    }

    fn lower_block<'a>(&mut self, block: &[MIRStatement<'a>]) -> Tokens<'a> {
        // Items inside a block ({ ... }) should be indented.
        let pre_indent = self.indent_level;
        self.indent_level = pre_indent + 1;

        let indent = indent_tokens(pre_indent + 1);

        let ret = block
            .iter()
            // Remove None values.
            .filter_map(|v| self.lower_statement(v))
            .flat_map(|v| spread![...indent.clone(), ...v, NEWLINE])
            .collect::<Tokens<'a>>();

        self.indent_level = pre_indent;

        ret
    }

    fn lower_statement<'a>(&mut self, stmt: &MIRStatement<'a>) -> Option<Tokens<'a>> {
        let indent = self.indent_level;

        match stmt {
            // Just for analysis, no real codegen.
            MIRStatement::CreateVariable {
                var: MIRVariable { arg: true, .. },
                ..
            }
            | MIRStatement::DropVariable(..) => None,

            MIRStatement::CreateVariable { var, value, .. } => {
                let decorated = self.decorate_with_type(var.name.clone(), &var.ty);

                if let Some(value) = value {
                    let expr = self.lower_expression(value);

                    Some(spread![
                        ...decorated,
                        Token::new("=".into()),
                        ...expr,
                        SEMI,
                    ])
                } else {
                    Some(spread![...decorated, SEMI,])
                }
            }

            MIRStatement::SetVariable { place, value, .. } => {
                let place = self.lower_expression(place);
                let expr = self.lower_expression(value);

                Some(spread![
                    ...place,
                    Token::new("=".into()),
                    ...expr,
                    SEMI,
                ])
            }

            MIRStatement::FunctionCall(call) => {
                let fn_src = self.lower_fn_source(&call.source);
                let args = call
                    .args
                    .iter()
                    .map(|v| self.lower_expression(v))
                    .intersperse(spread![Token::new(",".into())])
                    .flatten()
                    .collect::<Tokens<'a>>();

                Some(spread![...fn_src, LEFT_PAREN, ...args, RIGHT_PAREN, SEMI,])
            }

            MIRStatement::Return { expr, .. } => {
                if let Some(expr) = expr {
                    let ret_expr = self.lower_expression(expr);

                    Some(spread![Token::new("return".into()), ...ret_expr, SEMI,])
                } else {
                    Some(spread![Token::new("return".into()), SEMI])
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
                let cond = self.lower_expression(condition);
                let true_block = self.lower_block(on_true);

                if on_false.is_empty() {
                    Some(spread![
                        Token::new("if".into()),
                        LEFT_PAREN,
                        ...cond,
                        RIGHT_PAREN,
                        LEFT_SQUIGGLE,
                        NEWLINE,
                        ...true_block,
                        ...indent_tokens(indent),
                        RIGHT_SQUIGGLE,
                    ])
                } else {
                    let false_block = self.lower_block(on_false);

                    Some(spread![
                        Token::new("if".into()),
                        LEFT_PAREN,
                        ...cond,
                        RIGHT_PAREN,
                        LEFT_SQUIGGLE,
                        NEWLINE,
                        ...true_block,
                        ...indent_tokens(indent),
                        RIGHT_SQUIGGLE,
                        Token::new("else".into()),
                        LEFT_SQUIGGLE,
                        NEWLINE,
                        ...false_block,
                        ...indent_tokens(indent),
                        RIGHT_SQUIGGLE,
                    ])
                }
            }

            MIRStatement::LoopStatement { body, .. } => {
                let loop_body = self.lower_block(body);

                Some(spread![
                    Token::new("while".into()),
                    LEFT_PAREN,
                    Token::new("1".into()),
                    RIGHT_PAREN,
                    LEFT_SQUIGGLE,
                    NEWLINE,
                    ...loop_body,
                    ...indent_tokens(indent),
                    RIGHT_SQUIGGLE,
                ])
            }

            MIRStatement::ContinueStatement { .. } => {
                Some(spread![Token::new("continue".into()), SEMI])
            }

            MIRStatement::BreakStatement { .. } => Some(spread![Token::new("break".into()), SEMI]),
        }
    }

    fn lower_static<'a>(&mut self, val: &MIRStatic<'a>) -> Tokens<'a> {
        let decorated = self.decorate_with_type(val.name.clone(), &val.ty);
        let expr = self.lower_expression(&val.value);

        spread![Token::new("static".into()), ...decorated, Token::new("=".into()), ...expr, SEMI]
    }

    fn lower_expression<'a>(&mut self, expr: &MIRExpression<'a>) -> Tokens<'a> {
        macro_rules! lower_binary {
            ($left:expr, $op:tt, $right:expr) => {{
                let left = self.lower_wrap_expression($left, expr);
                let right = self.lower_wrap_expression($right, expr);

                spread![...left, Token::new($op.into()), ...right]
            }};
        }

        macro_rules! lower_unary {
            ($op:tt, $expr:expr) => {{
                let value = self.lower_wrap_expression($expr, expr);
                spread![Token::new($op.into()), ...value]
            }};
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
            MIRExpressionInner::Ref(inner) => lower_unary!("&", inner),
            MIRExpressionInner::Deref(inner) => lower_unary!("*", inner),

            MIRExpressionInner::Number(num) => spread![Token::new(num.to_string().into())],
            // This MUST be a single token, as we cannot insert spaces between the quotes and the string content.
            MIRExpressionInner::String(val) => spread![Token::new(
                ("\"".to_string() + &escape_string(val) + "\"").into()
            )],
            MIRExpressionInner::Bool(val) => {
                if *val {
                    spread![Token::new("true".into())]
                } else {
                    spread![Token::new("false".into())]
                }
            }
            MIRExpressionInner::Unit => spread![Token::new("void".into())],
            MIRExpressionInner::Variable(name, _) => spread![Token::new(name.clone())],
            MIRExpressionInner::FunctionCall(call) => {
                let args = call
                    .args
                    .iter()
                    .map(|v| self.lower_expression(v))
                    .intersperse(spread![Token::new(", ".into())])
                    .flatten()
                    .collect::<Tokens<'a>>();
                let src = self.lower_fn_source(&call.source);

                spread![...src, LEFT_PAREN, ...args, RIGHT_PAREN]
            }
            MIRExpressionInner::Member(base, field) => {
                let base = self.lower_wrap_expression(base, expr);
                spread![...base, Token::new(".".into()), Token::new(field.clone())]
            }
            MIRExpressionInner::Index(base, index) => {
                let base = self.lower_wrap_expression(base, expr);
                // Already wrapped by [], so no need to wrap it again.
                let index = self.lower_expression(index);
                spread![...base, Token::new("[".into()), ...index, Token::new("]".into())]
            }
            MIRExpressionInner::Array(elems) => {
                let elems = elems
                    .iter()
                    .map(|v| self.lower_expression(v))
                    .intersperse(spread![Token::new(",".into())])
                    .flatten()
                    .collect::<Tokens<'a>>();
                spread![LEFT_SQUIGGLE, ...elems, RIGHT_SQUIGGLE]
            }
        }
    }

    fn lower_fn_source<'a>(&mut self, src: &MIRFnSource<'a>) -> Tokens<'a> {
        match src {
            MIRFnSource::Direct(src, _) => spread![Token::new(src.clone())],
            MIRFnSource::Indirect(name) => {
                let lowered = self.lower_expression(name);
                spread![LEFT_PAREN, ...lowered, RIGHT_PAREN]
            }
        }
    }

    fn lower_datatype<'a>(&mut self, ty: &MIRTypeInner<'a>) -> (Tokens<'a>, Tokens<'a>) {
        match ty {
            MIRTypeInner::UnknownNumber | MIRTypeInner::NotConstructed => unreachable!(),
            MIRTypeInner::I32 => (spread![Token::new("int".into())], [].into()),
            MIRTypeInner::U32 => (
                spread![Token::new("unsigned".into()), Token::new("int".into())],
                [].into(),
            ),
            MIRTypeInner::String => (
                spread![Token::new("char".into()), Token::new("*".into())],
                [].into(),
            ),
            MIRTypeInner::Bool => (spread![Token::new("bool".into())], [].into()),
            MIRTypeInner::Unit => (spread![Token::new("void".into())], [].into()),
            MIRTypeInner::Named(name) => (spread![Token::new(name.clone())], [].into()),
            // Array is essentially just a ref in C, no reason to handle it differently.
            MIRTypeInner::Ref(box inner) | MIRTypeInner::Array(box inner) => {
                let (mut left, right) = self.lower_datatype(inner);

                // Only wrap parentheses if we haven't already.
                let needs_parens = right.first().map(|t| t != &RIGHT_PAREN).unwrap_or(false);

                if needs_parens {
                    // Need parens to protect * from postfix operators (like [])
                    left.push(LEFT_PAREN);
                    left.push(Token::new("*".into()));
                    (left, spread![RIGHT_PAREN, ...right])
                } else {
                    // The rightmost (innermost) part is what applies first.
                    // If we went with the leftmost part, we'd be injecting a reference
                    // too far down.
                    if let Some(paren_idx) = left.iter().rposition(|t| t == &LEFT_PAREN) {
                        left.insert(paren_idx + 1, Token::new("*".into()));
                    } else {
                        left.push(Token::new("*".into()));
                    }
                    (left, right)
                }
            }
            MIRTypeInner::ArrayFixed(box inner, size) => {
                let (left, right) = self.lower_datatype(inner);
                // Prepend [size] to right
                (
                    left,
                    spread![LEFT_BRACKET, Token::new(size.to_string().into()), RIGHT_BRACKET, ...right],
                )
            }
            MIRTypeInner::FunctionPtr(func_args, box ret) => {
                let (mut left, right) = self.lower_datatype(ret);

                // Build parameter list as tokens
                let mut params: Tokens<'a> = func_args
                    .args
                    .iter()
                    .map(|arg| {
                        let (left, right) = self.lower_datatype(arg);
                        spread![...left, ...right]
                    })
                    .intersperse(spread![Token::new(",".into())])
                    .flatten()
                    .collect();

                if func_args.variadic {
                    if !params.is_empty() {
                        params.push(Token::new(",".into()));
                    }
                    params.push(Token::new("...".into()));
                } else if params.is_empty() {
                    params.push(Token::new("void".into()));
                }

                // Example:
                // Ret: ["int*", "[10]"] -> "int(*(*NAME)(void))[10]"
                left.push(LEFT_PAREN);
                left.push(Token::new("*".into()));
                (
                    left,
                    spread![RIGHT_PAREN, LEFT_PAREN, ...params, RIGHT_PAREN, ...right],
                )
            }
        }
    }

    fn decorate_with_type<'a>(&mut self, name: Cow<'a, str>, ty: &MIRType<'a>) -> Tokens<'a> {
        let (prefix, postfix) = self.lower_datatype(&ty.ty);
        spread![...prefix, Token::new(name), ...postfix]
    }

    fn lower_wrap_expression<'a>(
        &mut self,
        expr: &MIRExpression<'a>,
        outer: &MIRExpression<'a>,
    ) -> Tokens<'a> {
        let lowered = self.lower_expression(expr);

        let outer_precedence = self.precedence(&outer.inner);
        let inner_precedence = self.precedence(&expr.inner);
        let needs_wrap = matches!((outer_precedence, inner_precedence), (Some(outer), Some(inner)) if inner > outer);

        if needs_wrap {
            spread![LEFT_PAREN, ...lowered, RIGHT_PAREN]
        } else {
            lowered
        }
    }

    fn precedence(&self, op: &MIRExpressionInner) -> Option<usize> {
        // https://en.cppreference.com/w/c/language/operator_precedence.html
        match op {
            MIRExpressionInner::Variable(..)
            | MIRExpressionInner::Number(_)
            | MIRExpressionInner::String(_)
            | MIRExpressionInner::Bool(_)
            | MIRExpressionInner::Unit
            | MIRExpressionInner::FunctionCall(_)
            | MIRExpressionInner::Array(_) => None,

            MIRExpressionInner::Member(..) | MIRExpressionInner::Index(..) => Some(1),

            MIRExpressionInner::Ref(..) | MIRExpressionInner::Deref(..) => Some(2),

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

    fn lower_imports<'a>(&mut self, imports: &[Cow<'a, str>]) -> Tokens<'a> {
        imports
            .iter()
            .flat_map(|import| {
                spread![
                    Token::new("#include".into()),
                    Token::new(import.clone()),
                    NEWLINE_REQUIRED,
                ]
            })
            .collect()
    }
}

/// Returns the indentation for the given level (level = number of tabs).
fn indent_tokens<'a>(indent_level: u32) -> Tokens<'a> {
    (0..indent_level).map(|_| INDENT).collect::<Tokens<'a>>()
}
