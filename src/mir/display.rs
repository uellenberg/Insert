use crate::mir::{
    MIRConstant, MIRExpression, MIRExpressionInner, MIRFnCall, MIRFnSource, MIRFunction,
    MIRFunctionType, MIRProgram, MIRStatement, MIRStatic, MIRType, MIRTypeInner,
};
use std::borrow::Cow;
use std::fmt::{Display, Formatter};

impl<'a> Display for MIRProgram<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "// Constants: \n\n")?;

        for (_name, constant) in &self.constants {
            constant.fmt(f)?;
            writeln!(f)?;
        }

        if !self.constants.is_empty() {
            writeln!(f)?;
        }

        write!(f, "// Statics: \n\n")?;

        for (_name, static_data) in &self.statics {
            static_data.fmt(f)?;
            writeln!(f)?;
        }

        if !self.statics.is_empty() {
            writeln!(f)?;
        }

        write!(f, "// Functions: \n\n")?;

        for function in self.functions.values() {
            function.fmt(f)?;
            writeln!(f)?;
        }

        Ok(())
    }
}

impl<'a> Display for MIRConstant<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "const {}: {} = ", &self.name, &self.ty)?;

        // Preserve formatting alternate mode.
        self.value.fmt(f)?;

        write!(f, ";")?;

        Ok(())
    }
}

impl<'a> Display for MIRStatic<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "static {}: {} = ", &self.name, &self.ty)?;

        // Preserve formatting alternate mode.
        self.value.fmt(f)?;

        write!(f, ";")?;

        Ok(())
    }
}

impl<'a> Display for MIRFunction<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.fn_type {
            MIRFunctionType::Export => {}
            MIRFunctionType::Inline => write!(f, "inline ")?,
            MIRFunctionType::Helper => write!(f, "helper ")?,
        }

        write!(f, "function {}(", &self.name)?;
        for (i, arg) in self.args.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}: {}", &arg.name, &arg.ty)?;
        }
        writeln!(f, ") : {} {{", &self.ret_ty)?;

        for stmt in &self.body {
            write_indented(f, stmt, "    ")?;
            writeln!(f)?;
        }

        write!(f, "}}")
    }
}

impl<'a> Display for MIRStatement<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            MIRStatement::CreateVariable {
                var,
                value,
                span,
                arg: _arg,
            } => {
                if let Some(value) = value {
                    write!(f, "let {}: {} = {};", &var.name, &var.ty, value)?;
                } else {
                    write!(f, "let {}: {};", &var.name, &var.ty)?;
                }

                if f.alternate() {
                    writeln!(f, " /* {span} */")?;
                }
            }
            MIRStatement::DropVariable(name, span) => {
                write!(f, "drop {};", name)?;

                if f.alternate() {
                    writeln!(f, " /* {span} */")?;
                }
            }
            MIRStatement::SetVariable {
                place, value, span, ..
            } => {
                write!(f, "{} = {};", place, value)?;

                if f.alternate() {
                    writeln!(f, " /* {span} */")?;
                }
            }
            MIRStatement::FunctionCall(fn_call) => {
                fn_call.fmt(f)?;
            }
            MIRStatement::Return { expr, span } => {
                write!(f, "return")?;
                if let Some(expr) = expr {
                    write!(f, " {}", expr)?;
                }
                write!(f, ";")?;

                if f.alternate() {
                    writeln!(f, " /* {span} */")?;
                }
            }
            MIRStatement::Label { name, span, .. } => {
                write!(f, "label {}:", name)?;

                if f.alternate() {
                    writeln!(f, " /* {span} */")?;
                }
            }
            MIRStatement::Goto { name, span, .. } => {
                write!(f, "goto {};", name)?;

                if f.alternate() {
                    writeln!(f, " /* {span} */")?;
                }
            }
            MIRStatement::GotoNotEqual {
                name,
                condition,
                span,
                ..
            } => {
                write!(f, "goto_ne({}) {};", condition, name)?;

                if f.alternate() {
                    writeln!(f, " /* {span} */")?;
                }
            }
            MIRStatement::IfStatement {
                condition,
                on_true,
                on_false,
                span,
                ..
            } => {
                writeln!(f, "if {} {{", condition)?;
                for stmt in on_true {
                    write_indented(f, stmt, "    ")?;
                    writeln!(f)?;
                }
                write!(f, "}}")?;

                if !on_false.is_empty() {
                    writeln!(f, " else {{")?;
                    for stmt in on_false {
                        write_indented(f, stmt, "    ")?;
                        writeln!(f)?;
                    }
                    write!(f, "}}")?;
                }

                if f.alternate() {
                    writeln!(f, " /* {span} */")?;
                }
            }
            MIRStatement::LoopStatement { body, span, .. } => {
                writeln!(f, "loop {{")?;

                for stmt in body {
                    write_indented(f, stmt, "    ")?;
                    writeln!(f)?;
                }

                write!(f, "}}")?;

                if f.alternate() {
                    writeln!(f, " /* {span} */")?;
                }
            }
            MIRStatement::ContinueStatement { span, .. } => {
                write!(f, "continue;")?;

                if f.alternate() {
                    writeln!(f, " /* {span} */")?;
                }
            }
            MIRStatement::BreakStatement { span, .. } => {
                write!(f, "break;")?;

                if f.alternate() {
                    writeln!(f, " /* {span} */")?;
                }
            }
        }

        Ok(())
    }
}

impl<'a> Display for MIRExpression<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.inner)
    }
}

impl<'a> Display for MIRExpressionInner<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // Use parentheses for binary operations to maintain precedence
        match self {
            MIRExpressionInner::Add(lhs, rhs) => write!(f, "({} + {})", lhs, rhs),
            MIRExpressionInner::Sub(lhs, rhs) => write!(f, "({} - {})", lhs, rhs),
            MIRExpressionInner::Mul(lhs, rhs) => write!(f, "({} * {})", lhs, rhs),
            MIRExpressionInner::Div(lhs, rhs) => write!(f, "({} / {})", lhs, rhs),
            MIRExpressionInner::Equal(lhs, rhs) => write!(f, "({} == {})", lhs, rhs),
            MIRExpressionInner::NotEqual(lhs, rhs) => write!(f, "({} != {})", lhs, rhs),
            MIRExpressionInner::Less(lhs, rhs) => write!(f, "({} < {})", lhs, rhs),
            MIRExpressionInner::Greater(lhs, rhs) => write!(f, "({} > {})", lhs, rhs),
            MIRExpressionInner::LessEq(lhs, rhs) => write!(f, "({} <= {})", lhs, rhs),
            MIRExpressionInner::GreaterEq(lhs, rhs) => write!(f, "({} >= {})", lhs, rhs),
            MIRExpressionInner::BoolAnd(lhs, rhs) => write!(f, "({} && {})", lhs, rhs),
            MIRExpressionInner::BoolOr(lhs, rhs) => write!(f, "({} || {})", lhs, rhs),
            MIRExpressionInner::Number(val) => write!(f, "{}", val),
            // TODO: Is escaping here worth it?
            MIRExpressionInner::String(val) => write!(f, "\"{}\"", val),
            MIRExpressionInner::Bool(val) => write!(f, "{}", val),
            MIRExpressionInner::Unit => write!(f, "()"),
            MIRExpressionInner::Variable(name) => write!(f, "{}", name),
            MIRExpressionInner::FunctionCall(fn_call) => (**fn_call).fmt(f),
            MIRExpressionInner::Ref(inner) => write!(f, "(&{})", inner),
            MIRExpressionInner::Deref(inner) => write!(f, "(*{})", inner),
            MIRExpressionInner::Member(base, field) => write!(f, "({}.{})", base, field),
            MIRExpressionInner::Index(base, index) => write!(f, "({}[{}])", base, index),
        }
    }
}

impl<'a> Display for MIRType<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", &self.ty)
    }
}

impl<'a> Display for MIRTypeInner<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let type_name: Cow<'_, str> = self.clone().into();
        write!(f, "{}", type_name)
    }
}

impl<'a> Display for MIRFnCall<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}(", &self.source)?;
        for (i, arg) in self.args.iter().enumerate() {
            if i != 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", arg)?;
        }
        write!(f, ")")?;

        if f.alternate() {
            writeln!(f, " /* {} */", &self.span)?;
        }

        Ok(())
    }
}

impl<'a> Display for MIRFnSource<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            MIRFnSource::Direct(name, _span) => write!(f, "{name}"),
            MIRFnSource::Indirect(expr) => write!(f, "{}", expr),
        }
    }
}

/// Adds indentation to an item that implements Display,
/// then writes it.
fn write_indented(f: &mut Formatter<'_>, item: &impl Display, indent: &str) -> std::fmt::Result {
    let mut first = true;

    let fmt = if f.alternate() {
        format!("{:#}", item)
    } else {
        format!("{}", item)
    };

    for line in fmt.lines() {
        if !first {
            writeln!(f)?;
        } else {
            first = false;
        }

        write!(f, "{}{}", indent, line)?;
    }
    Ok(())
}
