use crate::mir::expr::{explore_expr, find_exprs};
use crate::mir::scope::StatementExplorer;
use crate::mir::{MIRContext, MIRExpression, MIRExpressionInner, MIRFunctionType, MIRStatement};
use crate::parser::span::{Span, eprintln_span};

/// Gives an error if a marker/binding is used in an invalid location.
/// An invalid location is any location where the marker would get
/// duplicated.
///
/// Returns true if it succeeded (i.e., no errors).
pub fn check_markers(ctx: &MIRContext) -> bool {
    for function in ctx.program.functions.values() {
        // Inline functions will be duplicated, and helper
        // functions might not be included.
        if matches!(
            function.fn_type,
            MIRFunctionType::Inline | MIRFunctionType::Helper
        ) && ensure_no_markers_block(&function.body)
        {
            return false;
        }
    }

    for constant in ctx.program.constants.values() {
        // Constants are always duplicated at their use site.
        if ensure_no_markers_expression(&constant.value) {
            return false;
        }
    }

    true
}

fn invalid_marker_error(span: &Span) {
    eprintln_span!(
        Some(span.clone()),
        "Invalid marker location (marker might be duplicated, won't always exist, or got removed with dead code optimization)"
    );
}

/// Assuming this block is an invalid spot for markers, checks
/// if there are any and shows an error.
///
/// Returns true if it succeeded (i.e., no errors).
pub fn ensure_no_markers_block(block: &[MIRStatement]) -> bool {
    <StatementExplorer>::explore_block(
        block,
        &mut |stmt, _| {
            if let MIRStatement::MarkerStatement { span, .. } = stmt {
                invalid_marker_error(span);
                return false;
            }

            find_exprs(stmt, &mut |expr, _| ensure_no_markers_expression(expr))
        },
        &mut |_, _| true,
        &|_, _| true,
    )
}

/// Assuming this expression is an invalid spot for markers, checks
/// if there are any and shows an error.
///
/// Returns true if it succeeded (i.e., no errors).
pub fn ensure_no_markers_expression(expr: &MIRExpression) -> bool {
    explore_expr(expr, &mut |expr| {
        if matches!(&expr.inner, MIRExpressionInner::Binding(..)) {
            invalid_marker_error(&expr.span);
            return false;
        }

        true
    })
}
