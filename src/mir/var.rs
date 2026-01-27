use crate::mir::expr::{explore_expr_mut, find_exprs_mut};
use crate::mir::scope::StatementExplorer;
use crate::mir::{MIRContext, MIRExpression, MIRExpressionInner, MIRStatement, MIRVariable};

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
                    var: MIRVariable { var_idx, name, .. },
                    arg: true,
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
                find_exprs_mut(statement, &mut |expr| {
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
