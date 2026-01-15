use crate::mir::{MIRContext, MIRStatement};
use std::collections::HashMap;

/// Stores the index that a label points to, so it can be easily jumped to by a goto.
/// This MUST be run after the MIR is fully flattened and after the last modification to
/// the order of statements in every function's MIR (order/insertion/deletion, etc.).
/// Realistically, this means this pass should be the last compile pass.
pub fn label_to_index(ctx: &mut MIRContext) {
    for function in ctx.program.functions.values_mut() {
        let mut label_mapper = HashMap::new();

        for (idx, statement) in function.body.iter().enumerate() {
            if let MIRStatement::Label { name, .. } = statement {
                label_mapper.insert(name.clone(), idx);
            }
        }

        for statement in function.body.iter_mut() {
            match statement {
                MIRStatement::Goto { name, index, .. }
                | MIRStatement::GotoNotEqual { name, index, .. } => {
                    let Some(label_idx) = label_mapper.get(name) else {
                        panic!("Goto statement without label!");
                    };

                    *index = Some(*label_idx);
                }
                _ => {}
            }
        }
    }
}
