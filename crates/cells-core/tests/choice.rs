//! Choices (plan 6, ADR 0009): `<conditionalContent>` under both
//! mechanisms, the branch interface, conditions, and `<select>` picks.

use cells_core::reference;
use cells_core::test_utils::{dast_json, load};
use cells_core::{Document, Error, Request};

const MECHANISMS: [&str; 2] = ["built", "rebuild"];

/// `src` with every `<conditionalContent` forced to one mechanism.
fn with_mechanism(src: &str, m: &str) -> String {
    src.replace("<conditionalContent", &format!("<conditionalContent _mechanism=\"{m}\""))
}

fn set(doc: &mut Document, name: &str, prop: &str, value: f64) -> cells_core::Tick {
    let cell = doc.cell(name, prop).unwrap_or_else(|| panic!("no {name}.{prop}"));
    doc.request(&[Request { cell, value }])
}

fn val(doc: &Document, name: &str, prop: &str) -> f64 {
    doc.value(name, prop).unwrap_or_else(|| panic!("no {name}.{prop}"))
}

/// The value of `$path` as a number, through a `<number>` the document
/// defines for the purpose.
fn num(doc: &Document, probe: &str) -> f64 {
    val(doc, probe, "value")
}

const SIGN: &str = r#"
<numberInput name="n" value="5"/>
<conditionalContent name="cc">
  <case condition="$n > 0"><number name="x">1</number> <numberInput name="w" value="10"/></case>
  <case condition="$n < 0"><number name="x">-1</number> <numberInput name="w" value="20"/></case>
  <else><number name="x">0</number> <numberInput name="w" value="30"/></else>
</conditionalContent>
<number name="sx">$cc.x</number>
<number name="sw">$cc.w</number>
"#;

#[test]
fn interface_names_follow_the_active_case_under_both_mechanisms() {
    for m in MECHANISMS {
        let mut doc = load(&with_mechanism(SIGN, m)).unwrap();
        assert_eq!(reference::check(&doc), None, "{m}");
        assert_eq!(val(&doc, "cc", "choice"), 1.0, "{m}");
        assert_eq!(num(&doc, "sx"), 1.0, "{m}");
        assert_eq!(num(&doc, "sw"), 10.0, "{m}");
        let tick = set(&mut doc, "n", "value", -3.0);
        assert_eq!(tick.rebuilt, m == "rebuild", "{m}");
        assert_eq!(val(&doc, "cc", "choice"), 2.0, "{m}");
        assert_eq!(num(&doc, "sx"), -1.0, "{m}");
        assert_eq!(num(&doc, "sw"), 20.0, "{m}");
        set(&mut doc, "n", "value", 0.0);
        assert_eq!(num(&doc, "sx"), 0.0, "{m}");
        // NaN: no comparison holds, so the else.
        set(&mut doc, "n", "value", f64::NAN);
        assert_eq!(num(&doc, "sx"), 0.0, "{m}");
        assert_eq!(reference::check(&doc), None, "{m}");
    }
}

#[test]
fn a_request_moves_the_active_branch_and_returning_restores_state() {
    for m in MECHANISMS {
        let mut doc = load(&with_mechanism(SIGN, m)).unwrap();
        // Write through the interface: only case 1's input changes.
        set(&mut doc, "sw", "value", 11.0);
        assert_eq!(num(&doc, "sw"), 11.0, "{m}");
        set(&mut doc, "n", "value", -1.0);
        assert_eq!(num(&doc, "sw"), 20.0, "{m}");
        set(&mut doc, "sw", "value", 21.0);
        set(&mut doc, "n", "value", 2.0);
        assert_eq!(num(&doc, "sw"), 11.0, "{m}: case 1 kept what was typed");
        set(&mut doc, "n", "value", -2.0);
        assert_eq!(num(&doc, "sw"), 21.0, "{m}: case 2 kept what was typed");
    }
}

#[test]
fn a_request_cannot_flip_a_branch() {
    let mut doc = load(SIGN).unwrap();
    let tick = set(&mut doc, "cc", "choice", 3.0);
    assert_eq!(tick.dropped.len(), 1);
    assert_eq!(val(&doc, "cc", "choice"), 1.0);
}

#[test]
fn built_cases_report_which_is_active() {
    let doc = load(&with_mechanism(SIGN, "built")).unwrap();
    let cc = doc.component("cc").unwrap();
    let active: Vec<f64> = doc
        .children(cc)
        .filter_map(|ch| match ch {
            cells_core::Child::Component(c) => Some(doc.cells[doc.comp_cells(c)[0] as usize]),
            _ => None,
        })
        .collect();
    assert_eq!(active, vec![1.0, 0.0, 0.0]);
    // A test path finds the active case's component.
    let x = doc.resolve_path("cc.x").unwrap();
    assert_eq!(doc.cells[doc.prop_cells(x, "value").unwrap()[0] as usize], 1.0);
}

fn load_err(src: &str) -> Error {
    match load(src) {
        Ok(_) => panic!("expected an error"),
        Err(e) => e,
    }
}

#[test]
fn a_name_whose_kind_depends_on_the_branch_is_not_in_the_interface() {
    let e = load_err(
        r#"
<numberInput name="ni"/>
<conditionalContent name="cc">
  <case condition="$ni = 1"><math name="x">a + 2 b</math></case>
  <else><text name="x">Hi there</text></else>
</conditionalContent>
<math>$cc.x</math>"#,
    );
    assert!(matches!(&e, Error::NotInInterface { reason, .. } if reason.contains("<math> in case 1 but a <text> in case 2")), "{e}");
}

#[test]
fn a_name_missing_from_a_branch_or_without_an_else_is_not_in_the_interface() {
    let e = load_err(
        r#"
<numberInput name="n"/>
<conditionalContent name="cc">
  <case condition="$n > 0"><number name="x">1</number></case>
  <else><number name="y">2</number></else>
</conditionalContent>
<number>$cc.x</number>"#,
    );
    assert!(matches!(&e, Error::NotInInterface { reason, .. } if reason.contains("case 2 has no 'x'")), "{e}");
    let e = load_err(
        r#"
<numberInput name="n"/>
<conditionalContent name="cc" condition="$n > 0"><number name="x">1</number></conditionalContent>
<number>$cc.x</number>"#,
    );
    assert!(matches!(&e, Error::NotInInterface { reason, .. } if reason.contains("no <else>")), "{e}");
    // A bare name inside a branch is private.
    let e = load_err(
        r#"
<numberInput name="n"/>
<conditionalContent name="cc"><case condition="$n > 0"><number name="x">1</number></case><else><number name="x">2</number></else></conditionalContent>
<number>$x</number>"#,
    );
    assert!(matches!(e, Error::UnknownName(_)), "{e}");
}

#[test]
fn conditions_parse_connectives_parentheses_and_entities() {
    let src = r#"
<numberInput name="n" value="0"/>
<booleanInput name="b"/>
<conditionalContent name="pos" condition="$n > 0"><number name="k">1</number></conditionalContent>
<conditionalContent name="cc">
  <case condition="not ($n>0 or $n<0 or $n=0)"><number name="r">1</number></case>
  <case condition="$n &gt;= 2 and !$b"><number name="r">2</number></case>
  <case condition="($n + 1) * 2 = 4 || $b"><number name="r">3</number></case>
  <case condition="$n != 0 && true"><number name="r">4</number></case>
  <else><number name="r">5</number></else>
</conditionalContent>
<number name="out">$cc.r</number>
"#;
    for m in MECHANISMS {
        let mut doc = load(&with_mechanism(src, m)).unwrap();
        let at = |doc: &mut Document, n: f64, b: f64| {
            set(doc, "n", "value", n);
            set(doc, "b", "value", b);
            num(doc, "out")
        };
        assert_eq!(at(&mut doc, 0.0, 0.0), 5.0, "{m}");
        assert_eq!(at(&mut doc, f64::NAN, 0.0), 1.0, "{m}");
        assert_eq!(at(&mut doc, 3.0, 0.0), 2.0, "{m}");
        assert_eq!(at(&mut doc, 1.0, 0.0), 3.0, "{m}");
        assert_eq!(at(&mut doc, 3.0, 1.0), 3.0, "{m}");
        assert_eq!(at(&mut doc, -1.0, 0.0), 4.0, "{m}");
        assert_eq!(val(&doc, "pos", "choice"), 0.0, "{m}");
    }
}

#[test]
fn a_condition_may_read_another_choices_interface() {
    let src = r#"
<numberInput name="n" value="1"/>
<conditionalContent name="cc1">
  <case condition="$n > 0"><number name="sign">1</number></case>
  <else><number name="sign">-1</number></else>
</conditionalContent>
<conditionalContent name="cc2">
  <case condition="$cc1.sign > 0"><number name="v">100</number></case>
  <else><number name="v">200</number></else>
</conditionalContent>
<number name="out">$cc2.v</number>
"#;
    for m in MECHANISMS {
        let mut doc = load(&with_mechanism(src, m)).unwrap();
        assert_eq!(num(&doc, "out"), 100.0, "{m}");
        set(&mut doc, "n", "value", -1.0);
        assert_eq!(num(&doc, "out"), 200.0, "{m}");
        assert_eq!(reference::check(&doc), None, "{m}");
    }
}

#[test]
fn a_condition_reading_its_own_interface_is_a_cycle() {
    let e = load_err(
        r#"
<conditionalContent name="cc">
  <case condition="$cc.x > 0"><number name="x">1</number></case>
  <else><number name="x">2</number></else>
</conditionalContent>"#,
    );
    assert!(matches!(e, Error::Cycle(_)), "{e}");
}

#[test]
fn texts_choose_like_numbers() {
    let src = r#"
<booleanInput name="b"/>
<conditionalContent name="cc">
  <case condition="$b"><text name="t">dog</text></case>
  <else><text name="t">cat</text></else>
</conditionalContent>
<text name="copy">$cc.t</text>
"#;
    for m in MECHANISMS {
        let mut doc = load(&with_mechanism(src, m)).unwrap();
        let s = |doc: &Document| doc.strings.get(val(doc, "copy", "value") as u32).trim().to_string();
        assert_eq!(s(&doc), "cat", "{m}");
        set(&mut doc, "b", "value", 1.0);
        assert_eq!(s(&doc), "dog", "{m}");
    }
}

#[test]
fn an_interface_math_can_be_symbolic() {
    let src = r#"
<booleanInput name="b"/>
<conditionalContent name="cc">
  <case condition="$b"><math name="m">x^2</math></case>
  <else><math name="m">x+1</math></else>
</conditionalContent>
<math name="copy" expand>2 $cc.m</math>
"#;
    for m in MECHANISMS {
        let mut doc = load(&with_mechanism(src, m)).unwrap();
        let cell = doc.cell("copy", "expr").unwrap();
        assert_eq!(doc.math_text(cell).replace(" ", ""), "2x+2", "{m}");
        set(&mut doc, "b", "value", 1.0);
        let cell = doc.cell("copy", "expr").unwrap();
        assert_eq!(doc.math_text(cell).replace(" ", ""), "2x^2", "{m}");
    }
}

const SELECT: &str = r#"
<select name="s" numToSelect="2">
  <option><number name="v">1</number></option>
  <option><number name="v">2</number></option>
  <option><number name="v">3</number></option>
  <option><number name="v">4</number></option>
</select>
<number name="a">$s[1].v</number>
<number name="b">$s[2].v</number>
"#;

fn load_seeded(src: &str, seed: u64) -> Document {
    Document::from_bytes_seeded(dast_json(src).as_bytes(), seed).unwrap()
}

#[test]
fn a_select_draws_from_the_seed_without_replacement() {
    let mut seen = std::collections::HashSet::new();
    for seed in 0..40 {
        let doc = load_seeded(SELECT, seed);
        let (a, b) = (num(&doc, "a"), num(&doc, "b"));
        assert_ne!(a, b, "seed {seed}");
        assert!((1.0..=4.0).contains(&a) && (1.0..=4.0).contains(&b));
        // Unchosen options are never built.
        assert_eq!(doc.children(doc.component("s").unwrap()).count(), 2);
        // The same seed draws the same way.
        let again = load_seeded(SELECT, seed);
        assert_eq!((num(&again, "a"), num(&again, "b")), (a, b));
        seen.insert((a as i64, b as i64));
    }
    assert!(seen.len() > 6, "40 seeds gave only {} distinct draws", seen.len());
}

#[test]
fn a_select_inside_a_repeat_draws_per_iteration_and_keeps_its_draw_across_rebuilds() {
    let src = r#"
<numberInput name="n" value="6"/>
<repeatForSequence name="r" length="$n">
  <select name="s"><option><number name="v">1</number></option><option><number name="v">2</number></option><option><number name="v">3</number></option><option><number name="v">4</number></option><option><number name="v">5</number></option></select>
  <number name="c">$s.v</number>
</repeatForSequence>
"#;
    let mut doc = load_seeded(src, 7);
    let draws = |doc: &Document, k: usize| -> Vec<f64> {
        (1..=k).map(|i| doc.cells[doc.prop_cells(doc.resolve_path(&format!("r[{i}].c")).unwrap(), "value").unwrap()[0] as usize]).collect()
    };
    let before = draws(&doc, 6);
    assert!(before.iter().any(|&v| v != before[0]), "every iteration drew {before:?}");
    set(&mut doc, "n", "value", 8.0);
    assert_eq!(&draws(&doc, 8)[..6], &before[..]);
}

#[test]
fn select_references_by_position_are_banned_and_bare_names_need_one_pick() {
    let e = load_err(&format!("{SELECT}<number>$s[1][1]</number>"));
    assert!(matches!(e, Error::Banned(_)), "{e}");
    let e = load_err(&format!("{SELECT}<number>$s.v</number>"));
    assert!(matches!(e, Error::Banned(_)), "{e}");
    let e = load_err(
        r#"<select name="s" numToSelect="$n"><option><number>1</number></option></select><numberInput name="n"/>"#,
    );
    assert!(matches!(e, Error::Banned(_)), "{e}");
    let e = load_err(
        r#"<numberInput name="n"/><conditionalContent name="cc" condition="$n>0"><number>1</number></conditionalContent><conditionalContent extend="$cc"/>"#,
    );
    assert!(matches!(e, Error::Banned(_)), "{e}");
}

#[test]
fn select_string_sugar_picks_a_math() {
    let doc = load_seeded(r#"<select name="s">x y z</select><math name="m">$s</math>"#, 3);
    let cell = doc.cell("m", "expr").unwrap();
    assert!(["x", "y", "z"].contains(&doc.math_text(cell).as_str()));
}

#[test]
fn group_rendered_is_a_single_case() {
    let src = r#"
<booleanInput name="b"/>
<group name="g" rendered="$b"><number name="k">7</number></group>
"#;
    for m in MECHANISMS {
        let mut doc = load(&src.replace("<group", &format!("<group _mechanism=\"{m}\""))).unwrap();
        assert_eq!(val(&doc, "g", "choice"), 0.0, "{m}");
        set(&mut doc, "b", "value", 1.0);
        assert_eq!(val(&doc, "g", "choice"), 1.0, "{m}");
        assert!(doc.resolve_path("g.k").is_some(), "{m}");
    }
}

#[test]
fn an_option_math_copies_an_input_outside_the_select() {
    let src = r#"<mathInput prefill="a" name="x"/><select name="s" withReplacement numToSelect="3"><option><math name="v">$x</math></option></select>"#;
    let doc = load_seeded(src, 1);
    let v = doc.resolve_path("s[2].v").unwrap();
    let e = doc.prop_cells(v, "expr").unwrap()[0];
    assert_eq!(doc.math_text(e), "a");
}
