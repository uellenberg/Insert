use crate::mir::{
    MIRExpression, MIRExpressionInner, MIRFnSource, MIRFunction, MIRProgram, MIRStatement,
    MIRStatic, MIRType, MIRTypeInner,
};
use std::borrow::Cow;

/// Converts a program from MIR to C.
pub fn mir_to_c(program: &MIRProgram) -> String {
    let mut output = vec![];

    for val in program.statics.values() {
        output.push(lower_static(val));
    }

    for val in program.functions.values() {
        output.push(lower_function(val));
    }

    output.into_iter().intersperse("\n".into()).collect()
}

/// Converts a function from MIR to C.
fn lower_function(func: &MIRFunction) -> String {
    format!(
        "{}({}) {{\n{}\n}}",
        decorate_with_type(&func.name, &func.ret_ty),
        func.args
            .iter()
            .map(|arg| decorate_with_type(&arg.name, &arg.ty))
            .collect::<Vec<_>>()
            .join(", "),
        lower_block(&func.body)
    )
}

/// Converts a block of statements from MIR to C.
fn lower_block(block: &Vec<MIRStatement>) -> String {
    block
        .iter()
        .map(lower_statement)
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Converts a statement from MIR to C.
/// Returns None if the statement is not valid C (i.e., it should be ignored).
fn lower_statement(stmt: &MIRStatement) -> Option<String> {
    match stmt {
        // Just for analysis, no real codegen.
        MIRStatement::CreateVariable { arg: true, .. } | MIRStatement::DropVariable(..) => None,

        MIRStatement::CreateVariable { var, value, .. } => {
            if let Some(value) = value {
                format!(
                    "{} = {};",
                    decorate_with_type(&var.name, &var.ty),
                    lower_expression(value)
                )
                .into()
            } else {
                format!("{};", decorate_with_type(&var.name, &var.ty)).into()
            }
        }

        MIRStatement::SetVariable { name, value, .. } => {
            format!("{} = {};", name, lower_expression(value)).into()
        }

        MIRStatement::FunctionCall(call) => {
            let args = call
                .args
                .iter()
                .map(lower_expression)
                .collect::<Vec<_>>()
                .join(",");
            format!("{}({})", lower_fn_source(&call.source), args).into()
        }

        MIRStatement::Return { expr, .. } => {
            if let Some(expr) = expr {
                format!("return {};", lower_expression(expr)).into()
            } else {
                "return;".to_string().into()
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
            let cond = lower_expression(&condition);
            let on_true = lower_block(&on_true);
            if on_false.is_empty() {
                format!("if ({}) {{\n{}\n}}", cond, on_true).into()
            } else {
                let on_false = lower_block(&on_false);
                format!(
                    "if ({}) {{\n{}\n}} else {{\n{}\n}}",
                    cond, on_true, on_false
                )
                .into()
            }
        }

        MIRStatement::LoopStatement { body, .. } => {
            format!("while (1) {{\n{}\n}}", lower_block(&body)).into()
        }

        MIRStatement::ContinueStatement { .. } => "continue;".to_string().into(),

        MIRStatement::BreakStatement { .. } => "break;".to_string().into(),
    }
}

/// Converts a static variable from MIR to C.
fn lower_static(val: &MIRStatic) -> String {
    format!(
        "static {} = {};",
        decorate_with_type(&val.name, &val.ty),
        lower_expression(&val.value)
    )
}

/// Converts an expression from MIR to C.
fn lower_expression<'a>(expr: &'a MIRExpression) -> Cow<'a, str> {
    match &expr.inner {
        MIRExpressionInner::Add(left, right) => format!(
            "{}+{}",
            lower_wrap_expression(left, expr),
            lower_wrap_expression(right, expr)
        )
        .into(),
        MIRExpressionInner::Sub(left, right) => format!(
            "{}-{}",
            lower_wrap_expression(left, expr),
            lower_wrap_expression(right, expr)
        )
        .into(),
        MIRExpressionInner::Mul(left, right) => format!(
            "{}*{}",
            lower_wrap_expression(left, expr),
            lower_wrap_expression(right, expr)
        )
        .into(),
        MIRExpressionInner::Div(left, right) => format!(
            "{}/{}",
            lower_wrap_expression(left, expr),
            lower_wrap_expression(right, expr)
        )
        .into(),
        MIRExpressionInner::Equal(left, right) => format!(
            "{}=={}",
            lower_wrap_expression(left, expr),
            lower_wrap_expression(right, expr)
        )
        .into(),
        MIRExpressionInner::NotEqual(left, right) => format!(
            "{}!={}",
            lower_wrap_expression(left, expr),
            lower_wrap_expression(right, expr)
        )
        .into(),
        MIRExpressionInner::Less(left, right) => format!(
            "{}<{}",
            lower_wrap_expression(left, expr),
            lower_wrap_expression(right, expr)
        )
        .into(),
        MIRExpressionInner::Greater(left, right) => format!(
            "{}>{}",
            lower_wrap_expression(left, expr),
            lower_wrap_expression(right, expr)
        )
        .into(),
        MIRExpressionInner::LessEq(left, right) => format!(
            "{}<={}",
            lower_wrap_expression(left, expr),
            lower_wrap_expression(right, expr)
        )
        .into(),
        MIRExpressionInner::GreaterEq(left, right) => format!(
            "{}>={}",
            lower_wrap_expression(left, expr),
            lower_wrap_expression(right, expr)
        )
        .into(),
        MIRExpressionInner::BoolAnd(left, right) => format!(
            "{}&&{}",
            lower_wrap_expression(left, expr),
            lower_wrap_expression(right, expr)
        )
        .into(),
        MIRExpressionInner::BoolOr(left, right) => format!(
            "{}||{}",
            lower_wrap_expression(left, expr),
            lower_wrap_expression(right, expr)
        )
        .into(),
        MIRExpressionInner::Number(num) => Cow::Owned(num.to_string()),
        MIRExpressionInner::Bool(val) => {
            if *val {
                "true".into()
            } else {
                "false".into()
            }
        }
        MIRExpressionInner::Variable(name) => name.to_string().into(),
        MIRExpressionInner::FunctionCall(call) => {
            let args = call
                .args
                .iter()
                .map(lower_expression)
                .collect::<Vec<_>>()
                .join(",");
            format!("{}({})", lower_fn_source(&call.source), args).into()
        }
    }
}

/// Lowers a function source into a callable string (i.e., () can be added after it).
fn lower_fn_source<'a>(src: &'a MIRFnSource) -> Cow<'a, str> {
    match src {
        MIRFnSource::Direct(src, _) => src.clone(),
        MIRFnSource::Indirect(name) => format!("({})", lower_expression(name)).into(),
    }
}

/// Converts a datatype from MIR to C.
/// Returns the type as (prefix, postfix), where
/// a variable can be constructed as:
/// {PREFIX} name{POSTFIX} (e.g., char* strings[]).
fn lower_datatype<'a>(ty: &MIRType<'a>) -> (Cow<'a, str>, Cow<'a, str>) {
    match &ty.ty {
        MIRTypeInner::U32 => ("unsigned int".into(), "".into()),
        MIRTypeInner::Bool => ("bool".into(), "".into()),
        MIRTypeInner::Unit => ("void".into(), "".into()),
        MIRTypeInner::FunctionPtr(args, ret) => todo!(),
        MIRTypeInner::Named(name) => (name.clone(), "".into()),
    }
}

/// Adds a datatype to a variable/function name.
fn decorate_with_type(name: &str, ty: &MIRType) -> String {
    let (prefix, postfix) = lower_datatype(ty);
    format!("{} {}{}", prefix, name, postfix)
}

/// Determines if the inner expression needs to be wrapped in parentheses
/// when using in the outer expression.
fn needs_wrap(inner: &MIRExpression, outer: &MIRExpression) -> bool {
    // TODO: Implement.

    true
}

/// Lowers a child expression and correctly wraps it in parentheses
/// if needed.
fn lower_wrap_expression<'a>(expr: &'a MIRExpression, outer: &'a MIRExpression) -> Cow<'a, str> {
    let lowered = lower_expression(expr);

    if needs_wrap(expr, outer) {
        format!("({})", lowered).into()
    } else {
        lowered.into()
    }
}
