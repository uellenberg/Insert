use crate::mir::scope::StatementExplorer;
use crate::mir::{MIRContext, MIRStatement};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// Helper to map labels to new names.
/// This should not be given as input
/// an already mapped label.
struct LabelMapper<'a> {
    /// The function's name.
    fn_name: Cow<'a, str>,

    /// A counter for the next label ID.
    label_idx: AtomicU32,

    /// A map of already mapped labels.
    label_map: HashMap<Cow<'a, str>, Cow<'a, str>>,
}

impl<'a> LabelMapper<'a> {
    fn new(fn_name: Cow<'a, str>) -> Self {
        Self {
            fn_name,
            label_idx: AtomicU32::new(0),
            label_map: HashMap::new(),
        }
    }

    /// Maps a label to a new name.
    /// This should not be given an already
    /// mapped label name as input.
    fn map(&mut self, label: Cow<'a, str>) -> Cow<'a, str> {
        if let Some(name) = self.label_map.get(&label) {
            return name.clone();
        }

        let name: Cow<'a, str> = Cow::Owned(format!(
            "lbl_{}${}",
            &self.fn_name,
            self.label_idx.fetch_add(1, Ordering::Relaxed)
        ));

        self.label_map.insert(label, name.clone());

        name
    }
}

/// Makes every label have a unique name.
pub fn rename_labels(ctx: &mut MIRContext) {
    for function in ctx.program.functions.values_mut() {
        let mut label_mapper = LabelMapper::new(function.name.clone());

        if !<StatementExplorer>::rewrite_block(
            &mut function.body,
            &mut move |statement, _scope, block| {
                let statement = match statement {
                    MIRStatement::Label { name, span } => MIRStatement::Label {
                        name: label_mapper.map(name),
                        span,
                    },
                    MIRStatement::Goto { name, span } => MIRStatement::Goto {
                        name: label_mapper.map(name),
                        span,
                    },
                    MIRStatement::GotoNotEqual {
                        name,
                        condition,
                        span,
                    } => MIRStatement::GotoNotEqual {
                        name: label_mapper.map(name),
                        condition,
                        span,
                    },
                    // We're only dealing with if statements.
                    _ => statement,
                };

                block.push(statement);

                true
            },
            &mut |_, _| true,
            &|_, _, _| true,
        ) {
            panic!("rename_labels returned false!");
        }
    }
}
