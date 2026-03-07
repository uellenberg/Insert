use crate::mir::scope::StatementExplorer;
use crate::mir::{MIRContext, MIRStatement};
use crate::parser::span::eprintln_span;
use std::borrow::Cow;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, Default, Clone)]
struct LoopData<'a> {
    /// 0: Points to the start of the loop (condition check for while loops).
    ///
    /// 1: Points to the first statement after a loop ends.
    ///
    /// 2: Continue target. For loops with iterate blocks, points to the iterate
    ///    block label. For while/infinite loops, same as 0.
    loop_labels: Option<(Cow<'a, str>, Cow<'a, str>, Cow<'a, str>)>,
}

/// Converts every loop statement into
/// labels and gotos.
pub fn flatten_loops<'a>(ctx: &mut MIRContext<'a>) -> bool {
    for function in ctx.program.functions.values_mut() {
        let label_idx = AtomicU32::new(0);

        if !<StatementExplorer<LoopData<'a>>>::rewrite_block(
            &mut function.body,
            &mut |statement, scope, block| {
                match statement {
                    MIRStatement::LoopStatement {
                        condition,
                        mut body,
                        mut iterate,
                        span,
                    } => {
                        let labels = scope
                            .parent_data
                            .loop_labels
                            .as_ref()
                            .expect("Loop data does not exist!");
                        let head = labels.0.clone();
                        let tail = labels.1.clone();
                        let continue_ = labels.2.clone();

                        // While loops and for-loops need a condition.
                        if let Some(condition) = condition {
                            block.push(MIRStatement::GotoNotEqual {
                                name: tail.clone(),
                                condition,
                                index: None,
                                span: span.clone(),
                            });
                        }

                        block.append(&mut body);

                        // For loops with iterate blocks (the last part of a for-loop) have
                        // a continue section, which gets ran when the loop body ends or
                        // continue is run.
                        block.push(MIRStatement::Label {
                            name: continue_,
                            span: span.clone(),
                        });
                        block.append(&mut iterate);

                        block.push(MIRStatement::Goto {
                            name: head,
                            index: None,
                            span: span.clone(),
                        });

                        block.push(MIRStatement::Label {
                            name: tail,
                            span: span.clone(),
                        });

                        true
                    }

                    MIRStatement::ScopeStatement { mut body, .. } => {
                        // Scopes are only used for variable dropping (which doesn't matter
                        // for the interpreter), so we can inline directly.
                        block.append(&mut body);
                        true
                    }

                    MIRStatement::ContinueStatement { span } => {
                        let Some(loop_data) = scope.parent_data.loop_labels.as_ref() else {
                            eprintln_span!(Some(span), "Continue only works inside of loops!");
                            return false;
                        };

                        // 2 - continue target (update section for for-loops, head for others).
                        block.push(MIRStatement::Goto {
                            name: loop_data.2.clone(),
                            index: None,
                            span,
                        });

                        true
                    }

                    MIRStatement::BreakStatement { span } => {
                        let Some(loop_data) = scope.parent_data.loop_labels.as_ref() else {
                            eprintln_span!(Some(span), "Break only works inside of loops!");
                            return false;
                        };

                        // 1 - loop tail.
                        block.push(MIRStatement::Goto {
                            name: loop_data.1.clone(),
                            index: None,
                            span,
                        });

                        true
                    }

                    // We're only dealing with loop
                    // information.
                    _ => {
                        block.push(statement);
                        true
                    }
                }
            },
            &mut |_, _| true,
            &|statement, scope, block| {
                if let MIRStatement::LoopStatement { span, .. } = statement {
                    let loop_id = label_idx.fetch_add(1, Ordering::Relaxed);
                    let head = format!("$loop_{}_head", loop_id);
                    let tail = format!("$loop_{}_tail", loop_id);
                    let continue_ = format!("$loop_{}_continue", loop_id);

                    scope.parent_data.loop_labels = Some((
                        Cow::Owned(head.clone()),
                        Cow::Owned(tail),
                        Cow::Owned(continue_),
                    ));

                    // This runs before the child statements,
                    // so insert a label so that we can go back to them.
                    block.push(MIRStatement::Label {
                        name: Cow::Owned(head),
                        span: span.clone(),
                    });
                }

                true
            },
        ) {
            return false;
        }
    }

    true
}
