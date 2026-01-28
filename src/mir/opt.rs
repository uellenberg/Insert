use crate::mir::expr::{explore_expr, explore_expr_mut, find_exprs, find_exprs_mut};
use crate::mir::scope::StatementExplorer;
use crate::mir::{MIRExpression, MIRExpressionInner, MIRFunction, MIRStatement, MIRVariable};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Removes trivial (e.g., if(true)) if statements.
/// Returns (success, modified function).
pub fn remove_trivial_ifs(function: &mut MIRFunction) -> (bool, bool) {
    let mut modified = false;

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

                    modified = true;
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
        return (false, false);
    }

    (true, modified)
}

/// Removes statements after a return, continue, or break.
/// Returns (success, modified function).
pub fn remove_dead_code(function: &mut MIRFunction) -> (bool, bool) {
    #[derive(Default, Debug, Clone)]
    struct DeadCodeData {
        is_dead: bool,
    }

    let mut modified = false;

    if !<StatementExplorer<(), DeadCodeData>>::rewrite_block(
        &mut function.body,
        &mut |statement, scope, values| {
            // One of the previous statements was a return/continue/break.
            if scope.scope_data.is_dead {
                modified = true;
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
        return (false, false);
    }

    (true, modified)
}

/// Inlines primitive expressions wherever possible, rather than
/// looking them up from variables.
/// This must run after var_idx is available.
/// Returns (success, modified function).
pub fn inline_primitives<'a>(function: &mut MIRFunction<'a>) -> (bool, bool) {
    #[derive(Default, Debug)]
    struct PrimitiveData<'a> {
        /// Stores the data last stored in a variable in this scope, with the following rules:
        /// - Data won't flow upwards in scope.
        /// - Only primitive expressions are stored here.
        /// - When data is conditionally set, all its parent scopes are invalidated,
        ///   either after or during the parent's execution, e.g., loops with variables in their
        ///   condition which get invalidated in the body must invalidate immediately, whereas ifs
        ///   can defer invalidation since their condition always runs first.
        values: HashMap<usize, MIRExpressionInner<'a>>,

        /// These variables are permanently invalidated from analysis and won't be
        /// inlined.
        perm_invalidated: HashSet<usize>,

        /// Variables that the children have requested we invalidate.
        /// These need to get propagated upwards to our parent.
        needs_invalidation: HashSet<usize>,

        /// A reference to our parent's PrimitiveData, None if we're the top-level scope.
        parent: Option<PrimitiveDataRef<'a>>,

        /// Whether this scope has been initialized yet.
        /// It will be initialized by the first statement that comes across it.
        initialized: bool,
    }

    #[derive(Debug)]
    struct PrimitiveDataRef<'a>(Rc<RefCell<PrimitiveData<'a>>>);

    // This is what's used to create a child scope, so we need to
    // correctly set it up to point to the parent.
    impl Clone for PrimitiveDataRef<'_> {
        fn clone(&self) -> Self {
            let data = self.0.borrow();

            Self(Rc::new(RefCell::new(PrimitiveData {
                values: data.values.clone(),
                perm_invalidated: data.perm_invalidated.clone(),
                needs_invalidation: data.needs_invalidation.clone(),
                parent: Some(PrimitiveDataRef(Rc::clone(&self.0))),
                // Every level needs to reinitialize itself, to pull data from the parent.
                initialized: false,
            })))
        }
    }

    impl Default for PrimitiveDataRef<'_> {
        fn default() -> Self {
            Self(Rc::new(RefCell::new(PrimitiveData::default())))
        }
    }

    #[derive(Default, Debug, Clone)]
    struct ParentData {
        /// These variables need to be added to the invalidation list.
        /// This shouldn't be propagated upwards.
        to_invalidate: HashSet<usize>,
    }

    let mut modified = false;

    if !<StatementExplorer<ParentData, PrimitiveDataRef<'a>>>::explore_block_mut(
        &mut function.body,
        &mut |statement, scope| {
            // If our parent asked us to invalidate some variables, do so.
            // The logic below will handle removing from values.
            {
                let mut data = scope.scope_data.0.borrow_mut();
                if !data.initialized {
                    data.initialized = true;
                    data.needs_invalidation
                        .extend(&scope.parent_data.to_invalidate);
                }
            }

            // Now, we need to apply any invalidations requested by children.
            // SetVariable has no children, so it doesn't really matter that this
            // happens after we read a value from it.
            {
                let mut data = scope.scope_data.0.borrow_mut();
                let data = &mut *data;
                data.values.retain(|var_idx, _| {
                    !data.perm_invalidated.contains(var_idx)
                        && !data.needs_invalidation.contains(var_idx)
                });

                // Unless permanently invalidated, if we set the value again, it'll be valid
                // for inlining once more.
                data.needs_invalidation.clear();
            }

            // TODO: Slight optimization for if statements: inline data in their
            //       condition before invalidation.

            // TODO: When while loops are implemented, they need to additionally use
            //       the invalidated list from parent_data.

            // Now, we can inline any primitive variables that are currently valid.
            if !find_exprs_mut(statement, &mut |expr, write_place| {
                // This means it's SetVariable (or similar)'s write place, which we don't want
                // to inline as it needs to be preserved (only reads get inlined to their known
                // values). SetVariable is safe here because we only save its variable after inlining.
                if write_place {
                    return true;
                }

                explore_expr_mut(expr, &mut |expr| {
                    // This may refer to a static if var_idx == None.
                    if let MIRExpressionInner::Variable(_, Some(var_idx)) = expr.inner {
                        let scope_data = scope.scope_data.0.borrow();

                        if let Some(var_data) = scope_data.values.get(&var_idx) {
                            expr.inner = var_data.clone();
                            modified = true;
                        }
                    }

                    true
                })
            }) {
                return false;
            }

            // If we ever take a reference to the variable, then we can't easily predict what
            // its value will be, so it needs to be completely excluded from inlining.
            // We only need to worry about direct variable refs, since others (index/member)
            // don't apply to primitives.
            if !find_exprs(statement, &mut |expr, _| {
                explore_expr(expr, &mut |expr| {
                    if let MIRExpressionInner::Ref(box MIRExpression {
                        // This may refer to a static if var_idx == None.
                        inner: MIRExpressionInner::Variable(_, Some(var_idx)),
                        ..
                    }) = expr.inner
                    {
                        let mut scope_data = scope.scope_data.0.borrow_mut();

                        scope_data.needs_invalidation.insert(var_idx);
                        scope_data.perm_invalidated.insert(var_idx);
                    }

                    true
                })
            }) {
                return false;
            }

            // We don't have arbitrary scopes, so a child scope is always
            // inside a loop/if/similar.
            // Therefore, if we set a variable, we should always invalidate
            // it in the parent scope.
            //
            // We only care about primitives here, so index and member access can be
            // safely ignored. Derefs mean that we aren't modifying the variable itself, so
            // can similarly be ignored.
            let mut var_idx = 0;
            let mut value = None;

            match statement {
                MIRStatement::SetVariable {
                    place:
                        MIRExpression {
                            // This may refer to a static if var_idx == None.
                            inner: MIRExpressionInner::Variable(_, Some(var_idx_1)),
                            ..
                        },
                    value: value_1,
                    ..
                } => {
                    var_idx = *var_idx_1;
                    value = Some(value_1);
                }
                MIRStatement::CreateVariable {
                    var:
                        MIRVariable {
                            var_idx: var_idx_1, ..
                        },
                    value: value_1,
                    ..
                } => {
                    var_idx = var_idx_1.unwrap();
                    value = value_1.as_mut();
                }
                _ => {}
            }

            if let Some(value) = value {
                if let Some(parent) = &scope.scope_data.0.borrow().parent {
                    parent.0.borrow_mut().needs_invalidation.insert(var_idx);
                }

                // On our scope however, this is a direct set, and can potentially be inlined.
                if matches!(
                    value.inner,
                    // If we ever decide to inline compound operations here, we need to
                    // invalidate any writes that read themselves back (e.g., a = a + 1).
                    // That's because inlining has a different meaning after the statement compared
                    // to within it.
                    MIRExpressionInner::Number(_)
                        | MIRExpressionInner::Bool(_)
                        | MIRExpressionInner::String(_)
                        | MIRExpressionInner::Unit
                        | MIRExpressionInner::Ref(_)
                ) {
                    let mut data = scope.scope_data.0.borrow_mut();

                    if !data.perm_invalidated.contains(&var_idx) {
                        data.values.insert(var_idx, value.inner.clone());
                    }
                }
            }

            true
        },
        &|_, _| true,
        &mut |statement, scope| {
            // For loops, we need to look ahead in the future to all variables a loop could update,
            // then tell the loop to invalidate those.
            // We don't have to permanently invalidate the data, just any writes that happen before
            // the loop, since those conflict with those within the loop.
            // If we read after we write within the loop, then we can inline that.
            // This automatically makes variables created within the loop work correctly, since
            // we only invalidate at the beginning of it.
            if let MIRStatement::LoopStatement { body, .. } = statement {
                let mut to_invalidate = HashSet::new();

                if !<StatementExplorer>::explore_block(
                    body,
                    &mut |statement, scope| {
                        if let MIRStatement::SetVariable {
                            // Member access/index/deref can be ignored for the same reason as above.
                            place:
                                MIRExpression {
                                    inner: MIRExpressionInner::Variable(_, Some(var_idx)),
                                    ..
                                },
                            ..
                        } = statement
                        {
                            to_invalidate.insert(*var_idx);
                        }

                        // We also need to exclude references for the same reason as mentioned above.
                        if !find_exprs(statement, &mut |expr, _| {
                            explore_expr(expr, &mut |expr| {
                                if let MIRExpressionInner::Ref(box MIRExpression {
                                    // This may refer to a static if var_idx == None.
                                    inner: MIRExpressionInner::Variable(_, Some(var_idx)),
                                    ..
                                }) = expr.inner
                                {
                                    to_invalidate.insert(var_idx);
                                }

                                true
                            })
                        }) {
                            return false;
                        }

                        true
                    },
                    &|_, _| true,
                    &|_, _| true,
                ) {
                    return false;
                }

                // All sub-statements should inherit this invalidation, so don't
                // override the value set from the parent.
                scope.parent_data.to_invalidate.extend(to_invalidate);
            }

            true
        },
    ) {
        return (false, false);
    }

    (true, modified)
}
