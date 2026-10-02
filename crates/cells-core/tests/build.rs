use cells_core::components::ComponentKind;
use cells_core::reference;
use cells_core::test_utils::load;
use cells_core::{Child, Document, Error};

fn load_ok(src: &str) -> Document {
    let doc = load(src).unwrap_or_else(|e| panic!("load failed: {e}\n{src}"));
    assert_eq!(reference::check(&doc), None, "scheduled values disagree with reference evaluator");
    doc
}

#[test]
fn three_cells_for_three_coordinates() {
    let doc = load_ok(r#"<point name="p1" x="1" y="2"/><point name="p2" x="5" y="$p1.y"/>"#);
    assert_eq!(doc.cells.len(), 3);
    assert_eq!(doc.n_essential, 3);
    assert_eq!(doc.cell("p1", "y"), doc.cell("p2", "y"));
    assert_ne!(doc.cell("p1", "x"), doc.cell("p2", "x"));
    assert_eq!(doc.value("p2", "y"), Some(2.0));
    assert_eq!(doc.value("p2", "x"), Some(5.0));
}

#[test]
fn bare_component_reference_shares_every_cell() {
    let doc = load_ok(r#"<graph name="g"><point name="p1" x="1" y="2"/>$p1</graph>"#);
    // four graph bounds plus two coordinates; the copy adds none
    assert_eq!(doc.cells.len(), 6);
    let g = doc.component("g").unwrap();
    let kids: Vec<_> = doc.components[g as usize]
        .children
        .iter()
        .filter_map(|c| if let Child::Component(i) = c { Some(*i) } else { None })
        .collect();
    assert_eq!(kids.len(), 2);
    let copy = &doc.components[kids[1] as usize];
    assert_eq!(copy.kind, ComponentKind::Point);
    assert_eq!(copy.name, None);
    assert_eq!(doc.prop_cells(kids[1], "coords"), doc.prop_cells(kids[0], "coords"));
}

#[test]
fn extend_with_override_shares_only_unoverridden_props() {
    let doc = load_ok(r#"<point name="p1" x="1" y="2"/><point name="p2" extend="$p1" y="3"/>"#);
    assert_eq!(doc.cells.len(), 3);
    assert_eq!(doc.cell("p1", "x"), doc.cell("p2", "x"));
    assert_ne!(doc.cell("p1", "y"), doc.cell("p2", "y"));
    assert_eq!(doc.value("p2", "y"), Some(3.0));
}

#[test]
fn coords_attribute_aliases_both_cells() {
    let doc = load_ok(r#"<point name="p1" x="1" y="2"/><point name="q" coords="$p1"/><point name="r" coords="$p1.coords"/>"#);
    assert_eq!(doc.cells.len(), 2);
    assert_eq!(doc.cell("q", "x"), doc.cell("p1", "x"));
    assert_eq!(doc.cell("r", "y"), doc.cell("p1", "y"));
}

#[test]
fn number_input_feeds_point_through_default_prop() {
    let mut doc = load_ok(r#"<numberInput name="n" value="4"/><point name="p" x="$n"/>"#);
    assert_eq!(doc.cells.len(), 2);
    assert_eq!(doc.cell("n", "value"), doc.cell("p", "x"));
    assert_eq!(doc.value("p", "x"), Some(4.0));
    let c = doc.cell("n", "value").unwrap();
    doc.set_essential(c, 7.5);
    assert_eq!(doc.value("p", "x"), Some(7.5));
}

#[test]
fn number_children_literal_reference_and_bare_text_reference() {
    let doc = load_ok(r#"<point name="p1" x="1" y="2"/><number name="a">3.5</number><number name="b">$p1.x</number> $p1.y <number name="c"/>"#);
    assert_eq!(doc.value("a", "value"), Some(3.5));
    assert_eq!(doc.cell("b", "value"), doc.cell("p1", "x"));
    assert!(doc.value("c", "value").unwrap().is_nan());
    // The bare $p1.y became an anonymous number aliasing p1.y.
    let root = &doc.components[doc.root as usize];
    let anon = root
        .children
        .iter()
        .filter_map(|c| if let Child::Component(i) = c { Some(&doc.components[*i as usize]) } else { None })
        .find(|c| c.name.is_none() && c.kind == ComponentKind::Number)
        .expect("anonymous number");
    assert_eq!(anon.props[0].cells, vec![doc.cell("p1", "y").unwrap()]);
    // p1.x, p1.y, a, c
    assert_eq!(doc.cells.len(), 4);
}

#[test]
fn operators_compute_and_propagate() {
    let mut doc = load_ok(
        r#"<numberInput name="a" value="2"/><numberInput name="b" value="5"/>
           <op name="s" kind="add" args="$a $b"/>
           <op name="t" kind="scale" k="10" args="$s"/>
           <op name="u" kind="clamp" lo="0" hi="50" args="$t"/>
           <op name="v" kind="lerp" t="0.5" args="$a $b"/>
           <op name="w" kind="sub" args="$b $a"/><op name="m" kind="mul" args="$a $b"/>
           <op name="n" kind="negate" args="$a"/><op name="o" kind="offset" k="1" args="$a"/>
           <point name="p" x="$t" y="$u"/>"#,
    );
    assert_eq!(doc.n_essential, 2);
    assert_eq!(doc.cells.len(), 10);
    assert_eq!(doc.value("s", "value"), Some(7.0));
    assert_eq!(doc.value("t", "value"), Some(70.0));
    assert_eq!(doc.value("u", "value"), Some(50.0));
    assert_eq!(doc.value("v", "value"), Some(3.5));
    assert_eq!(doc.value("w", "value"), Some(3.0));
    assert_eq!(doc.value("m", "value"), Some(10.0));
    assert_eq!(doc.value("n", "value"), Some(-2.0));
    assert_eq!(doc.value("o", "value"), Some(3.0));
    assert_eq!(doc.cell("p", "x"), doc.cell("t", "value"));
    let a = doc.cell("a", "value").unwrap();
    doc.set_essential(a, 1.0);
    assert_eq!(doc.value("p", "x"), Some(60.0));
    assert_eq!(doc.value("p", "y"), Some(50.0));
    assert_eq!(reference::check(&doc), None);
}

#[test]
fn forward_references_schedule_correctly() {
    let doc = load_ok(
        r#"<op name="c" kind="negate" args="$b"/><op name="b" kind="negate" args="$a"/>
           <op name="a" kind="offset" k="1" args="$n"/><numberInput name="n" value="1"/>"#,
    );
    assert_eq!(doc.value("c", "value"), Some(2.0));
    assert_eq!(doc.program.len(), 3);
}

#[test]
fn graph_defaults_and_tree() {
    let doc = load_ok(r#"<graph name="g" xmin="-5"><point name="p"/></graph>"#);
    assert_eq!(doc.value("g", "xmin"), Some(-5.0));
    assert_eq!(doc.value("g", "xmax"), Some(10.0));
    assert_eq!(doc.value("p", "x"), Some(0.0));
    let p = doc.component("p").unwrap();
    assert_eq!(doc.components[p as usize].parent, doc.component("g"));
    assert_eq!(doc.cells.len(), 6);
}

#[test]
fn alias_cycle_is_rejected() {
    let err = load(r#"<point name="a" x="$b.x"/><point name="b" x="$a.x"/>"#).unwrap_err();
    assert!(matches!(err, Error::Cycle(_)), "{err}");
}

#[test]
fn operator_cycle_is_rejected() {
    let err = load(r#"<op name="a" kind="negate" args="$b"/><op name="b" kind="negate" args="$a"/>"#).unwrap_err();
    assert!(matches!(err, Error::Cycle(_)), "{err}");
}

#[test]
fn error_cases() {
    assert!(matches!(load(r#"<point x="$nope"/>"#).unwrap_err(), Error::UnknownName(_)));
    assert!(matches!(load(r#"<point name="a"/><point name="a"/>"#).unwrap_err(), Error::DuplicateName(_)));
    assert!(matches!(load(r#"<point x="1+2"/>"#).unwrap_err(), Error::BadValue { .. }));
    assert!(matches!(load(r#"<point name="p"/><point x="$p"/>"#).unwrap_err(), Error::ArityMismatch { .. }));
    assert!(matches!(load(r#"<point name="p"/><number extend="$p"/>"#).unwrap_err(), Error::ExtendKindMismatch { .. }));
    assert!(matches!(load(r#"<point name="p"/><op kind="add" args="$p.x"/>"#).unwrap_err(), Error::OpArity { .. }));
    assert!(matches!(load(r#"<point name="p"/><op kind="scale" args="$p.x"/>"#).unwrap_err(), Error::MissingParam { .. }));
    assert!(matches!(load(r#"<point name="p"/>$p.x.y"#).unwrap_err(), Error::PathTooDeep(_)));
    assert!(matches!(load(r#"<graph name="g"/>$g"#).unwrap_err(), Error::UncopyableKind(_)));
    assert!(matches!(load(r#"<text>hi</text>"#).unwrap_err(), Error::UnsupportedTag(_)));
}

#[test]
fn load_timed_reports_stages() {
    let json = cells_core::test_utils::dast_json(r#"<numberInput name="a" value="2"/><op kind="negate" args="$a"/>"#);
    let (doc, t) = Document::load_timed(&json).unwrap();
    assert_eq!(doc.cells.len(), 2);
    assert!(t.deserialize.as_nanos() > 0);
}
