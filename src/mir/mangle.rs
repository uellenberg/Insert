use crate::mir::expr::{explore_expr_mut, find_exprs_mut};
use crate::mir::scope::StatementExplorer;
use crate::mir::{
    FunctionOverloads, MIRContext, MIRDeclarationKey, MIRExpressionInner, MIRFnSource,
    MIRFunctionArgs, MIRFunctionKey, MIRStatement,
};
use crate::util::name;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

/// Renames all statics, functions, and local variables to the shortest possible names.
/// This must be used after var_idx has been assigned and after all
/// renames/removals/insertions are complete.
pub fn mangle_names(ctx: &mut MIRContext) {
    // Names which have been claimed.
    // No need to use Cow, since the majority of strings
    // will be dynamic (other than nomangle).
    let mut used: HashSet<String> = HashSet::new();
    // The next name to use (converted to a string with name_num_to_str).
    let mut name_num: usize = 0;

    // Old static name -> new static name.
    let mut static_map: HashMap<String, String> = HashMap::new();
    // Old function key (can't use name due to overloads) -> new function name.
    let mut func_map: HashMap<MIRFunctionKey, String> = HashMap::new();

    let exported_keys = ctx.program.decls.iter().cloned().collect::<HashSet<_>>();

    // Generate new mappings.
    // We won't change things yet, since it'll be easier to
    // update the references first.
    for (key, static_data) in &ctx.program.statics {
        if !exported_keys.contains(&MIRDeclarationKey::Static(key)) {
            continue;
        }

        let new_name = name::next_name(&mut name_num, &used);
        used.insert(new_name.clone());
        static_map.insert(static_data.name.to_string(), new_name);
    }

    for (key, func_data) in &ctx.program.functions {
        if !exported_keys.contains(&MIRDeclarationKey::Function(key)) {
            continue;
        }

        let new_name = if Some(func_data.name.as_ref()) == ctx.target.main() {
            // If this is a main function, we can't mangle it.
            // The easiest thing to do is just remap the name to itself.
            func_data.name.to_string()
        } else {
            name::next_name(&mut name_num, &used)
        };

        used.insert(new_name.clone());
        func_map.insert(key, new_name);
    }

    for (key, func_data) in &mut ctx.program.functions {
        if !exported_keys.contains(&MIRDeclarationKey::Function(key)) {
            continue;
        }

        // Var names can be reused between functions, so clone them.
        let mut name_num = name_num;
        let mut used = used.clone();

        // Collect new names for every local variable (by var_idx).
        let mut var_map: HashMap<usize, String> = HashMap::new();
        <StatementExplorer>::explore_block(
            &func_data.body,
            &mut |statement, _| {
                if let MIRStatement::CreateVariable { var, .. } = statement {
                    let new_name = name::next_name(&mut name_num, &used);
                    used.insert(new_name.clone());
                    var_map.insert(var.var_idx.unwrap(), new_name);
                }

                true
            },
            &|_, _| true,
            &|_, _| true,
        );

        // Arg phantom variables will be renamed alongside
        // the rest of the variables.
        for arg in &mut func_data.args {
            arg.name = var_map[&arg.var_idx.unwrap()].clone().into();
        }

        rename_statements(
            &mut func_data.body,
            &static_map,
            &func_map,
            &ctx.program.function_names,
            &var_map,
        );
    }

    // Now that the code has been modified, we can rename the declarations themselves.

    // We're renaming everything, so we can just rebuild the lookup tables instead of adding/removing.
    ctx.program.static_names.clear();
    for (key, static_data) in &mut ctx.program.statics {
        if !exported_keys.contains(&MIRDeclarationKey::Static(key)) {
            continue;
        }

        static_data.name = static_map[&*static_data.name].clone().into();
        ctx.program
            .static_names
            .insert(static_data.name.clone(), key);
    }

    ctx.program.function_names.clear();
    for (key, func_data) in &mut ctx.program.functions {
        if !exported_keys.contains(&MIRDeclarationKey::Function(key)) {
            continue;
        }

        func_data.name = func_map[&key].clone().into();
        ctx.program
            .function_names
            .entry(func_data.name.clone())
            .or_insert(FunctionOverloads::new(ctx.target))
            .push(func_data.args_ty.clone(), key);
    }
}

/// Renames every variable/function reference in the given statements
/// according to the provided rename maps.
fn rename_statements<'a>(
    statements: &mut [MIRStatement<'a>],
    static_map: &HashMap<String, String>,
    fn_map: &HashMap<MIRFunctionKey, String>,
    function_names: &HashMap<Cow<'a, str>, FunctionOverloads<'a>>,
    var_map: &HashMap<usize, String>,
) {
    <StatementExplorer>::explore_block_mut(
        statements,
        &mut |statement, _| {
            // Some statements have names hard-coded into them,
            // and need to be resolved specially.
            match statement {
                MIRStatement::CreateVariable { var, .. } => {
                    var.name = var_map[&var.var_idx.unwrap()].clone().into();
                }
                MIRStatement::DropVariable(name, var_idx, _) => {
                    *name = var_map[var_idx].clone().into();
                }
                MIRStatement::FunctionCall(fn_call) => {
                    // This only handles direct, but indirect is renamed with
                    // the rest of the expressions.
                    rename_fn_source(
                        &mut fn_call.source,
                        &fn_call.args_ty,
                        fn_map,
                        function_names,
                    );
                }
                _ => {}
            }

            find_exprs_mut(statement, &mut |expr, _| {
                explore_expr_mut(expr, &mut |expr| {
                    match &mut expr.inner {
                        MIRExpressionInner::Variable(name, var_idx) => {
                            if let Some(var_idx) = var_idx {
                                *name = var_map[var_idx].clone().into();
                            } else if let Some(new_name) = static_map.get(name.as_ref()) {
                                *name = new_name.clone().into();
                            }

                            // TODO: Handle function pointers?
                        }
                        MIRExpressionInner::FunctionCall(fn_call) => {
                            rename_fn_source(
                                &mut fn_call.source,
                                &fn_call.args_ty,
                                fn_map,
                                function_names,
                            );
                        }
                        _ => {}
                    }
                    true
                })
            })
        },
        &|_, _| true,
        &mut |_, _| true,
    );
}

/// Remaps direct function calls to the correct function name.
fn rename_fn_source<'a>(
    source: &mut MIRFnSource<'a>,
    args_ty: &Option<MIRFunctionArgs<'a>>,
    fn_map: &HashMap<MIRFunctionKey, String>,
    function_names: &HashMap<Cow<'a, str>, FunctionOverloads<'a>>,
) {
    if let MIRFnSource::Direct(name, _) = source
        // If the name doesn't exist, it means it's external.
        && let Some(new_name) = args_ty.as_ref().and_then(|args_ty| {
            function_names
                .get(name.as_ref())
                .and_then(|overloads| overloads.find_compatible(&args_ty.args))
                .and_then(|fn_key| fn_map.get(&fn_key).cloned())
        })
    {
        *name = new_name.into();
    }
}
