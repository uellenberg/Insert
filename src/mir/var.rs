use crate::mir::expr::{explore_expr_mut, explore_outer_place, find_exprs_mut};
use crate::mir::scope::{Scope, StatementExplorer};
use crate::mir::{
    MIRContext, MIRExpression, MIRExpressionInner, MIRStatement, MIRTypeInner, MIRVariable,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Gives each variable a unique name and assigns the var_idx.
/// This MUST be ran after all passes which create new variables.
pub fn make_vars_unique(ctx: &mut MIRContext) -> bool {
    for function in ctx.program.functions.values_mut() {
        let mut cur_var_idx = 0;

        if !<StatementExplorer>::explore_block_mut(
            &mut function.body,
            &mut |statement, scope| {
                // For phantom arg variables, we need to save the var_idx to the real arg as well.
                if let MIRStatement::CreateVariable {
                    var:
                        MIRVariable {
                            var_idx,
                            name,
                            arg: true,
                            ..
                        },
                    ..
                } = statement
                {
                    function
                        .args
                        .iter_mut()
                        .find(|arg| arg.name == *name)
                        .expect("Real arg not found for phantom!")
                        .var_idx = Some(var_idx.unwrap());
                }

                // Rewrite expressions to use the new variable.
                // This includes place expressions in SetVariable.
                find_exprs_mut(statement, &mut |expr, _| {
                    explore_expr_mut(expr, &mut |expr| {
                        if let MIRExpressionInner::Variable(name, var_idx) = &mut expr.inner
                            && let Some(var) = scope.get_variable(name)
                        {
                            *var_idx = Some(var.var_idx.unwrap());
                        }

                        true
                    })
                })
            },
            &|_, _| true,
            &mut |statement, _| {
                // This runs before children, so they'll have access to var_idx when they run in for_each.
                if let MIRStatement::CreateVariable {
                    var: MIRVariable { var_idx, .. },
                    ..
                } = statement
                {
                    *var_idx = Some(cur_var_idx);
                    cur_var_idx += 1;
                }

                true
            },
        ) {
            return false;
        }

        // We also need to re-write function args
    }

    true
}

#[derive(Default, Clone, Debug)]
struct MinVarData<'a> {
    /// Variables that our children requested we drop.
    /// This either means a scoped drop, or the variable has been dropped early (liveness analysis).
    /// In any case, having a drop here means that any code after (including in the parent) does not
    /// use that variable or a reference to.
    to_drop: HashSet<usize>,

    /// Variables which have already been dropped.
    /// If a variable is dropped before it's created, then it
    /// just won't be created.
    dropped: HashSet<usize>,

    /// Variable -> Allocated variable it's currently using.
    /// These are scoped locally so that if statement branches don't affect one another.
    allocations: HashMap<usize, usize>,

    /// A reference to our parent's MinVarData, None if we're the top-level scope.
    parent: Option<MinVarDataRef<'a>>,
}

#[derive(Debug)]
struct MinVarDataRef<'a>(Rc<RefCell<MinVarData<'a>>>);

// This is what's used to create a child scope, so we need to
// correctly set it up to point to the parent.
impl Clone for MinVarDataRef<'_> {
    fn clone(&self) -> Self {
        let data = self.0.borrow();

        Self(Rc::new(RefCell::new(MinVarData {
            to_drop: data.to_drop.clone(),
            dropped: data.dropped.clone(),
            allocations: data.allocations.clone(),
            parent: Some(MinVarDataRef(Rc::clone(&self.0))),
        })))
    }
}

impl Default for MinVarDataRef<'_> {
    fn default() -> Self {
        Self(Rc::new(RefCell::new(MinVarData::default())))
    }
}

/// This reduces the number of variables in each function by reusing variables
/// when they get dropped.
/// After this is run, all drops will be removed.
/// This must be run after var_idx is assigned, and only has an effect after
/// drops are added. Additionally, this will create conflicting variables, so
/// a renaming step must occur afterwards to make the output valid.
pub fn min_vars<'a>(ctx: &mut MIRContext<'a>) -> bool {
    for function in ctx.program.functions.values_mut() {
        // We need to hoist create var statements up to the top of the function.
        // The create vars that we don't use can just be discarded, though.
        let mut creates: Vec<MIRStatement> = vec![];
        // Type -> vars that have been allocated already.
        let mut vars: HashMap<MIRTypeInner<'a>, Vec<usize>> = HashMap::new();

        if !<StatementExplorer<(), MinVarDataRef<'a>>>::rewrite_block(
            &mut function.body,
            &mut |mut statement, scope, statements| {
                // Update the current allocations based on this statement, and ensure
                // that we have a variable allocated for CreateVariable.
                // This may want to early return if the statement should be dropped.
                if let Some(value) =
                    update_var_allocations(&mut creates, &mut vars, &mut statement, scope)
                {
                    return value;
                }

                // Rewrite to use the new allocations.
                if !find_exprs_mut(&mut statement, &mut |expr, _| {
                    explore_expr_mut(expr, &mut |expr| {
                        if let MIRExpressionInner::Variable(_, Some(var_idx)) = &mut expr.inner {
                            *var_idx = scope.scope_data.0.borrow().allocations[var_idx];
                        }

                        true
                    })
                }) {
                    return false;
                }

                // The statement has been fully processed, so we can add it back now.
                statements.push(statement);

                // Propagate drops upwards.
                // We need to do this after handling our own statement because expressions in
                // this statement generally run before the body (e.g., if a var is dropped in an if body,
                // we need to update its condition first, then deallocate, since the condition happens first).
                {
                    let data = &mut *scope.scope_data.0.borrow_mut();
                    if !data.to_drop.is_empty() {
                        // Keys are the variables pointing to allocated variables.
                        data.allocations
                            .retain(|var_idx, _| !data.to_drop.contains(var_idx));
                        data.dropped.extend(data.to_drop.iter());
                        if let Some(parent) = &data.parent {
                            parent.0.borrow_mut().to_drop.extend(data.to_drop.iter());
                        }

                        // This is no longer needed, although behavior won't change if we
                        // leave it full.
                        data.to_drop.clear();
                    }
                }

                true
            },
            &mut |_, _| true,
            &|_, _, _| true,
        ) {
            return false;
        }

        // Any created variables must be prepended to the start, since they can be used at any
        // point in the function.
        function.body.splice(0..0, creates);
    }

    true
}

/// Updates the current variable allocations according to the statement.
/// If this is a CreateVariable statement, then it ensures the variable is
/// properly allocated.
/// If it's a DropVariable statement, then it marks it as dropped and handles
/// deallocation.
///
/// If this returns Some, the caller should return with the inner value as
/// the status. Some(true) means we succeeded, but shouldn't go any further
/// than updating allocations (i.e., the statement should be dropped).
fn update_var_allocations<'a>(
    creates: &mut Vec<MIRStatement<'a>>,
    vars: &mut HashMap<MIRTypeInner<'a>, Vec<usize>>,
    statement: &mut MIRStatement<'a>,
    scope: &mut Scope<'a, (), MinVarDataRef<'a>>,
) -> Option<bool> {
    match &statement {
        MIRStatement::DropVariable(_, var_idx, _) => {
            // We need to make this dropped locally and push it up to our parents so
            // they can do the same.
            let mut data = scope.scope_data.0.borrow_mut();
            // Keys are the variables pointing to allocated variables.
            data.allocations
                .retain(|check_var_idx, _| check_var_idx != var_idx);
            data.dropped.insert(*var_idx);
            if let Some(parent) = &data.parent {
                parent.0.borrow_mut().to_drop.insert(*var_idx);
            }

            // We've processed this Drop, so no need to keep it around.
            return Some(true);
        }

        MIRStatement::CreateVariable { var, span, value } => {
            // Try to allocate. If we don't have space, then we'll need to
            // add this variable to the list of allocated vars.

            if scope
                .scope_data
                .0
                .borrow()
                .dropped
                .contains(&var.var_idx.unwrap())
                // Even if the variable is dead, we can't remove args.
                && !var.arg
            {
                // Dead variable.
                return Some(true);
            }

            let available = {
                let data = scope.scope_data.0.borrow();
                // Values are the allocated variables.
                let used_allocations = data.allocations.values().cloned().collect::<HashSet<_>>();

                vars.entry(var.ty.ty.clone())
                    .or_default()
                    .iter()
                    .cloned()
                    .find(|var_idx| !used_allocations.contains(var_idx))
            };
            // If there's no existing space, we'll need to allocate this variable.
            let available = match available {
                Some(available) => available,
                None => {
                    // No space, so we need to allocate this variable.
                    let var_idx = var.var_idx.unwrap();

                    creates.push(MIRStatement::CreateVariable {
                        var: var.clone(),
                        span: span.clone(),
                        // Hoisted variables can't have any data, since there might
                        // be dependencies here.
                        // A separate pass can clean this up and merge Create + Set into
                        // a single Create.
                        value: None,
                    });
                    vars.entry(var.ty.ty.clone()).or_default().push(var_idx);

                    var_idx
                }
            };

            // We have an allocation to use.
            // We can just remap to a SetVariable, and rely
            // on the code below to handle remapping to the new var.
            scope
                .scope_data
                .0
                .borrow_mut()
                .allocations
                .insert(var.var_idx.unwrap(), available);

            let Some(value) = value else {
                // No need to continue below, since we have no expressions
                // to fix.
                return Some(true);
            };

            *statement = MIRStatement::SetVariable {
                place: MIRExpression {
                    // Use the remapping code below to handle this.
                    // If we partially remap here but don't remap the value,
                    // it'll make things complicated.
                    inner: MIRExpressionInner::Variable(var.name.clone(), var.var_idx),
                    span: span.clone(),
                    ty: Some(var.ty.clone()),
                },
                value: value.clone(),
                span: span.clone(),
            };
        }

        MIRStatement::SetVariable { place, .. } => {
            let mut dead = false;

            if !explore_outer_place(place, &mut |expr| {
                if let MIRExpressionInner::Variable(_, Some(var_idx)) = &expr.inner
                    && scope.scope_data.0.borrow().dropped.contains(var_idx)
                {
                    // Dead set.
                    dead = true;
                }

                true
            }) {
                return Some(false);
            }

            if dead {
                // Don't push to statements.
                return Some(true);
            }
        }
        _ => {}
    }

    // Keep going.
    None
}
