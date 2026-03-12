use crate::mir::interpreter::InterpreterData;
use crate::mir::{
    FunctionOverloads, MIRContext, MIRFunction, MIRFunctionArgs, MIRFunctionKey, MIRFunctionType,
    MIRType, MIRTypeInner, MIRVariable,
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

    for (name, args, variadic, ret, func) in get_funcs() {
        let args_ty = MIRFunctionArgs {
            args: args.to_vec(),
            variadic,
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
            .or_insert(FunctionOverloads::new(ctx.target));

        // This will shadow existing functions without an error.
        // This isn't desirable, but it's very complicated to do so otherwise,
        // since we do want to shadow functions within target blocks, while still handling
        // statics within those same target blocks.
        func_overloads.remove_conflicts(&args_ty);
        func_overloads.push(args_ty, key);

        funcs.funcs.insert(key, func);
    }

    Ok(funcs)
}

macro_rules! function {
    // Variadic with one or more args
    ($name:literal ( $($arg:expr),+ , ... ) -> $ret:expr => $func:ident) => {
        ($name, vec![$($arg),+], true, $ret, $func)
    };
    // Variadic with no args
    ($name:literal ( ... ) -> $ret:expr => $func:ident) => {
        ($name, vec![], true, $ret, $func)
    };
    // Non-variadic
    ($name:literal ( $($arg:expr),* ) -> $ret:expr => $func:ident) => {
        ($name, vec![$($arg),*], false, $ret, $func)
    };
}

/// Name, args, variadic, return type, function.
fn get_funcs() -> Vec<(
    &'static str,
    Vec<MIRTypeInner<'static>>,
    bool,
    MIRTypeInner<'static>,
    NativeFunction,
)> {
    vec![
        // string
        function! { "string" (MIRTypeInner::U32) -> MIRTypeInner::String => string },
        function! { "string" (MIRTypeInner::I32) -> MIRTypeInner::String => string },
        function! { "string" (MIRTypeInner::Bool) -> MIRTypeInner::String => string },
        function! { "string" (MIRTypeInner::String) -> MIRTypeInner::String => string },
        function! { "string" (MIRTypeInner::Array(Box::new(MIRTypeInner::Char))) -> MIRTypeInner::String => string },
        // stringInto
        function! { "stringInto" (MIRTypeInner::U32, MIRTypeInner::Ref(Box::new(MIRTypeInner::String))) -> MIRTypeInner::String => string_into },
        function! { "stringInto" (MIRTypeInner::I32, MIRTypeInner::Ref(Box::new(MIRTypeInner::String))) -> MIRTypeInner::String => string_into },
        function! { "stringInto" (MIRTypeInner::Bool, MIRTypeInner::Ref(Box::new(MIRTypeInner::String))) -> MIRTypeInner::String => string_into },
        function! { "stringInto" (MIRTypeInner::String, MIRTypeInner::Ref(Box::new(MIRTypeInner::String))) -> MIRTypeInner::String => string_into },
        function! { "stringInto" (MIRTypeInner::Array(Box::new(MIRTypeInner::Char)), MIRTypeInner::Ref(Box::new(MIRTypeInner::String))) -> MIRTypeInner::String => string_into },
        // stringInto (&[char] output)
        function! { "stringInto" (MIRTypeInner::U32, MIRTypeInner::Ref(Box::new(MIRTypeInner::Array(Box::new(MIRTypeInner::Char))))) -> MIRTypeInner::String => string_into },
        function! { "stringInto" (MIRTypeInner::I32, MIRTypeInner::Ref(Box::new(MIRTypeInner::Array(Box::new(MIRTypeInner::Char))))) -> MIRTypeInner::String => string_into },
        function! { "stringInto" (MIRTypeInner::Bool, MIRTypeInner::Ref(Box::new(MIRTypeInner::Array(Box::new(MIRTypeInner::Char))))) -> MIRTypeInner::String => string_into },
        function! { "stringInto" (MIRTypeInner::String, MIRTypeInner::Ref(Box::new(MIRTypeInner::Array(Box::new(MIRTypeInner::Char))))) -> MIRTypeInner::String => string_into },
        function! { "stringInto" (MIRTypeInner::Array(Box::new(MIRTypeInner::Char)), MIRTypeInner::Ref(Box::new(MIRTypeInner::Array(Box::new(MIRTypeInner::Char))))) -> MIRTypeInner::String => string_into },
    ]
}

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

fn string_into<'a>(args: &[(InterpreterData<'a>, MIRTypeInner<'a>)]) -> InterpreterData<'a> {
    let val = string(args);

    let InterpreterData::Ref(inner, _) = &args[1].0 else {
        unreachable!();
    };
    *inner.borrow_mut() = Some(val);

    InterpreterData::Unit(())
}
