//! Number, text, op, math, evaluate, function, derivative and answer:
//! types that hold a value, numeric or symbolic. Function, derivative and
//! answer are `symbolic`: the builder plans them as math cells
//! (`plan_symbolic`).

use super::define::*;
use super::{ComponentTypeInfo, PropDef};
use crate::program::SymKind;

/// `<number>`.
pub(super) const NUMBER: ComponentTypeInfo = info(&["number"], NUMBER_PROPS)
    .copyable()
    .default_prop("value");

const NUMBER_PROPS: &[PropDef] = &props([children("value")]);

/// `<text>` with literal content: `value` is a fixed cell holding the
/// string id of its text (ADR 0009). A cell's meaning is a property of
/// the operators around it, so a text value never reaches a numeric one.
pub(super) const TEXT: ComponentTypeInfo =
    info(&["text"], TEXT_PROPS).copyable().default_prop("value");

const TEXT_PROPS: &[PropDef] = &props([children("value")]);

pub mod text {
    use super::*;
    pub const VALUE: usize = at(TEXT_PROPS, "value");
}

/// Prototype-only tag that applies a numeric operator to referenced cells.
pub(super) const OP: ComponentTypeInfo = info(&["op"], OP_PROPS).copyable().default_prop("value");

const OP_PROPS: &[PropDef] = &props([derived("value")]);

/// `<math>`: lowered to a numeric chain in `value` when its expression
/// is all numbers and numeric cells (`expr` is then NaN). Otherwise a
/// math cell: `expr` holds an engine handle, instantiated from the
/// template whenever a leaf changes (then simplified or expanded if the
/// attribute says so), and `value` evaluates it (NaN with free symbols).
pub(super) const MATH: ComponentTypeInfo =
    info(&["math"], MATH_PROPS).copyable().default_prop("value");

// Both of a math's props are set by the builder from its children.
const MATH_PROPS: &[PropDef] = &props([children("expr"), children("value")]);

pub mod math {
    use super::*;
    pub const EXPR: usize = at(MATH_PROPS, "expr");
    pub const VALUE: usize = at(MATH_PROPS, "value");
}

/// `<evaluate function="$m" input="$a"/>`: the math's expression with
/// its free symbol set to the input.
pub(super) const EVALUATE: ComponentTypeInfo = info(&["evaluate"], EVALUATE_PROPS)
    .copyable()
    .default_prop("value");

const EVALUATE_PROPS: &[PropDef] = &props([
    attr("function", f64::NAN).ref_prop("expr"),
    attr("input", f64::NAN),
    computed(
        "value",
        OpSpec::Sym(SymKind::EvalAt),
        &["function", "input"],
    ),
]);

/// `<function>`: a math cell `expr` (variable `x`) and, as a curve, the
/// `SAMPLES` cells from `samples` on, filled by a `Sample` instruction
/// over the enclosing graph's x-range.
pub(super) const FUNCTION: ComponentTypeInfo = info(&["function"], CURVE_PROPS)
    .copyable()
    .symbolic()
    .default_prop("expr");

/// `<derivative>$f</derivative>`: d/dx of a function or math, sampled
/// like a function.
pub(super) const DERIVATIVE: ComponentTypeInfo = info(&["derivative"], CURVE_PROPS)
    .copyable()
    .symbolic()
    .default_prop("expr");

// Planned by the builder (`plan_symbolic`); `samples` is the first of
// `SAMPLES` consecutive cells.
const CURVE_PROPS: &[PropDef] = &props([
    children("expr"),
    planned("xmin"),
    planned("xmax"),
    planned("samples"),
]);

/// `<answer response="$mi">correct</answer>`: `submitted` is an
/// essential math cell that a submit request sets to the response;
/// `credit` compares it with `correct` (`symbolicEquality`: as written).
pub(super) const ANSWER: ComponentTypeInfo = info(&["answer"], ANSWER_PROPS)
    .copyable()
    .symbolic()
    .default_prop("credit");

const ANSWER_PROPS: &[PropDef] = &props([
    attr("response", f64::NAN).ref_prop("expr"),
    children("correct"),
    attr("submitted", f64::NAN),
    planned("credit"),
    attr("weight", 1.0),
]);

pub mod answer {
    use super::*;
    pub const RESPONSE: usize = at(ANSWER_PROPS, "response");
    pub const SUBMITTED: usize = at(ANSWER_PROPS, "submitted");
    pub const CREDIT: usize = at(ANSWER_PROPS, "credit");
    pub const WEIGHT: usize = at(ANSWER_PROPS, "weight");
}
