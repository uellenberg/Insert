use crate::mir::scope::StatementExplorer;
use crate::mir::{MIRContext, MIRStatement};
use std::borrow::Cow;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, Default, Clone)]
struct LoopData<'a> {
    /// 0: Points to the start of the loop.
    ///
    /// 1: Points to the first statement
    ///    after a loop ends.
    loop_labels: Option<(Cow<'a, str>, Cow<'a, str>)>,
}

/// Converts every loop statement into
/// labels and gotos.
pub fn flatten_loops<'a>(ctx: &mut MIRContext<'a>) {
    for function in ctx.program.functions.values_mut() {
        let label_idx = AtomicU32::new(0);

        if !<StatementExplorer<LoopData<'a>>>::rewrite_block(
            &mut function.body,
            &mut |statement, scope, block| {
                match statement {
                    MIRStatement::LoopStatement { mut body, span, .. } => {
                        // We don't need to handle LoopStatement here
                        // to unset the parent data, as modifying parent
                        // data only affects the children.

                        block.append(&mut body);

                        block.push(MIRStatement::Goto {
                            // This data is generated from pre_run.
                            // If we don't have it, it's a bug.
                            // 0 - loop head.
                            name: scope
                                .parent_data
                                .loop_labels
                                .as_ref()
                                .expect("Loop data does not exist!")
                                .0
                                .clone(),
                            index: None,
                            span: span.clone(),
                        });

                        block.push(MIRStatement::Label {
                            // This data is generated from pre_run.
                            // If we don't have it, it's a bug.
                            // 1 - loop tail.
                            name: scope
                                .parent_data
                                .loop_labels
                                .as_ref()
                                .expect("Loop data does not exist!")
                                .1
                                .clone(),
                            span: span.clone(),
                        });

                        true
                    }

                    MIRStatement::ContinueStatement { span } => {
                        let Some(loop_data) = scope.parent_data.loop_labels.as_ref() else {
                            eprintln!("Continue only works inside of loops!");
                            return false;
                        };

                        // 0 - loop head.
                        block.push(MIRStatement::Goto {
                            name: loop_data.0.clone(),
                            index: None,
                            span,
                        });

                        true
                    }

                    MIRStatement::BreakStatement { span } => {
                        let Some(loop_data) = scope.parent_data.loop_labels.as_ref() else {
                            eprintln!("Break only works inside of loops!");
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
            &mut |statement, scope, block| {
                match statement {
                    MIRStatement::LoopStatement { span, .. } => {
                        let loop_id = label_idx.fetch_add(1, Ordering::Relaxed);
                        let loop_label_head = format!("$loop_{}_head", loop_id);
                        let loop_label_tail = format!("$loop_{}_tail", loop_id);

                        scope.parent_data.loop_labels = Some((
                            Cow::Owned(loop_label_head.clone()),
                            Cow::Owned(loop_label_tail),
                        ));

                        // This runs before the child statements,
                        // so insert a label so that we can go back to them.
                        block.push(MIRStatement::Label {
                            name: Cow::Owned(loop_label_head),
                            span: span.clone(),
                        })
                    }

                    // We only care about loops here.
                    _ => {}
                }

                true
            },
        ) {
            panic!("flatten_loops returned false!");
        }
    }
}
