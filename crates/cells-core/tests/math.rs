//! `<math>`: numeric expressions lower to operators at build time; symbolic
//! ones are math cells holding engine handles, instantiated and evaluated by
//! symbolic instructions (ADR 0008).

mod common;

use cells_core::reference;
use cells_core::test_utils::load;
use cells_core::{Document, Op};
use common::req;

fn has_evaluate(doc: &Document) -> bool {
    doc.program.instrs.iter().any(|i| matches!(i.op, Op::Sym(..)))
}

#[test]
fn numeric_math_lowers_to_operators_and_inverts_through_them() {
    let mut doc = load(r#"<numberInput name="a" value="2"/><math name="m">3$a + 2</math>"#).unwrap();
    assert_eq!(doc.value("m", "value"), Some(8.0));
    assert!(!has_evaluate(&doc), "a numeric math is plain operators");
    // Scale then Offset: two instructions, no fixed literal cells.
    assert_eq!(doc.program.len(), 2);
    assert_eq!(doc.n_fixed, 3, "only the (NaN) expression handle and the document's two credit cells are fixed");
    // Dragging the math's value inverts through the lowered chain.
    doc.request(&[req(&doc, "m", "value", 14.0)]);
    assert_eq!(doc.value("a", "value"), Some(4.0));
    assert_eq!(reference::check(&doc), None);
}

#[test]
fn number_children_accept_math_text() {
    let doc = load(r#"<numberInput name="a" value="3"/><number name="n">2$a^2 - (1 + $a)/2</number><number name="k">4 * pi</number>"#).unwrap();
    assert_eq!(doc.value("n", "value"), Some(16.0));
    assert!((doc.value("k", "value").unwrap() - 4.0 * std::f64::consts::PI).abs() < 1e-12);
    assert!(!has_evaluate(&doc));
}

#[test]
fn symbolic_math_is_a_handle_and_evaluates_to_nan() {
    let doc = load(r#"<numberInput name="a" value="2"/><math name="f">x^2 + $a</math><number name="n">$f</number>"#).unwrap();
    assert!(doc.value("f", "value").unwrap().is_nan());
    assert!(doc.value("n", "value").unwrap().is_nan());
    assert!(has_evaluate(&doc));
    let handle = doc.value("f", "expr").unwrap();
    assert!(handle.fract() == 0.0 && handle >= 0.0);
    // The handle is derived from the template and `a`: no inverse.
    let mut doc = doc;
    let tick = doc.request(&[req(&doc, "f", "expr", 0.0)]);
    assert_eq!(tick.dropped.len(), 1);
    assert_eq!(doc.math_text(doc.cell("f", "expr").unwrap()), "x^2 + 2");
    doc.request(&[req(&doc, "a", "value", 5.0)]);
    assert_eq!(doc.math_text(doc.cell("f", "expr").unwrap()), "x^2 + 5");
}

#[test]
fn evaluate_at_feeds_a_symbolic_math_into_a_numeric_cell() {
    let mut doc = load(
        r#"<numberInput name="a" value="2"/><numberInput name="t" value="3"/>
           <math name="f">x^2 + $a</math>
           <evaluate name="e" function="$f" input="$t"/>
           <graph><point name="p" x="$t" y="$e"/></graph>"#,
    )
    .unwrap();
    assert_eq!(doc.value("e", "value"), Some(11.0));
    // Both the symbolic expression's cell leaf and the input are live.
    doc.request(&[req(&doc, "t", "value", 4.0)]);
    assert_eq!(doc.value("e", "value"), Some(18.0));
    doc.request(&[req(&doc, "a", "value", 1.0)]);
    assert_eq!(doc.value("e", "value"), Some(17.0));
    // No symbolic inverse: dragging the point's y is dropped, x still moves.
    let tick = doc.request(&[req(&doc, "p", "y", 5.0), req(&doc, "p", "x", 2.0)]);
    assert_eq!(tick.dropped.len(), 1);
    assert_eq!(doc.value("t", "value"), Some(2.0));
    assert_eq!(doc.value("e", "value"), Some(5.0));
    assert_eq!(reference::check(&doc), None);
}

#[test]
fn math_of_math_lowers_through_and_a_symbolic_leaf_stays_symbolic() {
    let doc = load(r#"<numberInput name="a" value="2"/><math name="m">$a + 1</math><math name="n">2$m</math><math name="g">$m + y</math>"#).unwrap();
    assert_eq!(doc.value("n", "value"), Some(6.0));
    assert!(doc.value("g", "value").unwrap().is_nan());
    // `$m` inside g is a numeric leaf: the lowered m's value, not its tree.
    assert_eq!(doc.math_text(doc.cell("g", "expr").unwrap()), "3 + y");
    assert!(doc.value("m", "expr").unwrap().is_nan(), "a numeric math is not a math cell");
}

#[test]
fn math_parse_errors_are_reported() {
    let err = load(r#"<math name="m">3 +</math>"#).unwrap_err();
    assert!(matches!(err, cells_core::Error::BadMath { .. }), "{err}");
    let err = load(r#"<math name="m">(1 + 2</math>"#).unwrap_err();
    assert!(matches!(err, cells_core::Error::BadMath { .. }), "{err}");
}
