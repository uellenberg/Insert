use crate::mir::interpreter::InterpreterData;
use crate::mir::{
    MIRContext, MIRFunction, MIRFunctionArgs, MIRFunctionKey, MIRFunctionType, MIRType,
    MIRTypeInner, MIRVariable,
};
use crate::parser::span::Span;
use slotmap::SparseSecondaryMap;
use std::cell::RefCell;
use std::rc::Rc;

pub type NativeFunction =
    for<'a> fn(&[(InterpreterData<'a>, MIRTypeInner<'a>)]) -> InterpreterData<'a>;

#[derive(Default, Debug)]
pub struct NativeFunctions {
    pub funcs: SparseSecondaryMap<MIRFunctionKey, NativeFunction>,
}

pub fn register_natives(ctx: &mut MIRContext) -> Result<NativeFunctions, ()> {
    let mut funcs = NativeFunctions::default();

    for (name, args, variadic, ret, func) in FUNCS {
        let args_ty = MIRFunctionArgs {
            args: args.to_vec(),
            variadic: *variadic,
        };
        let func_data = MIRFunction {
            name: (*name).into(),
            // This isn't quite the same thing as an extern function, but
            // the semantics are essentially the same.
            // The only difference is that we can't save a ref to this function, but
            // refs aren't taken from the interpreter anyway.
            fn_type: MIRFunctionType::Extern,
            ret_ty: MIRType {
                ty: ret.clone(),
                span: None,
            },
            body: vec![],
            args_ty: args_ty.clone(),
            args: args
                .iter()
                .map(|arg| MIRVariable {
                    name: "".into(),
                    span: Span::empty(),
                    ty: MIRType {
                        ty: arg.clone(),
                        span: None,
                    },
                    var_idx: None,
                    arg: true,
                })
                .collect(),
            extern_import: None,
            span: Span::empty(),
        };

        let key = ctx.program.functions.insert(func_data);

        let func_overloads = ctx
            .program
            .function_names
            .entry((*name).into())
            .or_default();

        // This will shadow existing functions without an error.
        // This isn't desirable, but it's very complicated to do so otherwise,
        // since we do want to shadow functions within target blocks, while still handling
        // statics within those same target blocks.
        func_overloads.remove_conflicts(&args_ty);
        func_overloads.push(args_ty, key);

        funcs.funcs.insert(key, *func);
    }

    Ok(funcs)
}

macro_rules! function {
    // Variadic with one or more args
    ($name:literal ( $($arg:expr),+ , ... ) -> $ret:expr => $func:ident) => {
        ($name, &[$($arg),+], true, $ret, $func)
    };
    // Variadic with no args
    ($name:literal ( ... ) -> $ret:expr => $func:ident) => {
        ($name, &[], true, $ret, $func)
    };
    // Non-variadic
    ($name:literal ( $($arg:expr),* ) -> $ret:expr => $func:ident) => {
        ($name, &[$($arg),*], false, $ret, $func)
    };
}

/// Name, args, variadic, return type, function.
const FUNCS: &[(&str, &[MIRTypeInner], bool, MIRTypeInner, NativeFunction)] = &[
    function! { "string" (MIRTypeInner::U32) -> MIRTypeInner::String => string },
    function! { "string" (MIRTypeInner::I32) -> MIRTypeInner::String => string },
    function! { "string" (MIRTypeInner::Bool) -> MIRTypeInner::String => string },
    function! { "string" (MIRTypeInner::String) -> MIRTypeInner::String => string },
];

fn string<'a>(args: &[(InterpreterData<'a>, MIRTypeInner<'a>)]) -> InterpreterData<'a> {
    let val = match &args[0] {
        (InterpreterData::U32(v), _) => format!("{v}").into(),
        (InterpreterData::I32(v), _) => format!("{v}").into(),
        (InterpreterData::Bool(v), _) => format!("{v}").into(),
        (InterpreterData::String(s), _) => s.borrow().clone(),
        _ => unreachable!(),
    };

    InterpreterData::String(Rc::new(RefCell::new(val)))
}
