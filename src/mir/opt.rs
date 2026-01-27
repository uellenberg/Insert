use crate::mir::scope::StatementExplorer;
use crate::mir::{MIRContext, MIRExpression, MIRExpressionInner, MIRStatement};

/// Removes trivial (e.g., if(true)) if statements.
pub fn remove_trivial_ifs(ctx: &mut MIRContext) -> bool {
    for function in ctx.program.functions.values_mut() {
        if !<StatementExplorer>::rewrite_block(
            &mut function.body,
            &mut |statement, scope, values| {
                match statement {
                    MIRStatement::IfStatement {
                        condition:
                            MIRExpression {
                                inner: MIRExpressionInner::Bool(cond),
                                ..
                            },
                        on_true,
                        on_false,
                        ..
                    } => {
                        if cond {
                            values.extend(on_true);
                        } else {
                            values.extend(on_false);
                        }
                    }
                    _ => {
                        values.push(statement);
                    }
                }

                true
            },
            &mut |_, _| true,
            &|_, _, _| true,
        ) {
            return false;
        }
    }

    true
}

/// Removes statements after a return, continue, or break.
pub fn remove_dead_code(ctx: &mut MIRContext) -> bool {
    #[derive(Default, Debug, Clone)]
    struct DeadCodeData {
        is_dead: bool,
    }

    for function in ctx.program.functions.values_mut() {
        if !<StatementExplorer<(), DeadCodeData>>::rewrite_block(
            &mut function.body,
            &mut |statement, scope, values| {
                // One of the previous statements was a return/continue/break.
                if scope.scope_data.is_dead {
                    return true;
                }

                match &statement {
                    MIRStatement::Return { .. }
                    | MIRStatement::ContinueStatement { .. }
                    | MIRStatement::BreakStatement { .. } => {
                        // Any children have already run, but these statements don't have children,
                        // and even if they did, we would want to set is_dead after them to keep them
                        // included, so doing this in for_each is safe.
                        scope.scope_data.is_dead = true;
                    }
                    _ => {}
                }

                values.push(statement);

                true
            },
            &mut |_, _| true,
            &|_, _, _| true,
        ) {
            return false;
        }
    }

    true
}
