use crate::mir::expr::{explore_expr, explore_outer_place, find_exprs};
use crate::mir::scope::{Scope, StatementExplorer};
use crate::mir::{
    MIRContext, MIRExpression, MIRExpressionInner, MIRFnCall, MIRStatement, MIRTypeInner,
    MIRVariable,
};
use crate::parser::span::{Span, eprintln_span};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::slice;

/// Performs variable liveness analysis and inserts drops as early
/// as possible, according to the following rules:
/// - References to variables may not live longer than those variables,
///   and may not be stored in a variable that lives longer (i.e., storing a
///   reference to a local in a static is UB).
/// - Variables must outlive all references to them.
/// - Drops inside blocks propagate to their parent, although
///   this propagation isn't handled by this code.
/// - For statements with multiple child blocks (i.e., if else),
///   all children must contain the same drops.
/// - Drops won't be inserted if one already exists at a later point.
///   This means user-created drops can extend the lifetime of variables.
///
/// Variables which are completely dead are removed entirely, although
/// their drops may remain.
///
/// This must be run after var_idx is assigned.
pub fn add_live_drops(ctx: &mut MIRContext) -> bool {
    for (_, function) in &mut ctx.program.functions {
        // We need to track which variables reference which other
        // variables, since we can only drop a variable once all its
        // references have been dropped.
        // This maps variables (var_idx) to the variables they reference.
        let mut relationships = HashMap::new();
        if !compute_var_relationships(&function.body, &mut relationships) {
            return false;
        }

        // It isn't enough to just look at direct references.
        // We also need to compute it transitively, since we can access
        // our variable through a double reference.
        apply_transitive_relationships(&mut relationships);

        // Now, we can add drops based on these relationships.
        add_drops(&mut function.body, &relationships);
    }

    true
}

#[derive(Default, Debug)]
struct DropData<'a> {
    /// Variables which we have already dropped.
    /// Counterintuitively, since we go backwards, this means
    /// these variables are currently live, and every other variable
    /// is dead.
    dropped: HashSet<usize>,

    /// Variables which our children dropped.
    to_drop: HashSet<usize>,

    /// Variables which the child needs the parent to drop in its own scope.
    /// This is different from to_drop, since we create DropVariable statements.
    inject_drops: HashSet<usize>,

    /// A reference to our parent's PrimitiveData, None if we're the top-level scope.
    parent: Option<DropDataRef<'a>>,
}

#[derive(Debug)]
struct DropDataRef<'a>(Rc<RefCell<DropData<'a>>>);

// This is what's used to create a child scope, so we need to
// correctly set it up to point to the parent.
impl Clone for DropDataRef<'_> {
    fn clone(&self) -> Self {
        let data = self.0.borrow();

        Self(Rc::new(RefCell::new(DropData {
            dropped: data.dropped.clone(),
            to_drop: data.to_drop.clone(),
            inject_drops: data.inject_drops.clone(),
            parent: Some(DropDataRef(Rc::clone(&self.0))),
        })))
    }
}

impl Default for DropDataRef<'_> {
    fn default() -> Self {
        Self(Rc::new(RefCell::new(DropData::default())))
    }
}

#[derive(Default, Debug, Clone)]
struct DropParentData {
    /// Is our parent (direct or indirect) a loop?
    is_loop: bool,

    /// These are variables created within the loop,
    /// which we are allowed to drop.
    /// All other variables must be hoisted up to the parent
    /// by setting inject_drops.
    inner_vars: HashSet<usize>,

    /// When pre_run encounters a LoopStatement, it overwrites
    /// is_loop and inner_vars with the child loop's context
    /// (for children to inherit via scope.child()). However,
    /// for_each runs at the enclosing scope level and needs the
    /// original values for its hoisting decisions. This field
    /// saves the original is_loop and inner_vars before the
    /// overwrite.
    ///
    /// Only consulted in for_each when the current statement is
    /// a LoopStatement (the only case where is_loop/inner_vars
    /// have been overwritten).
    enclosing_loop: Option<(bool, HashSet<usize>)>,
}

/// Adds drops to the block, according to the relationships computed earlier.
/// Relationships is var_idx -> all var_idx that it might reference.
/// These must be transitively evaluated.
fn add_drops(block: &mut Vec<MIRStatement>, relationships: &HashMap<usize, HashSet<usize>>) {
    // We can now insert a drop, from the bottom-up, in the following conditions:
    // - We haven't already inserted a drop **in this scope** (if else can drop the same varibale twice).
    // - A sibling block inserted a drop that we didn't (in which case, we can place the
    //   drop at the top of our block).
    <StatementExplorer<DropParentData, DropDataRef>>::rewrite_block_rev(
        block,
        &mut |mut statement, scope, block| {
            // Special case: if we encounter a CreateVariable that's unused,
            // we can assume the drop happens earlier and remove it entirely.
            if let MIRStatement::CreateVariable {
                var:
                    MIRVariable {
                        // We can't eliminate arg CreateVariables, since we need
                        // them for analysis.
                        arg: false,
                        var_idx,
                        ..
                    },
                ..
            } = &statement
                && !scope
                    .scope_data
                    .0
                    .borrow()
                    .dropped
                    .contains(&var_idx.unwrap())
            {
                // If this variable is used anywhere, the drop will tell future
                // passes that it's a dead set.
                block.push(MIRStatement::DropVariable(
                    "".into(),
                    var_idx.unwrap(),
                    Span::empty(),
                ));
                return true;
            }

            // Because if statements have parallel branches, we need to make sure that
            // drops occur in both.
            // If a variable is unused in one branch, we can just add a drop to the very top,
            // since that's the first point where we know the variable will be unused.
            if let MIRStatement::IfStatement {
                on_true, on_false, ..
            } = &mut statement
            {
                let data = scope.scope_data.0.borrow();

                // to_drop is the union of variables dropped in both branches.
                // Any used variables will already have been dropped.
                inject_drops_on_unused(on_true, &data.to_drop);
                inject_drops_on_unused(on_false, &data.to_drop);
            }

            // If this statement's children dropped something, we need to
            // mark those as dropped so we don't try to drop them again.
            let mut drops_from_children: HashSet<usize> = HashSet::new();

            {
                let data = &mut *scope.scope_data.0.borrow_mut();
                // Drain to ensure to_drop doesn't leak between statements in the
                // same scope, since it's used for, e.g., if statements.
                drops_from_children.extend(data.to_drop.iter());
                data.dropped.extend(data.to_drop.drain());
            }

            let mut drop_after: HashSet<usize> = HashSet::new();

            // Any undropped variables we encounter will be their last usage, so we should
            // drop them immediately after this statement.
            find_exprs(&statement, &mut |expr, place_write| {
                // Directly writing to a variable doesn't count as usage.
                // If it's through a reference though, it gets more complicated to analyze.
                if place_write && matches!(expr.inner, MIRExpressionInner::Variable(_, _)) {
                    return true;
                }

                explore_expr(expr, &mut |expr| {
                    if let MIRExpressionInner::Variable(_, Some(var_idx)) = &expr.inner
                        && !scope.scope_data.0.borrow().dropped.contains(var_idx)
                    {
                        drop_after.insert(*var_idx);
                    }

                    true
                })
            });

            // The child may ask us to hoist up some drops (for loops).
            // This needs to be empty so we don't double drop.
            drop_after.extend(scope.scope_data.0.borrow_mut().inject_drops.drain());

            // If any of these variables are references, then we need to drop
            // any potential references as well. This is because it can't outlive
            // the data it references, so this is the earliest spot to drop them as
            // well.
            //
            // Since transitive relationships are already fully resolved, we just
            // need to use the values immediately within relationships.
            let pull_indices = drop_after.iter().copied().collect::<Vec<_>>();
            for var_idx in pull_indices {
                if let Some(referenced) = relationships.get(&var_idx) {
                    drop_after.extend(referenced.iter());
                }
            }

            // We avoid already-dropped variables when they're initially added,
            // but some more may added when adding references above.
            drop_after.retain(|&var_idx| !scope.scope_data.0.borrow().dropped.contains(&var_idx));

            // If we're in a loop, we can only drop variables we've created locally.
            // Otherwise, they need to be hoisted up to the parent.
            // That's because the loop runs multiple times, so the variables will
            // be used after a drop otherwise.
            //
            // When this statement is a LoopStatement, pre_run will have overwritten
            // is_loop/inner_vars with the child loop's context (this statement).
            // We need to use the context of the outer loop instead (which is the same
            // as what any other non-loop statement would use).
            //
            // loop a { <-- inner_vars = {val}
            //     let val = 1; <-- inner_vars = {val}
            //     loop b { <-- inner_vars = {innerval}
            //         let innerval = 2; <-- inner_vars = {innerval}
            //         val = 20; <-- inner_vars = {innerval}
            //     }
            // }
            //
            // If we're in loop b, our children will have requested us to hoist up the drop
            // for val.
            // However, inner_vals refers to our own context, even though we're going to place
            // the drop into loop a's context.
            // So we need to use enclosing_loop (inner_vars = {val}) instead, which is loop a's context.
            let (hoist_is_loop, hoist_inner_vars) =
                if matches!(statement, MIRStatement::LoopStatement { .. }) {
                    scope
                        .parent_data
                        .enclosing_loop
                        .as_ref()
                        .map(|(is_loop, vars)| (*is_loop, vars))
                        .unwrap_or((scope.parent_data.is_loop, &scope.parent_data.inner_vars))
                } else {
                    (scope.parent_data.is_loop, &scope.parent_data.inner_vars)
                };

            if hoist_is_loop {
                let scope_data = scope.scope_data.0.borrow();
                let parent = scope_data
                    .parent
                    .as_ref()
                    .expect("Loop must have a parent scope");

                let drop_in_parent = drop_after.extract_if(|val| !hoist_inner_vars.contains(val));
                parent.0.borrow_mut().inject_drops.extend(drop_in_parent);
            }

            block.push(statement);
            block.extend(
                drop_after
                    .iter()
                    .map(|&var_idx| MIRStatement::DropVariable("".into(), var_idx, Span::empty())),
            );

            // Now, we need to keep track of these drops so that we don't do them again, and
            // inform our parent about them so it doesn't either.
            {
                let data = &mut *scope.scope_data.0.borrow_mut();

                data.dropped.extend(drop_after.iter());
                if let Some(parent) = &data.parent {
                    parent
                        .0
                        .borrow_mut()
                        .to_drop
                        .extend(drop_after.iter().chain(drops_from_children.iter()));
                }
            }

            true
        },
        &mut |_, _| true,
        &|statement, scope, _| invalidate_loop_writes(statement, scope),
    );
}

/// Tells a loop which variables it has created.
/// This lets us hoist drops to non-inner variables outside
/// the loop.
fn invalidate_loop_writes<'a>(
    statement: &MIRStatement<'a>,
    scope: &mut Scope<'a, DropParentData, DropDataRef<'a>>,
) -> bool {
    // For loops, we can't drop a variable created outside the loop inside it,
    // since that would cause multiple drops.
    // This finds the inner variables which the loop is allowed to drop.
    if matches!(statement, MIRStatement::LoopStatement { .. }) {
        let mut inner_vars = HashSet::new();

        if !<StatementExplorer>::explore_block(
            slice::from_ref(statement),
            &mut |statement, _scope| {
                if let MIRStatement::CreateVariable {
                    var: MIRVariable { var_idx, .. },
                    ..
                } = statement
                {
                    inner_vars.insert(var_idx.unwrap());
                }

                true
            },
            &|_, _| true,
            &|_, _| true,
        ) {
            return false;
        }

        // We need to save the enclosing loop, since parent_data affects
        // both the loop's body as well as the loop statement itself.
        // The loop statement must know the inner_vars of its enclosing loop,
        // to determine which ones can be dropped and which need to be hoisted.
        scope.parent_data.enclosing_loop = Some((
            scope.parent_data.is_loop,
            scope.parent_data.inner_vars.clone(),
        ));
        scope.parent_data.inner_vars = inner_vars;
        scope.parent_data.is_loop = true;
    }

    true
}

/// Adds drop statements to the top of this block for each of the variables
/// mentioned in drops, so long as those variables are unused.
fn inject_drops_on_unused(block: &mut Vec<MIRStatement>, drops: &HashSet<usize>) {
    let mut drops = drops.clone();

    // If we use a variable, we can't drop it early.
    <StatementExplorer>::explore_block(
        block,
        &mut |statement, _scope| {
            find_exprs(&statement, &mut |expr, place_write| {
                explore_expr(expr, &mut |expr| {
                    // This needs to match the behavior of the usage finding
                    // code in add_drops.
                    if place_write && matches!(expr.inner, MIRExpressionInner::Variable(_, _)) {
                        return true;
                    }

                    if let MIRExpressionInner::Variable(_, Some(var_idx)) = &expr.inner {
                        drops.remove(var_idx);
                    }

                    true
                })
            });

            true
        },
        &|_, _| true,
        &|_, _| true,
    );

    let drops = drops
        .into_iter()
        .map(|var_idx| MIRStatement::DropVariable("".into(), var_idx, Span::empty()));
    block.splice(0..0, drops);
}

/// Populates relationships with var_idx -> all var_idx that it might reference, according to the following rules:
/// - For SetVariable/CreateVariable place expr a, if we take a reference to variable b,
///   and a is a reference type, then a is a reference to b.
/// - For function call, if any argument a has type Ref<b>, where b is another argument
///   that is itself a reference type, then a is a reference to b.
///   This is because we can store a reference to b within a, thus allowing it to escape
///   our checks. In other words, we act as though the function contains *a = b;
///   The args don't follow through to the statement if it's used in an expression, however.
/// - For a function call, if the return value is a reference, any reference taken within
///   the args is treated as a reference that could be output by the function, which then
///   flows upwards to the statement.
fn compute_var_relationships(
    body: &[MIRStatement],
    relationships: &mut HashMap<usize, HashSet<usize>>,
) -> bool {
    <StatementExplorer>::explore_block(
        body,
        &mut |statement, _scope| {
            // Functions can alias pointers.
            find_exprs(statement, &mut |expr, _| {
                if let MIRExpressionInner::FunctionCall(fn_call) = &expr.inner {
                    handle_fn_arg_relationships(fn_call, relationships);
                }
                true
            });

            match statement {
                MIRStatement::CreateVariable {
                    var,
                    value: Some(value),
                    ..
                } => {
                    if is_ref(&var.ty.ty) {
                        let referenced = collect_refs(value, relationships);
                        if !referenced.is_empty() {
                            relationships
                                .entry(var.var_idx.unwrap())
                                .or_default()
                                .extend(referenced);
                        }
                    }

                    // Arrays are weird, since they're created as values but passed
                    // by reference. We need to make them "refer to themselves", that way
                    // any usage of the array will refer back to it.
                    // TODO: What to do if an array is inside a struct?
                    if matches!(
                        var.ty.ty,
                        MIRTypeInner::Array(_) | MIRTypeInner::ArrayFixed(_, _)
                    ) {
                        relationships
                            .entry(var.var_idx.unwrap())
                            .or_default()
                            .insert(var.var_idx.unwrap());
                    }
                }

                MIRStatement::SetVariable {
                    place, value, span, ..
                } => {
                    let var_idx = extract_place_var(place);

                    if place.ty.as_ref().is_some_and(|ty| is_ref(&ty.ty)) {
                        let referenced = collect_refs(value, relationships);
                        if !referenced.is_empty() {
                            // No var_idx means we're writing to a static, which is UB.
                            // We can be nice and report it (doing a span error requires restructuring, though).
                            // If the reference isn't pointing to a variable though, it's fine.
                            // That's why we do the check here (when referenced is not empty, meaning
                            // it does point to a variable).
                            let Some(var_idx) = var_idx.expect("Reference with no variable!")
                            else {
                                eprintln_span!(
                                    Some(span.clone()),
                                    "Reference escapes to static {statement}!"
                                );
                                return false;
                            };

                            relationships.entry(var_idx).or_default().extend(referenced);
                        }
                    }
                }

                // Functions can alias pointers.
                MIRStatement::FunctionCall(fn_call) => {
                    handle_fn_arg_relationships(fn_call, relationships);
                }

                _ => {}
            }

            true
        },
        &|_, _| true,
        &|_, _| true,
    )
}

/// Finds the variable being references in a place expression.
/// If no variable was found, returns None.
/// Otherwise, returns the variable's var_idx (None for non-locals).
fn extract_place_var(expr: &MIRExpression) -> Option<Option<usize>> {
    let mut root = None;
    explore_outer_place(expr, &mut |expr| {
        if let MIRExpressionInner::Variable(_, var_idx) = &expr.inner {
            root = Some(var_idx.clone());
        }

        true
    });

    root
}

/// Collects all var_idx that could be referenced by the result of this expression.
fn collect_refs(
    expr: &MIRExpression,
    relationships: &HashMap<usize, HashSet<usize>>,
) -> HashSet<usize> {
    match &expr.inner {
        // When taking a reference directly, we need to find the root variable,
        // i.e., &expr.inner -> expr.
        // Referencing adds an extra layer of indirection, so we use this variable
        // and not its references.
        MIRExpressionInner::Ref(inner) => {
            let place = extract_place_var(inner).expect("Reference with no variable!");
            if let Some(var_idx) = place {
                HashSet::from([var_idx])
            } else {
                // Non-local
                HashSet::new()
            }
        }

        // Variable access is a copy, so we can just copy its references.
        MIRExpressionInner::Variable(_, var_idx) => {
            if let Some(var_idx) = var_idx
                && expr
                    .ty
                    .as_ref()
                    .is_some_and(|ty| matches!(ty.ty, MIRTypeInner::Ref(_)))
            {
                relationships.get(var_idx).cloned().unwrap_or_default()
            } else {
                // Non-local
                HashSet::new()
            }
        }

        // Flow through.
        // We only consider the base, as the index isn't where the data is stored.
        MIRExpressionInner::Index(inner, _)
        | MIRExpressionInner::Member(inner, _)
        | MIRExpressionInner::Deref(inner)
        | MIRExpressionInner::Binding(_, inner, _) => collect_refs(inner, relationships),

        // If this returns a reference, then that reference could come from any of the args.
        MIRExpressionInner::FunctionCall(fn_call) => {
            if fn_call
                .ret_ty
                .as_ref()
                .is_some_and(|ty| contains_ref(&ty.ty))
            {
                let mut result = HashSet::new();
                for arg in &fn_call.args {
                    result.extend(collect_refs(arg, relationships));
                }

                result
            } else {
                HashSet::new()
            }
        }

        MIRExpressionInner::Array(elems) => elems
            .iter()
            .flat_map(|expr| collect_refs(expr, relationships))
            .collect(),

        // Binary ops, literals, etc: no refs flow through
        MIRExpressionInner::Add(..)
        | MIRExpressionInner::Sub(..)
        | MIRExpressionInner::Mul(..)
        | MIRExpressionInner::Div(..)
        | MIRExpressionInner::Equal(..)
        | MIRExpressionInner::NotEqual(..)
        | MIRExpressionInner::Less(..)
        | MIRExpressionInner::Greater(..)
        | MIRExpressionInner::LessEq(..)
        | MIRExpressionInner::GreaterEq(..)
        | MIRExpressionInner::BoolAnd(..)
        | MIRExpressionInner::BoolOr(..)
        | MIRExpressionInner::Number(..)
        | MIRExpressionInner::String(..)
        | MIRExpressionInner::Bool(..)
        | MIRExpressionInner::Char(..)
        | MIRExpressionInner::Unit
        | MIRExpressionInner::Quine
        | MIRExpressionInner::QuineLen
        | MIRExpressionInner::QuineSpace
        | MIRExpressionInner::QuineLine => HashSet::new(),
    }
}

/// Handles function call arg relationships.
/// If arg a has type Ref<Ref> and arg b has type Ref,
/// the function could do *a = b, making a reference b.
fn handle_fn_arg_relationships(
    fn_call: &MIRFnCall,
    relationships: &mut HashMap<usize, HashSet<usize>>,
) {
    // This will point to the values behind the refs, so if we write
    // a(&val1, &val2), were val1: u32 and val2: &u32, we'll get val1
    // for single and val2 for double.
    // Similarly, if we do a(val3, &val2), where val3: &u32, we'll get
    // the values that val3 could be pointing to.
    let mut double_refs: HashSet<usize> = HashSet::new();
    let mut single_refs: HashSet<usize> = HashSet::new();

    for arg in &fn_call.args {
        // All refs are single refs.
        single_refs.extend(collect_refs(arg, relationships));

        // Double refs can be nested in expressions.
        explore_expr(arg, &mut |expr| {
            let expr_ty = &expr.ty.as_ref().unwrap().ty;

            if let MIRTypeInner::Ref(inner) = expr_ty
                && contains_ref(inner)
            {
                double_refs.extend(collect_refs(expr, relationships));
            }

            true
        });
    }

    // This is overly conservative, since it doesn't consider reference depth or the
    // inner types, and treats all references as equal.

    // Each double-ref could now reference what any single-ref references.
    for double_ref in double_refs {
        relationships
            .entry(double_ref)
            .or_default()
            // Remove self-cycles, since we can never point to ourselves.
            .extend(single_refs.iter().filter(|&&x| x != double_ref));
    }
}

/// Applies relationships transitively, so that if a references b, and b references c, then a references c.
/// Relationships is var_idx -> all var_idx that it might reference.
fn apply_transitive_relationships(relationships: &mut HashMap<usize, HashSet<usize>>) {
    loop {
        let mut modified = false;

        let keys: Vec<usize> = relationships.keys().cloned().collect();

        for ref_var in keys {
            let current_refs: Vec<usize> = relationships
                .get(&ref_var)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();

            for referenced_var in current_refs {
                if let Some(transitive) = relationships.get(&referenced_var).cloned() {
                    let entry = relationships.entry(ref_var).or_default();
                    let old_len = entry.len();

                    entry.extend(transitive);
                    if entry.len() != old_len {
                        modified = true;
                    }
                }
            }
        }

        if !modified {
            break;
        }
    }
}

/// Whether the type is directly a reference type.
fn is_ref(ty: &MIRTypeInner) -> bool {
    match ty {
        // Not references.
        MIRTypeInner::UnknownNumber
        | MIRTypeInner::NotConstructed
        | MIRTypeInner::I32
        | MIRTypeInner::U32
        | MIRTypeInner::Bool
        | MIRTypeInner::Unit
        | MIRTypeInner::String
        | MIRTypeInner::Char
        | MIRTypeInner::Named(_) => false,

        // Technically a reference, but no mutation.
        MIRTypeInner::FunctionPtr(_, _) => false,

        // Arrays are passed by reference.
        MIRTypeInner::Ref(_) | MIRTypeInner::Array(_) | MIRTypeInner::ArrayFixed(_, _) => true,
    }
}

/// Determines whether the type contains any references.
fn contains_ref(ty: &MIRTypeInner) -> bool {
    match ty {
        // Not references.
        MIRTypeInner::UnknownNumber
        | MIRTypeInner::NotConstructed
        | MIRTypeInner::I32
        | MIRTypeInner::U32
        | MIRTypeInner::Bool
        | MIRTypeInner::Unit
        | MIRTypeInner::Char
        | MIRTypeInner::String => false,

        // Technically a reference, but no mutation.
        MIRTypeInner::FunctionPtr(_, _) => false,

        MIRTypeInner::Named(_name) => todo!("Descend into structs"),

        // Arrays are passed by reference.
        MIRTypeInner::Ref(_) | MIRTypeInner::Array(_) | MIRTypeInner::ArrayFixed(_, _) => true,
    }
}
