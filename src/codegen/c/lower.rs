use crate::codegen::Codegen;
use crate::codegen::LowerOptions;
use crate::codegen::c::token::{
    INDENT, LEFT_BRACKET, LEFT_PAREN, LEFT_SQUIGGLE, NEWLINE, NEWLINE_REQUIRED, RIGHT_BRACKET,
    RIGHT_PAREN, RIGHT_SQUIGGLE, SEMI, compress_with_defines, escape_char, escape_string,
};
use crate::codegen::token::{Token, TokenInfo, TokenStyle, Tokens, spread, strip_fancy_tokens};
use crate::mir::{
    MIRDeclarationKey, MIRExpression, MIRExpressionInner, MIRFnSource, MIRFunction,
    MIRFunctionType, MIRProgram, MIRStatement, MIRStatic, MIRType, MIRTypeInner, MIRVariable,
};
use crate::parser::span::Span;
use std::borrow::Cow;

pub const C: &'static dyn Codegen = &CLowerer {
    indent_level: 0,
    explicit_array: false,
};

#[derive(Default, Debug, Clone)]
pub struct CLowerer {
    /// The current indentation level.
    /// This represents the number of tabs of indentation (not the number of spaces).
    indent_level: u32,

    /// Whether to explicitly declare the next type to lower as an array.
    /// Non-recursive.
    explicit_array: bool,
}

impl Codegen for CLowerer {
    fn new(&self) -> Box<dyn Codegen> {
        Box::new(CLowerer::default())
    }

    fn lower_program(&mut self, program: &MIRProgram, options: LowerOptions) -> String {
        let mut header = spread![];
        let mut body = spread![];

        header.extend(self.lower_imports(&program.required_imports));

        for val in &program.decls {
            match val {
                MIRDeclarationKey::Static(val) => {
                    body.extend(self.lower_static(&program.statics[*val]))
                }
                MIRDeclarationKey::Function(val) => {
                    // Skip extern functions (they have no body to emit)
                    if program.functions[*val].fn_type != MIRFunctionType::Extern {
                        body.extend(self.lower_function(&program.functions[*val]))
                    }
                }
                // Constants are never exported.
                MIRDeclarationKey::Constant(_) => {}
                MIRDeclarationKey::Marker(key) => {
                    let marker = &program.markers[*key];

                    body.push(Token {
                        text: Some(marker.name.clone()),
                        style: TokenStyle::Marker,
                    });
                }
            }
        }

        if !options.fancy {
            strip_fancy_tokens(&mut header);
            strip_fancy_tokens(&mut body);

            compress_with_defines(self, &mut header, &mut body);
        }

        body.splice(0..0, header);

        // TODO: Use markers to inform merging and generate an index list.
        self.merge_tokens(&mut body, None);

        // Encode required spaces and newlines in the quine string as other printable
        // characters.
        // This allows spaces and newlines to be inserted arbitrarily into it.
        //
        // To do this, we need to find two printable characters not already used in
        // the string.
        let mut used_chars = [false; u8::MAX as usize + 1];
        for token in &body {
            if token.style != TokenStyle::Marker
                && let Some(text) = &token.text
            {
                for &byte in text.as_bytes() {
                    used_chars[byte as usize] = true;
                }
            }
        }

        // Disallow characters which could be generated programmatically,
        // such as numbers and booleans.
        // TODO: This is fragile, ideally the user should be able to configure the chars to use.
        for &byte in b"0123456789.+-truefalse \n" {
            used_chars[byte as usize] = true;
        }

        let mut encoding_chars = (0u8..=u8::MAX).filter(|&b| {
            let c = b as char;
            !c.is_control() && !used_chars[b as usize]
        });
        let space_char = encoding_chars
            .next()
            .expect("No unused printable ASCII char for quine space");
        let newline_char = encoding_chars
            .next()
            .expect("No unused printable ASCII char for quine newline");

        // We need to know the length of the quine upfront, since we have to
        // inject quine_len into it.
        // TODO: Can we avoid this creating a new element just for quine_len itself?
        let quine_len = body
            .iter()
            .filter(|token| {
                token.style != TokenStyle::Marker
                    || matches!(
                        token.text.as_deref(),
                        Some("$quine")
                            | Some("$quineLen")
                            | Some("$quineSpace")
                            | Some("$quineLine")
                    )
            })
            .count();

        // This gives us a view of the output, where
        // each marker is its own element.
        // An empty string is a reference back to this array.
        // Spaces and newlines in token texts are replaced with the encoding chars.
        let quine = body
            .iter()
            .flat_map(|token| match token.style {
                TokenStyle::Marker => {
                    match token.text.as_deref() {
                        Some("$quine") => {
                            // Empty string is interpreted as inserting the quine array
                            // at runtime.
                            Some("".to_string())
                        }
                        Some("$quineLen") => Some(quine_len.to_string()),
                        Some("$quineSpace") => Some(space_char.to_string()),
                        Some("$quineLine") => Some(newline_char.to_string()),
                        _ => None,
                    }
                }
                _ => token.text.as_ref().map(|t| {
                    t.chars()
                        .map(|c| match c {
                            ' ' => space_char as char,
                            '\n' => newline_char as char,
                            c => c,
                        })
                        .collect::<String>()
                }),
            })
            .map(|s| format!("\"{}\"", escape_string(&s)))
            .collect::<Vec<_>>();

        assert_eq!(quine.len(), quine_len);

        // Inject generated expressions.
        for token in &mut body {
            if token.style == TokenStyle::Marker {
                match token.text.as_deref() {
                    Some("$quine") => {
                        *token = Token {
                            style: TokenStyle::Required,
                            text: Some(quine.join(",").into()),
                        }
                    }
                    Some("$quineLen") => {
                        *token = Token {
                            style: TokenStyle::Required,
                            text: Some(quine_len.to_string().into()),
                        }
                    }
                    Some("$quineSpace") => {
                        *token = Token {
                            style: TokenStyle::Required,
                            text: Some(space_char.to_string().into()),
                        }
                    }
                    Some("$quineLine") => {
                        *token = Token {
                            style: TokenStyle::Required,
                            text: Some(newline_char.to_string().into()),
                        }
                    }
                    _ => {}
                }
            }
        }

        body.retain(|token| token.style != TokenStyle::Marker);

        let mut output_str = String::new();
        let mut iter = body.iter().peekable();
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
        let block = self.lower_block(&func.body, true, true);

        spread![...decorated, LEFT_PAREN, ...args, RIGHT_PAREN, LEFT_SQUIGGLE, NEWLINE, ...block, RIGHT_SQUIGGLE, NEWLINE]
    }

    fn lower_block<'a>(
        &mut self,
        block: &[MIRStatement<'a>],
        increase_indent: bool,
        is_enclosed: bool,
    ) -> Tokens<'a> {
        // Items inside a block ({ ... }) should be indented.
        let pre_indent = self.indent_level;
        if increase_indent {
            self.indent_level += 1;
        }

        let indent = indent_tokens(self.indent_level);

        let mut ret = block
            .iter()
            // Remove None values.
            .filter_map(|v| self.lower_statement(v))
            .enumerate()
            .flat_map(|(i, v)| {
                if i == 0 && !is_enclosed {
                    // If we aren't enclosed, then we can't print out
                    // the first indent, since it will already exist
                    // from the caller.
                    spread![...v, NEWLINE]
                } else {
                    spread![...&indent, ...v, NEWLINE]
                }
            })
            .collect::<Tokens<'a>>();

        if !is_enclosed && !ret.is_empty() {
            // If we're not enclosed, the caller will print out a newline
            // as well, so we need to remove ours to prevent duplicates.
            ret.pop();
        }

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
                // Array initializers require explicit array type.
                self.explicit_array = matches!(
                    value,
                    Some(MIRExpression {
                        inner: MIRExpressionInner::Array(_),
                        ..
                    },)
                );
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
                let place_tokens = self.lower_expression(place);

                // Optimize to shorthand += / ++ / etc
                // if we can (i.e., a = a + 1 -> a++).
                match &value.inner {
                    MIRExpressionInner::Add(
                        box left,
                        box MIRExpression {
                            inner: MIRExpressionInner::Number(1),
                            ..
                        },
                    ) if left == place => Some(spread![
                        ...place_tokens,
                        Token::new("++".into()),
                        SEMI,
                    ]),

                    MIRExpressionInner::Sub(
                        box left,
                        box MIRExpression {
                            inner: MIRExpressionInner::Number(1),
                            ..
                        },
                    ) if left == place => Some(spread![
                        ...place_tokens,
                        Token::new("--".into()),
                        SEMI,
                    ]),

                    MIRExpressionInner::Add(box left, box right) if left == place => {
                        let expr = self.lower_expression(right);

                        Some(spread![
                            ...place_tokens,
                            Token::new("+=".into()),
                            ...expr,
                            SEMI,
                        ])
                    }

                    MIRExpressionInner::Sub(box left, box right) if left == place => {
                        let expr = self.lower_expression(right);

                        Some(spread![
                            ...place_tokens,
                            Token::new("-=".into()),
                            ...expr,
                            SEMI,
                        ])
                    }

                    MIRExpressionInner::Mul(box left, box right) if left == place => {
                        let expr = self.lower_expression(right);

                        Some(spread![
                            ...place_tokens,
                            Token::new("*=".into()),
                            ...expr,
                            SEMI,
                        ])
                    }

                    MIRExpressionInner::Div(box left, box right) if left == place => {
                        let expr = self.lower_expression(right);

                        Some(spread![
                            ...place_tokens,
                            Token::new("/=".into()),
                            ...expr,
                            SEMI,
                        ])
                    }

                    _ => {
                        let expr = self.lower_expression(value);

                        Some(spread![
                            ...place_tokens,
                            Token::new("=".into()),
                            ...expr,
                            SEMI,
                        ])
                    }
                }
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
                let true_part = if on_true.len() == 1
                    // If the child is an if statement and we have an else, then
                    // that child must have its own else or else our else will bind to it.
                    // For example, if the else belongs to us, we don't want:
                    // if (a) if(b) ...; else ...;
                    //
                    // Otherwise, C will interpret our else as belonging to the child. However,
                    // if the child has an else:
                    // if (a) if(b) ...; else ...; else ...;
                    //
                    // Then the ambiguity no longer exists.
                    && !matches!(&on_true[0], MIRStatement::IfStatement { on_false: on_false_child, .. } if !on_false.is_empty() && on_false_child.is_empty())
                {
                    self.lower_statement(&on_true[0])?
                } else {
                    spread![
                        LEFT_SQUIGGLE,
                        NEWLINE,
                        ...self.lower_block(on_true, true, true),
                        ...indent_tokens(indent),
                        RIGHT_SQUIGGLE,
                    ]
                };

                if on_false.is_empty() {
                    Some(spread![
                        Token::new("if".into()),
                        LEFT_PAREN,
                        ...cond,
                        RIGHT_PAREN,
                        ...true_part,
                    ])
                } else {
                    let else_part = if on_false.len() == 1 {
                        self.lower_statement(&on_false[0])?
                    } else {
                        spread![
                            LEFT_SQUIGGLE,
                            NEWLINE,
                            ...self.lower_block(on_false, true, true),
                            ...indent_tokens(indent),
                            RIGHT_SQUIGGLE,
                        ]
                    };

                    Some(spread![
                        Token::new("if".into()),
                        LEFT_PAREN,
                        ...cond,
                        RIGHT_PAREN,
                        ...true_part,
                        NEWLINE,
                        ...indent_tokens(indent),
                        Token::new("else".into()),
                        ...else_part,
                    ])
                }
            }

            MIRStatement::LoopStatement {
                condition,
                body,
                iterate,
                ..
            } => {
                let loop_body = if body.len() == 1 {
                    self.lower_statement(&body[0])?
                } else {
                    spread![
                        LEFT_SQUIGGLE,
                        NEWLINE,
                        ...self.lower_block(body, true, true),
                        ...indent_tokens(indent),
                        RIGHT_SQUIGGLE,
                    ]
                };

                let cond = if let Some(condition) = condition {
                    self.lower_expression(condition)
                } else {
                    spread![]
                };

                let iterate = if !iterate.is_empty() {
                    if iterate.len() != 1 {
                        panic!("Loop iterate body can only be a single statement!");
                    }

                    let mut tokens = self.lower_statement(&iterate[0])?;
                    // The iterate part can't have a semicolon at the end.
                    if tokens.last() == Some(&SEMI) {
                        tokens.pop();
                    }

                    tokens
                } else {
                    spread![]
                };

                Some(spread![
                    Token::new("for".into()),
                    LEFT_PAREN,
                    // For loops with initializers are handled by ScopeStatement.
                    SEMI,
                    ...cond,
                    SEMI,
                    ...iterate,
                    RIGHT_PAREN,
                    ...loop_body,
                ])
            }

            MIRStatement::ScopeStatement { body, .. } => {
                // Optimization: if the scope contains just a statement and Loop (for-loop),
                // then we can merge them together.
                if let Some(initializer) = body.first()
                    && let Some(loop_ @ MIRStatement::LoopStatement { .. }) = body.get(1)
                {
                    let mut initializer = self.lower_statement(initializer)?;
                    let loop_ = self.lower_statement(loop_)?;

                    // Loop will always be ["for", "(", ";"].
                    // We want to inject between the parentheses and ";"
                    // for the initializer.
                    //
                    // It might have a trailing semicolon, though.
                    if initializer.last() == Some(&SEMI) {
                        initializer.pop();
                    }

                    if loop_[0] != Token::new("for".into())
                        || loop_[1] != LEFT_PAREN
                        || loop_[2] != SEMI
                    {
                        panic!("Loop statement is not in the correct format!");
                    }

                    let for_ = loop_[0].clone();
                    let paren = loop_[1].clone();
                    let remaining_loop = &loop_[2..];

                    Some(spread![
                        for_,
                        paren,
                        ...initializer,
                        ...remaining_loop,
                    ])
                } else {
                    // No extra ident here, since the scope effectively just
                    // exists for drop analysis.
                    // We also don't want to indent the first line, since this statement
                    // will have already been indented by the caller.
                    Some(self.lower_block(body, false, false))
                }
            }

            MIRStatement::ContinueStatement { .. } => {
                Some(spread![Token::new("continue".into()), SEMI])
            }

            MIRStatement::BreakStatement { .. } => Some(spread![Token::new("break".into()), SEMI]),

            MIRStatement::MarkerStatement { name, .. } => Some(spread![Token {
                text: Some(name.clone()),
                style: TokenStyle::Marker,
            }]),
        }
    }

    fn lower_static<'a>(&mut self, val: &MIRStatic<'a>) -> Tokens<'a> {
        // Array initializers require explicit array type.
        self.explicit_array = matches!(
            val.value,
            MIRExpression {
                inner: MIRExpressionInner::Array(_) | MIRExpressionInner::Quine,
                ..
            },
        );
        let decorated = self.decorate_with_type(val.name.clone(), &val.ty);
        let expr = self.lower_expression(&val.value);

        // "static" only affects mutliple files, so we can exclude it.
        spread![...decorated, Token::new("=".into()), ...expr, SEMI]
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

        // This is a hack - there isn't a unary negation type, but
        // deref is also a unary op and has the same precedence level.
        fn unary_op_outer<'a>() -> MIRExpression<'a> {
            MIRExpression {
                inner: MIRExpressionInner::Deref(Box::new(MIRExpression {
                    inner: MIRExpressionInner::Unit,
                    ty: None,
                    span: Span::empty(),
                })),
                ty: None,
                span: Span::empty(),
            }
        }

        match &expr.inner {
            MIRExpressionInner::Add(left, right) => lower_binary!(left, "+", right),
            MIRExpressionInner::Sub(left, right) => lower_binary!(left, "-", right),
            MIRExpressionInner::Mul(left, right) => {
                // Optimize -1 multiplication to negation.
                match (left, right) {
                    (
                        box MIRExpression {
                            inner: MIRExpressionInner::Number(-1),
                            ..
                        },
                        _,
                    ) => {
                        let right = self.lower_wrap_expression(right, &unary_op_outer());
                        spread![Token::new("-".into()), ...right]
                    }

                    (
                        _,
                        box MIRExpression {
                            inner: MIRExpressionInner::Number(-1),
                            ..
                        },
                    ) => {
                        let left = self.lower_wrap_expression(left, &unary_op_outer());
                        spread![Token::new("-".into()), ...left]
                    }

                    _ => {
                        let left = self.lower_wrap_expression(left, expr);
                        let right = self.lower_wrap_expression(right, expr);

                        spread![...left, Token::new("*".into()), ...right]
                    }
                }
            }
            MIRExpressionInner::Div(left, right) => lower_binary!(left, "/", right),
            MIRExpressionInner::NotEqual(left, right) => {
                // Comparison against zero values like a != 0 can always be simplified to a (truthy coercion).
                match (&left.inner, &right.inner) {
                    (
                        _,
                        MIRExpressionInner::Number(0)
                        | MIRExpressionInner::Bool(false)
                        | MIRExpressionInner::Char('\0'),
                    ) => self.lower_wrap_expression(left, expr),
                    (
                        MIRExpressionInner::Number(0)
                        | MIRExpressionInner::Bool(false)
                        | MIRExpressionInner::Char('\0'),
                        _,
                    ) => self.lower_wrap_expression(right, expr),
                    _ => lower_binary!(left, "!=", right),
                }
            }
            MIRExpressionInner::Equal(left, right) => {
                // Comparison against zero values like a == 0 can always be simplified to !a (truthy coercion).
                match (&left.inner, &right.inner) {
                    (
                        _,
                        MIRExpressionInner::Number(0)
                        | MIRExpressionInner::Bool(false)
                        | MIRExpressionInner::Char('\0'),
                    ) => {
                        let left = self.lower_wrap_expression(left, &unary_op_outer());
                        spread![Token::new("!".into()), ...left]
                    }
                    (
                        MIRExpressionInner::Number(0)
                        | MIRExpressionInner::Bool(false)
                        | MIRExpressionInner::Char('\0'),
                        _,
                    ) => {
                        let right = self.lower_wrap_expression(right, &unary_op_outer());
                        spread![Token::new("!".into()), ...right]
                    }
                    _ => lower_binary!(left, "==", right),
                }
            }
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
                    spread![Token::new("1".into())]
                } else {
                    spread![Token::new("0".into())]
                }
            }
            MIRExpressionInner::Unit => spread![Token::new("void".into())],
            MIRExpressionInner::Char(c) => {
                // 'c' is 3 chars, so it's always more or as efficient to use
                // numbers if it fits.
                let c = *c as u32;
                if c < 999 {
                    spread![Token::new(c.to_string().into())]
                } else {
                    spread![Token::new(
                        ("'".to_string() + &escape_char(&c.to_string()) + "'").into()
                    )]
                }
            }
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
                // Accessing index 0 is the same as a dereference.
                if index.inner == MIRExpressionInner::Number(0) {
                    let base = self.lower_wrap_expression(base, &unary_op_outer());
                    spread![Token::new("*".into()), ...base]
                } else {
                    let base = self.lower_wrap_expression(base, expr);
                    // Already wrapped by [], so no need to wrap it again.
                    let index = self.lower_expression(index);
                    spread![...base, Token::new("[".into()), ...index, Token::new("]".into())]
                }
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
            MIRExpressionInner::Quine => {
                spread![
                    LEFT_SQUIGGLE,
                    Token {
                        text: Some("$quine".into()),
                        style: TokenStyle::Marker,
                    },
                    RIGHT_SQUIGGLE
                ]
            }
            MIRExpressionInner::QuineLen => {
                spread![Token {
                    text: Some("$quineLen".into()),
                    style: TokenStyle::Marker,
                },]
            }
            MIRExpressionInner::QuineSpace => {
                spread![Token {
                    text: Some("$quineSpace".into()),
                    style: TokenStyle::Marker,
                }]
            }
            MIRExpressionInner::QuineLine => {
                spread![Token {
                    text: Some("$quineLine".into()),
                    style: TokenStyle::Marker,
                }]
            }
            MIRExpressionInner::Binding(left, inner, right) => {
                let inner = self.lower_expression(inner);
                spread![
                    Token {
                        text: Some(left.name.clone()),
                        style: TokenStyle::Marker,
                    },
                    ...inner,
                    Token {
                        text: Some(right.name.clone()),
                        style: TokenStyle::Marker,
                    },
                ]
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
        // Used to prevent converting [] to *.
        let explicit_array = self.explicit_array;
        self.explicit_array = false;

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
            MIRTypeInner::Bool => (spread![Token::new("int".into())], [].into()),
            MIRTypeInner::Unit => (spread![Token::new("void".into())], [].into()),
            MIRTypeInner::Char => (spread![Token::new("char".into())], [].into()),
            MIRTypeInner::Named(name) => (spread![Token::new(name.clone())], [].into()),
            // Array is essentially just a ref in C, no reason to handle it differently.
            // However, the caller may sometimes tell us to not do this.
            MIRTypeInner::Ref(box inner) | MIRTypeInner::Array(box inner)
                if !matches!(inner, MIRTypeInner::Array(..)) || !explicit_array =>
            {
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
            MIRTypeInner::Ref(_) => unreachable!("Should have been handled above!"),
            MIRTypeInner::Array(box inner) => {
                let (left, right) = self.lower_datatype(inner);
                // Prepend [] to right
                (left, spread![LEFT_BRACKET, RIGHT_BRACKET, ...right])
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
            | MIRExpressionInner::Char(_)
            | MIRExpressionInner::FunctionCall(_)
            | MIRExpressionInner::Array(_)
            | MIRExpressionInner::Quine
            | MIRExpressionInner::QuineLen
            | MIRExpressionInner::QuineSpace
            | MIRExpressionInner::QuineLine => None,

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

            MIRExpressionInner::Binding(_, _, _) => todo!(),
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
