//! Plan 3: line, circle and polygon as operator chains (ADR 0006). The
//! scenarios mirror the current core's vitest suites; the adapter runs those
//! verbatim, these are the core-level checks.

use cells_core::reference;
use cells_core::test_utils::load;
use cells_core::{Document, PointRequest, Request};

fn req(doc: &Document, name: &str, prop: &str, value: f64) -> Request {
    Request { cell: doc.cell(name, prop).unwrap(), value }
}

fn v(doc: &Document, name: &str, prop: &str) -> f64 {
    doc.value(name, prop).unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9 || (a.is_nan() && b.is_nan())
}

macro_rules! assert_close {
    ($a:expr, $b:expr) => {
        let (a, b) = ($a, $b);
        assert!(close(a, b), "expected {b}, got {a}");
    };
}

/// A whole-shape drag: the shape's points requested together (ADR 0006).
fn move_points(doc: &mut Document, name: &str, props: &[(&str, &str)], at: &[(f64, f64)]) {
    let pts: Vec<PointRequest> = props.iter().zip(at).map(|(&(px, py), &(x, y))| PointRequest { cells: [doc.cell(name, px).unwrap(), doc.cell(name, py).unwrap()], values: [x, y] }).collect();
    let t = doc.request_points(&pts);
    assert!(t.dropped.is_empty(), "dropped {:?}", t.dropped);
    assert_eq!(reference::check(doc), None);
}

fn move_point(doc: &mut Document, name: &str, x: f64, y: f64) {
    let t = doc.request(&[req(doc, name, "x", x), req(doc, name, "y", y)]);
    assert!(t.dropped.is_empty(), "dropped {:?}", t.dropped);
    assert_eq!(reference::check(doc), None);
}

// ---------------------------------------------------------------------------
// Circle
// ---------------------------------------------------------------------------

#[test]
fn circle_defaults_to_unit_circle_with_essential_center_and_radius() {
    let mut doc = load(r#"<graph><circle name="c"/></graph>"#).unwrap();
    assert_eq!((v(&doc, "c", "cx"), v(&doc, "c", "cy"), v(&doc, "c", "radius")), (0.0, 0.0, 1.0));
    doc.request(&[req(&doc, "c", "cx", 2.0), req(&doc, "c", "cy", 3.0)]);
    assert_eq!((v(&doc, "c", "cx"), v(&doc, "c", "cy")), (2.0, 3.0));
    // A negative radius clamps to zero (projection, ADR 0003).
    doc.request(&[req(&doc, "c", "radius", -4.0)]);
    assert_eq!(v(&doc, "c", "radius"), 0.0);
    assert_eq!(v(&doc, "c", "diameter"), 0.0);
}

#[test]
fn circle_with_center_point_and_bound_radius() {
    let mut doc = load(
        r#"<mathInput name="r" prefill="3"/>
           <graph><point name="p">(-3,5)</point><circle name="c" center="$p" radius="$r"/></graph>
           <point name="cc" extend="$c.center"/>"#,
    )
    .unwrap();
    assert_eq!((v(&doc, "c", "cx"), v(&doc, "c", "cy"), v(&doc, "c", "radius")), (-3.0, 5.0, 3.0));
    // Dragging the circle moves the defining point.
    doc.request(&[req(&doc, "c", "cx", 2.0), req(&doc, "c", "cy", 3.0)]);
    assert_eq!((v(&doc, "p", "x"), v(&doc, "p", "y")), (2.0, 3.0));
    // The radius request reaches the input; negative becomes zero there too.
    doc.request(&[req(&doc, "c", "radius", -4.0)]);
    assert_eq!(v(&doc, "r", "value"), 0.0);
    // A negative value typed into the input stays, the circle shows zero.
    doc.request(&[req(&doc, "r", "value", -3.0)]);
    assert_eq!(v(&doc, "r", "value"), -3.0);
    assert_eq!(v(&doc, "c", "radius"), 0.0);
    // Dragging the center copy moves the point.
    move_point(&mut doc, "cc", -6.0, -2.0);
    assert_eq!((v(&doc, "p", "x"), v(&doc, "p", "y")), (-6.0, -2.0));
}

#[test]
fn circle_center_and_through_point() {
    let mut doc = load(r#"<graph><point name="c">(0,0)</point><point name="p">(3,4)</point><circle name="circ" center="$c" through="$p"/></graph>"#).unwrap();
    assert_eq!(v(&doc, "circ", "radius"), 5.0);
    // Radius request moves the through point along its ray.
    doc.request(&[req(&doc, "circ", "radius", 10.0)]);
    assert_close!(v(&doc, "p", "x"), 6.0);
    assert_close!(v(&doc, "p", "y"), 8.0);
    // Center request translates both.
    doc.request(&[req(&doc, "circ", "cx", 1.0), req(&doc, "circ", "cy", 1.0)]);
    assert_close!(v(&doc, "c", "x"), 1.0);
    assert_close!(v(&doc, "p", "x"), 7.0);
    assert_close!(v(&doc, "p", "y"), 9.0);
    assert_close!(v(&doc, "circ", "radius"), 10.0);
    assert_eq!(reference::check(&doc), None);
}

#[test]
fn circle_through_three_points_drags_rigidly_and_scales_about_center() {
    let mut doc = load(
        r#"<graph>
             <point name="p1">(2,-3)</point><point name="p2">(3,4)</point><point name="p3">(-3,4)</point>
             <circle name="c" through="$p1 $p2 $p3"/>
           </graph>"#,
    )
    .unwrap();
    let (cx, cy, r) = (v(&doc, "c", "cx"), v(&doc, "c", "cy"), v(&doc, "c", "radius"));
    for (x, y) in [(2.0, -3.0), (3.0, 4.0), (-3.0, 4.0)] {
        assert_close!((x - cx).hypot(y - cy), r);
    }
    doc.request(&[req(&doc, "c", "cx", cx + 3.0), req(&doc, "c", "cy", cy + 4.0)]);
    assert_close!(v(&doc, "p1", "x"), 5.0);
    assert_close!(v(&doc, "p1", "y"), 1.0);
    assert_close!(v(&doc, "p3", "x"), 0.0);
    assert_close!(v(&doc, "c", "radius"), r);
    // Scaling the radius keeps every point on its ray from the center.
    doc.request(&[req(&doc, "c", "radius", 2.0 * r)]);
    assert_close!(v(&doc, "c", "radius"), 2.0 * r);
    assert_close!(v(&doc, "c", "cx"), cx + 3.0);
    assert_close!(v(&doc, "c", "cy"), cy + 4.0);
    assert_eq!(reference::check(&doc), None);
}

#[test]
fn circle_with_radius_and_two_points() {
    let mut doc = load(r#"<graph><point name="p1">(0,0)</point><point name="p2">(2,0)</point><circle name="c" radius="2" through="$p1 $p2"/></graph>"#).unwrap();
    // The current core's closed form puts the center on the left of p1 -> p2.
    assert_close!(v(&doc, "c", "cx"), 1.0);
    assert_close!(v(&doc, "c", "cy"), 3f64.sqrt());
    doc.request(&[req(&doc, "c", "cx", 11.0), req(&doc, "c", "cy", 10.0 + 3f64.sqrt())]);
    assert_close!(v(&doc, "p1", "x"), 10.0);
    assert_close!(v(&doc, "p1", "y"), 10.0);
    assert_close!(v(&doc, "p2", "x"), 12.0);
}

#[test]
fn circle_center_constrained_to_grid_carries_through_point() {
    // The current core corrects this after the update; here one inversion
    // with lookahead gives the same answer (ADR 0006).
    let mut doc = load(
        r#"<graph>
             <point name="c">(0,0)<constraints><constrainToGrid dx="3" dy="2"/></constraints></point>
             <point name="p">(1,0)</point>
             <circle name="circ" center="$c" through="$p"/>
           </graph>"#,
    )
    .unwrap();
    doc.request(&[req(&doc, "circ", "cx", 4.0), req(&doc, "circ", "cy", 2.7)]);
    assert_eq!((v(&doc, "c", "x"), v(&doc, "c", "y")), (3.0, 2.0));
    assert_close!(v(&doc, "p", "x"), 4.0);
    assert_close!(v(&doc, "p", "y"), 2.0);
    assert_close!(v(&doc, "circ", "radius"), 1.0);
}

#[test]
fn circle_through_three_points_one_constrained() {
    let mut doc = load(
        r#"<graph>
             <point name="p1">(0,0)<constraints><constrainToGrid dx="3" dy="2"/></constraints></point>
             <point name="p2">(4,0)</point><point name="p3">(0,4)</point>
             <circle name="c" through="$p1 $p2 $p3"/>
           </graph>"#,
    )
    .unwrap();
    let (cx, cy) = (v(&doc, "c", "cx"), v(&doc, "c", "cy"));
    doc.request(&[req(&doc, "c", "cx", cx + 1.0), req(&doc, "c", "cy", cy + 1.0)]);
    // p1 snapped to (0,2): the shift (-1, +1) is applied to every point.
    assert_eq!((v(&doc, "p1", "x"), v(&doc, "p1", "y")), (0.0, 2.0));
    assert_close!(v(&doc, "p2", "x"), 4.0);
    assert_close!(v(&doc, "p2", "y"), 2.0);
    assert_close!(v(&doc, "p3", "x"), 0.0);
    assert_close!(v(&doc, "p3", "y"), 6.0);
}

// ---------------------------------------------------------------------------
// Line
// ---------------------------------------------------------------------------

#[test]
fn line_through_two_points_and_its_point_list() {
    let mut doc = load(r#"<graph name="g"><point name="a">(1,2)</point><point name="b">(4,7)</point><line name="l" through="$a $b"/><pointList extend="$l.points" name="Ps"/></graph>"#).unwrap();
    assert_close!(v(&doc, "l", "slope"), 5.0 / 3.0);
    assert_close!(v(&doc, "l", "yintercept"), 2.0 - 5.0 / 3.0);
    assert_close!(v(&doc, "l", "xintercept"), 1.0 - 2.0 * 3.0 / 5.0);
    // Dragging one of the line's own points moves that defining point only.
    let ps2 = doc.resolve_path("g.Ps[2]").unwrap();
    let cells = doc.prop_cells(ps2, "coords").unwrap();
    doc.request(&[Request { cell: cells[0], value: 0.0 }, Request { cell: cells[1], value: 0.0 }]);
    assert_eq!((v(&doc, "b", "x"), v(&doc, "b", "y")), (0.0, 0.0));
    assert_eq!((v(&doc, "a", "x"), v(&doc, "a", "y")), (1.0, 2.0));
    // moveLine: both points requested together, both move.
    move_points(&mut doc, "l", &[("x1", "y1"), ("x2", "y2")], &[(5.0, 5.0), (7.0, 9.0)]);
    assert_eq!((v(&doc, "a", "x"), v(&doc, "b", "y")), (5.0, 9.0));
    assert_eq!(reference::check(&doc), None);
}

#[test]
fn line_through_point_with_slope() {
    let mut doc = load(r#"<mathInput name="m" prefill="1"/><graph><point name="a">(1,2)</point><line name="l" through="$a" slope="$m"/></graph>"#).unwrap();
    let s = 1.0 / 2f64.sqrt();
    assert_close!(v(&doc, "l", "x2"), 1.0 + s);
    assert_close!(v(&doc, "l", "y2"), 2.0 + s);
    assert_eq!(v(&doc, "l", "basedOnDirection"), 1.0);
    // Dragging the second point rewrites the slope and the distance.
    doc.request(&[req(&doc, "l", "x2", 1.0), req(&doc, "l", "y2", 7.0)]);
    assert_eq!(v(&doc, "m", "value"), f64::INFINITY);
    assert_eq!(v(&doc, "l", "x2"), 1.0);
    assert_close!(v(&doc, "l", "y2"), 7.0);
    doc.request(&[req(&doc, "l", "x2", -4.0), req(&doc, "l", "y2", 2.0)]);
    assert_close!(v(&doc, "m", "value"), 0.0);
    assert_close!(v(&doc, "l", "x2"), -4.0);
    // Changing the slope keeps the distance and the sign of dx.
    doc.request(&[req(&doc, "m", "value", 1.0)]);
    let (dx, dy) = (v(&doc, "l", "x2") - 1.0, v(&doc, "l", "y2") - 2.0);
    assert_close!(dx.hypot(dy), 5.0);
    assert!(dx < 0.0);
    assert_close!(dy / dx, 1.0);
    // Moving the first point translates the line.
    move_point(&mut doc, "a", 3.0, 3.0);
    assert_close!(v(&doc, "l", "x2") - 3.0, dx);
    assert_close!(v(&doc, "l", "y2") - 3.0, dy);
}

#[test]
fn line_from_equation_lowers_to_coefficient_cells() {
    let mut doc = load(r#"<graph><line name="l">5x-2y=3</line></graph>"#).unwrap();
    assert_eq!((v(&doc, "l", "coeffvar1"), v(&doc, "l", "coeffvar2"), v(&doc, "l", "coeff0")), (5.0, -2.0, -3.0));
    assert_close!(v(&doc, "l", "slope"), 2.5);
    assert_close!(v(&doc, "l", "xintercept"), 0.6);
    assert_close!(v(&doc, "l", "yintercept"), -1.5);
    let (x1, y1, x2, y2) = (v(&doc, "l", "x1"), v(&doc, "l", "y1"), v(&doc, "l", "x2"), v(&doc, "l", "y2"));
    assert_close!(5.0 * x1 - 2.0 * y1, 3.0);
    assert_close!(5.0 * x2 - 2.0 * y2, 3.0);
    // Translating the line keeps a and b and moves c.
    doc.request(&[req(&doc, "l", "x1", x1 + 1.0), req(&doc, "l", "y1", y1 + 2.0), req(&doc, "l", "x2", x2 + 1.0), req(&doc, "l", "y2", y2 + 2.0)]);
    assert_eq!((v(&doc, "l", "coeffvar1"), v(&doc, "l", "coeffvar2")), (5.0, -2.0));
    assert_close!(v(&doc, "l", "coeff0"), -3.0 - (5.0 * 1.0 - 2.0 * 2.0));
}

#[test]
fn line_from_dynamic_equation_inverts_into_the_inputs() {
    let mut doc = load(r#"<mathInput name="m" prefill="2"/><mathInput name="b" prefill="1"/><graph><line name="l">y = $m x + $b</line></graph>"#).unwrap();
    assert_close!(v(&doc, "l", "slope"), 2.0);
    assert_close!(v(&doc, "l", "yintercept"), 1.0);
    let (x1, y1, x2, y2) = (v(&doc, "l", "x1"), v(&doc, "l", "y1"), v(&doc, "l", "x2"), v(&doc, "l", "y2"));
    doc.request(&[req(&doc, "l", "x1", x1), req(&doc, "l", "y1", y1 + 3.0), req(&doc, "l", "x2", x2), req(&doc, "l", "y2", y2 + 3.0)]);
    assert_close!(v(&doc, "b", "value"), 4.0);
    assert_close!(v(&doc, "m", "value"), 2.0);
}

#[test]
fn line_through_two_points_one_constrained_translates_as_a_whole() {
    let mut doc = load(
        r#"<graph>
             <point name="a">(0,0)<constraints><constrainToGrid dx="2" dy="3"/></constraints></point>
             <point name="b">(5,5)</point>
             <line name="l" through="$a $b"/>
           </graph>"#,
    )
    .unwrap();
    // Request a translation by (0.7, 1.4): a snaps to (0, 0)... no, to the
    // nearest grid point of (0.7, 1.4), which is (0, 0); b then stays.
    move_points(&mut doc, "l", &[("x1", "y1"), ("x2", "y2")], &[(0.7, 1.4), (5.7, 6.4)]);
    assert_eq!((v(&doc, "a", "x"), v(&doc, "a", "y")), (0.0, 0.0));
    assert_close!(v(&doc, "b", "x"), 5.0);
    assert_close!(v(&doc, "b", "y"), 5.0);
    // A translation by (1.2, 1.9): a snaps to (2, 3), b follows by (2, 3).
    move_points(&mut doc, "l", &[("x1", "y1"), ("x2", "y2")], &[(1.2, 1.9), (6.2, 6.9)]);
    assert_eq!((v(&doc, "a", "x"), v(&doc, "a", "y")), (2.0, 3.0));
    assert_close!(v(&doc, "b", "x"), 7.0);
    assert_close!(v(&doc, "b", "y"), 8.0);
    assert_eq!(reference::check(&doc), None);
}

// ---------------------------------------------------------------------------
// Polygon
// ---------------------------------------------------------------------------

#[test]
fn rigid_polygon_rotates_about_its_centroid_when_one_vertex_is_dragged() {
    let mut doc = load(
        r#"<graph name="g1">
             <point name="p1">(3,7)</point><point name="p2">(-4,-1)</point><point name="p3">(8,2)</point><point name="p4">(-3,4)</point>
             <polygon vertices="$p1 $p2 $p3 $p4" name="pg" rigid/>
           </graph>
           <graph name="g2"><polygon extend="$g1.pg" name="pg"/><pointList extend="$pg.vertices" name="vs"/></graph>
           <graph extend="$g2" name="g3"/>"#,
    )
    .unwrap();
    assert_eq!(v(&doc, "g1.pg", "numVertices"), 4.0);
    let mut verts = [[3.0, 7.0], [-4.0, -1.0], [8.0, 2.0], [-3.0, 4.0]];
    let c = [verts.iter().map(|p| p[0]).sum::<f64>() / 4.0, verts.iter().map(|p| p[1]).sum::<f64>() / 4.0];
    // Rotate 90 degrees counterclockwise about the centroid, asking for half the length.
    let requested = [-0.5 * (verts[1][1] - c[1]) + c[0], 0.5 * (verts[1][0] - c[0]) + c[1]];
    for p in &mut verts {
        *p = [-(p[1] - c[1]) + c[0], p[0] - c[0] + c[1]];
    }
    doc.request(&[req(&doc, "g1.pg", "x2", requested[0]), req(&doc, "g1.pg", "y2", requested[1])]);
    for (i, p) in verts.iter().enumerate() {
        assert_close!(v(&doc, &format!("p{}", i + 1), "x"), p[0]);
        assert_close!(v(&doc, &format!("p{}", i + 1), "y"), p[1]);
    }
    // The copies see the same vertices through their aliases.
    let g3 = doc.resolve_path("g3.pg").unwrap();
    let cells = doc.prop_cells(g3, "x1").unwrap();
    assert_close!(doc.cells[cells[0] as usize], verts[0][0]);
    let vs2 = doc.resolve_path("g3.vs[2]").unwrap();
    let cells = doc.prop_cells(vs2, "coords").unwrap();
    assert_close!(doc.cells[cells[1] as usize], verts[1][1]);
    // Dragging every vertex of the copy translates by the smallest shift.
    let moved: Vec<Request> = verts
        .iter()
        .enumerate()
        .flat_map(|(i, p)| {
            let cx = doc.prop_cells(g3, &format!("x{}", i + 1)).unwrap()[0];
            let cy = doc.prop_cells(g3, &format!("y{}", i + 1)).unwrap()[0];
            [Request { cell: cx, value: p[0] + 3.0 + i as f64 }, Request { cell: cy, value: p[1] + 2.0 + 2.0 * i as f64 }]
        })
        .collect();
    doc.request(&moved);
    for (i, p) in verts.iter().enumerate() {
        assert_close!(v(&doc, &format!("p{}", i + 1), "x"), p[0] + 3.0);
        assert_close!(v(&doc, &format!("p{}", i + 1), "y"), p[1] + 2.0);
    }
    assert_eq!(reference::check(&doc), None);
}

// ---------------------------------------------------------------------------
// Names and copies
// ---------------------------------------------------------------------------

#[test]
fn names_resolve_through_containers_and_copies() {
    let doc = load(
        r#"<graph><point name="q">(1,2)</point></graph>
           <graph name="g"><point name="p">(3,4)</point></graph>
           <graph extend="$g" name="h"/>"#,
    )
    .unwrap();
    assert_eq!(v(&doc, "q", "x"), 1.0);
    // `p` exists in g and in its copy h; the original wins a bare lookup.
    let gp = doc.resolve_path("g.p").unwrap();
    assert_eq!(doc.resolve_path("p"), Some(gp));
    assert_eq!(doc.cells[doc.prop_cells(gp, "x").unwrap()[0] as usize], 3.0);
    let hp = doc.resolve_path("h.p").unwrap();
    assert_eq!(doc.prop_cells(hp, "x"), doc.prop_cells(gp, "x"));
}

#[test]
fn circle_through_triangle_vertices() {
    let doc = load(r#"<graph><triangle name="t" vertices="(1,2) (3,5) (-5,2)"/><circle name="c" through="$t.vertex1 $t.vertex2 $t.vertex3"/></graph>"#).unwrap();
    assert_eq!((v(&doc, "t", "x1"), v(&doc, "t", "y2"), v(&doc, "t", "x3")), (1.0, 5.0, -5.0));
    let (cx, cy, r) = (v(&doc, "c", "cx"), v(&doc, "c", "cy"), v(&doc, "c", "radius"));
    assert!(r > 0.0, "radius {r}, center ({cx}, {cy})");
    assert_close!((1.0 - cx).hypot(2.0 - cy), r);
    assert_close!((-5.0 - cx).hypot(2.0 - cy), r);
}

#[test]
fn fixed_number_and_inscribed_triangle_drag() {
    let mut doc = load(
        r#"<number hide name="fixedZero" fixed>0</number>
           <graph>
             <triangle name="t" vertices="(1,2) (3,5) (-5,2)"/>
             <circle name="c" through="$t.vertex1 $t.vertex2 $t.vertex3"/>
             <point name="x">($c.center.x, $fixedZero)</point>
           </graph>"#,
    )
    .unwrap();
    let z = doc.cell("fixedZero", "value").unwrap();
    let tick = doc.request(&[Request { cell: z, value: 5.0 }]);
    assert_eq!(tick.dropped.len(), 1, "a fixed number must reject writes");
    let (cx, cy, r) = (v(&doc, "c", "cx"), v(&doc, "c", "cy"), v(&doc, "c", "radius"));
    // The y request lands on the fixed zero and is dropped; x moves the circle.
    let tick = doc.request(&[req(&doc, "x", "x", -3.0), req(&doc, "x", "y", 0.0)]);
    assert_eq!(tick.dropped.len(), 1);
    assert_eq!(reference::check(&doc), None);
    assert_close!(v(&doc, "c", "cx"), -3.0);
    assert_close!(v(&doc, "c", "cy"), cy);
    assert_close!(v(&doc, "c", "radius"), r);
    assert_close!(v(&doc, "t", "x1"), 1.0 - 3.0 - cx);
}

#[test]
fn self_referencing_shapes_are_not_cycles() {
    // A parallelogram's fourth vertex from its first three, as the current
    // core's tests write it: legal, since nothing couples the free vertices.
    let mut doc = load(
        r#"<graph>
             <polygon name="pg" vertices="(1,2) (3,4) (-5,6) ($pg.vertex3[1]+$pg.vertex2[1]-$pg.vertex1[1], $pg.vertex3[2]+$pg.vertex2[2]-$pg.vertex1[2])"/>
           </graph>"#,
    )
    .unwrap();
    assert_eq!((v(&doc, "pg", "x4"), v(&doc, "pg", "y4")), (-3.0, 8.0));
    // Dragging vertex 1 moves the derived fourth vertex with it.
    doc.request(&[req(&doc, "pg", "x1", 0.0), req(&doc, "pg", "y1", 0.0)]);
    assert_eq!((v(&doc, "pg", "x4"), v(&doc, "pg", "y4")), (-2.0, 10.0));
    let doc2 = load(r#"<graph><point name="A">(1,2)</point><line name="l" through="$A ($l.point1.y, $l.point1.x)"/></graph>"#).unwrap();
    assert_eq!((v(&doc2, "l", "x2"), v(&doc2, "l", "y2")), (2.0, 1.0));
    let _ = doc;
}

#[test]
fn a_whole_line_drag_against_a_grid_keeps_the_line_parallel() {
    // Same scenario as above but through the public entry point a renderer
    // uses: the two points requested as one group.
    let mut doc = load(
        r#"<graph>
             <point name="a">(0,0)<constraints><constrainToGrid dx="2" dy="3"/></constraints></point>
             <point name="b">(5,5)</point>
             <line name="l" through="$a $b"/>
           </graph>"#,
    )
    .unwrap();
    let pts =
        [PointRequest { cells: [doc.cell("a", "x").unwrap(), doc.cell("a", "y").unwrap()], values: [1.2, 1.9] }, PointRequest { cells: [doc.cell("b", "x").unwrap(), doc.cell("b", "y").unwrap()], values: [6.2, 6.9] }];
    doc.request_points(&pts);
    assert_eq!((v(&doc, "a", "x"), v(&doc, "a", "y")), (2.0, 3.0));
    assert_close!(v(&doc, "b", "x"), 7.0);
    assert_close!(v(&doc, "b", "y"), 8.0);
    // The same two requests as plain scalars move only what they name.
    doc.request(&[req(&doc, "a", "x", 3.1), req(&doc, "a", "y", 5.9), req(&doc, "b", "x", 8.1), req(&doc, "b", "y", 10.9)]);
    assert_eq!((v(&doc, "a", "x"), v(&doc, "a", "y")), (4.0, 6.0));
    assert_close!(v(&doc, "b", "x"), 8.1);
}
