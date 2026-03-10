use crate::mir::expr::{explore_expr, explore_expr_mut, find_exprs, find_exprs_mut};
use crate::mir::quine::ensure_no_markers_block;
use crate::mir::scope::{Scope, StatementExplorer};
use crate::mir::{MIRExpression, MIRExpressionInner, MIRFunction, MIRStatement, MIRVariable};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::slice;

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
                        // We're dropping the else block, so if there are markers,
                        // we need to error.
                        ensure_no_markers_block(&on_false);
                    } else {
                        values.extend(on_false);
                        ensure_no_markers_block(&on_true);
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
            // We can't remove markers though, since they aren't really "code",
            // and it will lead to unexpected results if we do so.
            if scope.scope_data.is_dead
                && !matches!(statement, MIRStatement::MarkerStatement { .. })
            {
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

    /// If we inline a variable read, then if the variable we read gets invalidated,
    /// we also need to invalidate the variable it's stored in, cascading.
    /// Variable -> variable it needs to invalidate when the key gets written to
    var_values: HashMap<usize, usize>,

    /// These variables are permanently invalidated from analysis and won't be
    /// inlined.
    perm_invalidated: HashSet<usize>,

    /// Variables that the children have requested we invalidate.
    /// These need to get propagated upwards to our parent.
    needs_invalidation: HashSet<usize>,

    /// Variables that the children have requested we permanently invalidate.
    needs_perm_invalidation: HashSet<usize>,

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
            var_values: data.var_values.clone(),
            perm_invalidated: data.perm_invalidated.clone(),
            needs_invalidation: data.needs_invalidation.clone(),
            needs_perm_invalidation: data.needs_perm_invalidation.clone(),
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
struct InvalidateParentData {
    /// These variables need to be added to the invalidation list.
    /// This shouldn't be propagated upwards.
    to_invalidate: HashSet<usize>,
}

/// Inlines primitive expressions wherever possible, rather than
/// looking them up from variables.
/// This must run after var_idx is available.
/// Returns (success, modified function).
pub fn inline_primitives<'a>(function: &mut MIRFunction<'a>) -> (bool, bool) {
    let mut modified = false;

    if !<StatementExplorer<InvalidateParentData, PrimitiveDataRef<'a>>>::explore_block_mut(
        &mut function.body,
        &mut |statement, scope| {
            // If our parent asked us to invalidate some variables, do so.
            // We need to invalidate the values here instead of adding to needs_invalidation,
            // since the code below has some conditional handling, whereas we always need to
            // invalidate here.
            {
                let mut data = scope.scope_data.0.borrow_mut();
                if !data.initialized {
                    data.initialized = true;
                    data.values
                        .retain(|var_idx, _| !scope.parent_data.to_invalidate.contains(var_idx));
                }
            }

            // If we ever take a reference to the variable, then we can't easily predict what
            // its value will be, so it needs to be completely excluded from inlining.
            // We do it here because otherwise validity is based on ordering, e.g,,
            // for a + val(&a), if we evaluate a first we can inline, but if we evaluate val first
            // we can't, since it might have changed a. So, it's easier to just invalidate a to begin
            // with and not worry about it.
            //
            // This is also a bit of a hack for the code below. Since we invalidate all references,
            // we'll never inline Ref<Variable> by accident, since we simply skip that case. But
            // the code doesn't really check to prevent inlining a variable within a ref, it just relies
            // on this.
            if !invalidate_refs(statement, scope) {
                return false;
            }

            // We need a special case for if statements: they have an expression in their condition,
            // but since their children run afterward, we can hold off on invalidations until after
            // we inline in their condition.
            //
            // This ISN'T true for while loops, since their condition runs multiple times, it must have
            // the same invalidations applied to it.
            let do_invalidation_after = matches!(statement, MIRStatement::IfStatement { .. });

            if !do_invalidation_after {
                forward_invalidations(scope);
            }

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

            if do_invalidation_after {
                forward_invalidations(scope);
            }

            // Save the variable if we can, and invalidate it in the parent scope, since
            // it's now indeterminate for it.
            process_var_write(statement, scope);

            true
        },
        &|_, _| true,
        &mut |statement, scope| invalidate_loop_writes(statement, scope),
    ) {
        return (false, false);
    }

    (true, modified)
}

/// Permanently invalidates any variables that have refs created to them.
fn invalidate_refs(
    statement: &mut MIRStatement,
    scope: &mut Scope<InvalidateParentData, PrimitiveDataRef>,
) -> bool {
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

                scope_data.perm_invalidated.insert(var_idx);
                // We need to invalidate directly, since we need it to affect
                // the next statement.
                cascade_invalidations(var_idx, &mut *scope_data);
                if let Some(parent) = &scope_data.parent {
                    parent
                        .0
                        .borrow_mut()
                        .needs_perm_invalidation
                        .insert(var_idx);
                }
            }

            true
        })
    }) {
        return false;
    }

    true
}

fn process_var_write<'a>(
    statement: &mut MIRStatement<'a>,
    scope: &mut Scope<'a, InvalidateParentData, PrimitiveDataRef<'a>>,
) {
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
            var: MIRVariable {
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
        let mut data = scope.scope_data.0.borrow_mut();

        // Preemptively invalidate this variable - if we write to it, it'll be
        // valid again, and we need to properly handle cascading on every write.
        cascade_invalidations(var_idx, &mut *data);

        // On our scope however, this is a direct set, and can potentially be inlined.
        if !data.perm_invalidated.contains(&var_idx)
            && (matches!(
                    value.inner,
                    // If we ever decide to inline compound operations here, we need to
                    // invalidate any writes that read themselves back (e.g., a = a + 1).
                    // That's because inlining has a different meaning after the statement compared
                    // to within it.
                    //
                    // Binding cannot be used here, otherwise we'll duplicate the markers and
                    // make it impossible for quines to properly replace them.
                    MIRExpressionInner::Number(_)
                    | MIRExpressionInner::Bool(_)
                    | MIRExpressionInner::String(_)
                    | MIRExpressionInner::Unit
                    | MIRExpressionInner::Ref(_)
                    // A read from a static is always valid to inline.
                    | MIRExpressionInner::Variable(_, None)
                )
            // If the variable was never a reference, we can inline it until
            // it's next written / referenced.
            || matches!(value.inner, MIRExpressionInner::Variable(_, Some(inner_var_idx)) if !data.perm_invalidated.contains(&inner_var_idx)))
        {
            data.values.insert(var_idx, value.inner.clone());
        }
    }
}

/// Invalidates var_idx and any variables which reference it.
/// This will update the parent.
fn cascade_invalidations(var_idx: usize, data: &mut PrimitiveData) {
    data.values.remove(&var_idx);
    if let Some(parent) = &data.parent {
        parent.0.borrow_mut().needs_invalidation.insert(var_idx);
    }

    let mut cur_var_idx = var_idx;
    while let Some(parent_var_idx) = data.var_values.remove(&cur_var_idx) {
        data.values.remove(&parent_var_idx);
        if let Some(parent) = &data.parent {
            parent
                .0
                .borrow_mut()
                .needs_invalidation
                .insert(parent_var_idx);
        }

        cur_var_idx = parent_var_idx;
    }
}

/// Tells a loop to invalidate any variables it writes to.
fn invalidate_loop_writes<'a>(
    statement: &mut MIRStatement<'a>,
    scope: &mut Scope<'a, InvalidateParentData, PrimitiveDataRef<'a>>,
) -> bool {
    // For loops, we need to look ahead in the future to all variables a loop could update,
    // then tell the loop to invalidate those.
    // We don't have to permanently invalidate the data, just any writes that happen before
    // the loop, since those conflict with those within the loop.
    // If we read after we write within the loop, then we can inline that.
    // This automatically makes variables created within the loop work correctly, since
    // we only invalidate at the beginning of it.
    if matches!(statement, MIRStatement::LoopStatement { .. }) {
        let mut to_invalidate = HashSet::new();

        if !<StatementExplorer>::explore_block(
            slice::from_ref(statement),
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
}

/// Applies any invalidations requested by the children, and forwards them to our parent.
fn forward_invalidations(scope: &Scope<InvalidateParentData, PrimitiveDataRef>) {
    let mut data = scope.scope_data.0.borrow_mut();
    let data = &mut *data;

    // If the children requested we permanently invalidate, do so.
    // We don't have to do this for normal invalidations, since they're
    // just a one-time thing applied below.
    data.perm_invalidated
        .extend(data.needs_perm_invalidation.iter());

    // No need to permanently invalidate any vars that are set to this
    // variable, but we do need to invalidate them so they don't inline
    // a value which is permanently invalidated.
    for var_idx in data.perm_invalidated.clone() {
        cascade_invalidations(var_idx, data);
    }

    data.values.retain(|var_idx, _| {
        !data.perm_invalidated.contains(var_idx) && !data.needs_invalidation.contains(var_idx)
    });

    // Propagate upwards.
    if let Some(parent_data) = &data.parent {
        let mut parent_data = parent_data.0.borrow_mut();
        parent_data
            .needs_perm_invalidation
            .extend(data.needs_perm_invalidation.iter());
        parent_data
            .needs_invalidation
            .extend(data.needs_invalidation.iter());
    }

    // Unless permanently invalidated, if we set the value again, it'll be valid
    // for inlining once more.
    data.needs_invalidation.clear();
}
