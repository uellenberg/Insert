use crate::mir::scope::StatementExplorer;
use crate::mir::{MIRContext, MIRStatement};

/// Adds variable drops at the end of
/// every scope in every function.
/// This must run after var_idx is assigned.
pub fn drop_at_scope_end(ctx: &mut MIRContext<'_>) -> bool {
    for function in ctx.program.functions.values_mut() {
        if !<StatementExplorer>::rewrite_block(
            &mut function.body,
            &mut |statement, _scope, block| {
                block.push(statement);
                true
            },
            &mut |scope, block| {
                // Drops occur in reverse order
                // from how they're recorded.
                for var in scope.auto_drops() {
                    block.push(MIRStatement::DropVariable(
                        var.name.clone(),
                        var.var_idx.unwrap(),
                        var.span.clone(),
                    ));
                }

                true
            },
            &|_, _, _| true,
        ) {
            return false;
        }
    }

    true
}
