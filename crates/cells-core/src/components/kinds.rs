//! The data: every kind's prop table and its row in `KINDS`.

use super::{
    ArrayProp, CONTAINER, COPYABLE, ComponentKind, INTERNAL, KindInfo, MAX_VERTICES, PLANNED,
    PropDef, PropFrom, SYMBOLIC, prop,
};
use crate::program::{OpSpec, SymKind};

const fn attr(name: &'static str, default: f64) -> PropDef {
    PropDef {
        name,
        default,
        from: PropFrom::Attribute,
        attr: None,
        bind: None,
        ref_prop: None,
    }
}

const fn computed(name: &'static str, op: OpSpec, args: &'static [u8]) -> PropDef {
    PropDef {
        name,
        default: f64::NAN,
        from: PropFrom::Computed { op, args },
        attr: None,
        bind: None,
        ref_prop: None,
    }
}

/// A prop whose source the builder plans from the element's specification.
const fn planned(name: &'static str) -> PropDef {
    PropDef {
        name,
        default: f64::NAN,
        from: PropFrom::Planned,
        attr: None,
        bind: None,
        ref_prop: None,
    }
}

/// `<circle>`. `cx`, `cy`, `radius` are the numerical center and radius;
/// how they are produced depends on which of `center`, `radius` and
/// `through` the author gave (see `plan_circle` in `build/compile/geometry/circle.rs`).
pub(super) const CIRCLE_PROPS: &[PropDef] = &[
    /* 0 */ planned("cx"),
    /* 1 */ planned("cy"),
    /* 2 */ planned("radius"),
    /* 3 */ computed("diameter", OpSpec::Scale { k: 2.0 }, &[2]),
    /* 4 */
    computed(
        "circumference",
        OpSpec::Scale {
            k: std::f64::consts::TAU,
        },
        &[2],
    ),
    /* 5 */ planned("area"),
    // The center as a *reference*: the prescribed center when there is one
    // (so a point extending `$c.center` drags that point alone, as in the
    // current core), else the derived center.
    /* 6 */
    planned("centerX"),
    /* 7 */ planned("centerY"),
    /* 8 */ planned("throughX1"),
    /* 9 */ planned("throughY1"),
    /* 10 */ planned("throughX2"),
    /* 11 */ planned("throughY2"),
    /* 12 */ planned("throughX3"),
    /* 13 */ planned("throughY3"),
    /* 14 */ planned("numThroughPoints"),
];

/// `<line>` and `<lineSegment>`. The first four props are the shape's own
/// points (ADR 0006). `basedOnDirection` is 1 when the second point is
/// derived from a slope or direction, so a renderer drags only the first.
pub(super) const LINE_PROPS: &[PropDef] = &[
    /* 0 */ planned("x1"),
    /* 1 */ planned("y1"),
    /* 2 */ planned("x2"),
    /* 3 */ planned("y2"),
    /* 4 */ planned("slope"),
    /* 5 */ planned("xintercept"),
    /* 6 */ planned("yintercept"),
    /* 7 */ planned("coeffvar1"),
    /* 8 */ planned("coeffvar2"),
    /* 9 */ planned("coeff0"),
    /* 10 */ planned("basedOnDirection"),
];

pub(super) const LINE_SEGMENT_PROPS: &[PropDef] = &[
    /* 0 */ planned("x1"),
    /* 1 */ planned("y1"),
    /* 2 */ planned("x2"),
    /* 3 */ planned("y2"),
];

/// `vertexN` of a polygon, as prop name pairs.
pub(super) const VERTEX_PARTS: [[&str; 2]; MAX_VERTICES] = [
    ["x1", "y1"],
    ["x2", "y2"],
    ["x3", "y3"],
    ["x4", "y4"],
    ["x5", "y5"],
    ["x6", "y6"],
    ["x7", "y7"],
    ["x8", "y8"],
    ["x9", "y9"],
    ["x10", "y10"],
    ["x11", "y11"],
    ["x12", "y12"],
    ["x13", "y13"],
    ["x14", "y14"],
    ["x15", "y15"],
    ["x16", "y16"],
];

/// `<polygon>`: `numVertices` then `MAX_VERTICES` coordinate pairs
/// (`x1`, `y1`, ...); unused pairs hold the shared NaN.
pub(super) const POLYGON_PROPS: &[PropDef] = {
    const fn build() -> [PropDef; 1 + 2 * MAX_VERTICES] {
        let mut out = [planned(""); 1 + 2 * MAX_VERTICES];
        out[0] = planned("numVertices");
        let mut i = 0;
        while i < MAX_VERTICES {
            out[1 + 2 * i] = planned(VERTEX_PARTS[i][0]);
            out[2 + 2 * i] = planned(VERTEX_PARTS[i][1]);
            i += 1;
        }
        out
    }
    &build()
};

/// `<slider>` in numeric mode. Prop indices are referenced by the computed
/// chain below, so the order matters. The chain reproduces the current core's
/// `numItems`, `index` and `value` definitions:
///
/// ```text
/// maxIndex = floor((to - from) / step + 1e-10)
/// index    = min(max(round((pre - from) / step), 0), maxIndex), or 0 if NaN
/// value    = from + index * step
/// ```
///
/// `preliminaryValue` is the slider's own essential cell unless `bindValueTo`
/// names another component, in which case it aliases that component's value.
pub(super) const SLIDER_PROPS: &[PropDef] = &[
    /* 0 */ attr("from", 0.0),
    /* 1 */ attr("to", 10.0),
    /* 2 */ attr("step", 1.0),
    /* 3 */
    PropDef {
        name: "preliminaryValue",
        default: 0.0,
        from: PropFrom::Attribute,
        attr: Some("initialValue"),
        bind: Some("bindValueTo"),
        ref_prop: None,
    },
    /* 4 */ computed("span", OpSpec::Sub, &[1, 0]),
    /* 5 */ computed("spanSteps", OpSpec::Div, &[4, 2]),
    /* 6 */ computed("spanStepsEps", OpSpec::Offset { k: 1e-10 }, &[5]),
    /* 7 */ computed("maxIndex", OpSpec::Floor, &[6]),
    /* 8 */ computed("offset", OpSpec::Sub, &[3, 0]),
    /* 9 */ computed("rawIndex", OpSpec::Div, &[8, 2]),
    /* 10 */ computed("roundedIndex", OpSpec::Round, &[9]),
    /* 11 */
    computed(
        "nonNegIndex",
        OpSpec::Clamp {
            lo: 0.0,
            hi: f64::INFINITY,
        },
        &[10],
    ),
    /* 12 */ computed("clampedIndex", OpSpec::Min, &[11, 7]),
    // A non-finite stored value is index 0, so the slider shows `from`,
    // as in the current core.
    /* 13 */
    computed("index", OpSpec::NanTo { k: 0.0 }, &[12]),
    /* 14 */ computed("scaled", OpSpec::Mul, &[13, 2]),
    /* 15 */ computed("value", OpSpec::Add, &[14, 0]),
];

/// `<repeatForSequence>`. The iteration count is derived like any other
/// cell and read by the builder (see `CONTEXT.md`, structural cell):
///
/// ```text
/// lengthFromTo = floor((to - from) / step + 1 + 1e-10)
/// count        = min(max(floor(length or lengthFromTo), 0), maxNumber)
/// ```
///
/// A NaN count means zero iterations.
pub(super) const REPEAT_PROPS: &[PropDef] = &[
    /* 0 */ attr("from", 1.0),
    /* 1 */ attr("to", f64::NAN),
    /* 2 */ attr("step", 1.0),
    /* 3 */ attr("maxNumber", 10_000.0),
    /* 4 */ computed("span", OpSpec::Sub, &[1, 0]),
    /* 5 */ computed("spanSteps", OpSpec::Div, &[4, 2]),
    /* 6 */ computed("spanStepsEps", OpSpec::Offset { k: 1.0 + 1e-10 }, &[5]),
    /* 7 */ computed("lengthFromTo", OpSpec::Floor, &[6]),
    /* 8 */
    PropDef {
        name: "length",
        default: f64::NAN,
        from: PropFrom::AttributeOr { alias: 7 },
        attr: None,
        bind: None,
        ref_prop: None,
    },
    /* 9 */ computed("lengthFloor", OpSpec::Floor, &[8]),
    /* 10 */
    computed(
        "lengthNonNeg",
        OpSpec::Clamp {
            lo: 0.0,
            hi: f64::INFINITY,
        },
        &[9],
    ),
    /* 11 */ computed("count", OpSpec::Min, &[10, 3]),
];

/// The value an iteration's `valueName` resolves to. `from` and `step`
/// alias the repeat's; `k` is the fixed 1-based position.
pub(super) const SEQUENCE_VALUE_PROPS: &[PropDef] = &[
    /* 0 */ attr("from", 1.0),
    /* 1 */ attr("step", 1.0),
    /* 2 */ attr("k", 1.0),
    /* 3 */ computed("km1", OpSpec::Offset { k: -1.0 }, &[2]),
    /* 4 */ computed("scaled", OpSpec::Mul, &[3, 1]),
    /* 5 */ computed("value", OpSpec::Add, &[4, 0]),
];

pub(super) const GRAPH_PROPS: &[PropDef] = &[
    attr("xmin", -10.0),
    attr("xmax", 10.0),
    attr("ymin", -10.0),
    attr("ymax", 10.0),
];
// `hide` is a boolean in the current core; here it is a 0/1 cell.
pub(super) const POINT_PROPS: &[PropDef] = &[planned("x"), planned("y"), planned("hide")];
pub(super) const BOOLEAN_INPUT_PROPS: &[PropDef] = &[attr("value", 0.0)];
// Both of a math's props are set by the builder from its children.
pub(super) const MATH_PROPS: &[PropDef] = &[
    PropDef {
        name: "expr",
        default: f64::NAN,
        from: PropFrom::Children,
        attr: None,
        bind: None,
        ref_prop: None,
    },
    PropDef {
        name: "value",
        default: f64::NAN,
        from: PropFrom::Children,
        attr: None,
        bind: None,
        ref_prop: None,
    },
];
pub(super) const EVALUATE_PROPS: &[PropDef] = &[
    PropDef {
        name: "function",
        default: f64::NAN,
        from: PropFrom::Attribute,
        attr: None,
        bind: None,
        ref_prop: Some("expr"),
    },
    attr("input", f64::NAN),
    computed("value", OpSpec::Sym(SymKind::EvalAt), &[0, 1]),
];
pub(super) const NUMBER_PROPS: &[PropDef] = &[PropDef {
    name: "value",
    default: f64::NAN,
    from: PropFrom::Children,
    attr: None,
    bind: None,
    ref_prop: None,
}];
pub(super) const NUMBER_INPUT_PROPS: &[PropDef] = &[attr("value", f64::NAN)];
// A mathInput's value is bound by a child reference or `bindValueTo`,
// else it is the `prefill` (the builder reads it). The builder plans
// `expr` from what `value` turned out to be.
pub(super) const MATH_INPUT_PROPS: &[PropDef] = &[
    PropDef {
        name: "value",
        default: f64::NAN,
        from: PropFrom::Children,
        attr: Some("prefill"),
        bind: Some("bindValueTo"),
        ref_prop: None,
    },
    PropDef {
        name: "expr",
        default: f64::NAN,
        from: PropFrom::Attribute,
        attr: Some("(planned)"),
        bind: None,
        ref_prop: None,
    },
];
// Planned by the builder (`plan_symbolic`); `samples` is the first of
// `SAMPLES` consecutive cells.
pub(super) const CURVE_PROPS: &[PropDef] = &[
    PropDef {
        name: "expr",
        default: f64::NAN,
        from: PropFrom::Children,
        attr: None,
        bind: None,
        ref_prop: None,
    },
    planned("xmin"),
    planned("xmax"),
    planned("samples"),
];
pub(super) const ANSWER_PROPS: &[PropDef] = &[
    PropDef {
        name: "response",
        default: f64::NAN,
        from: PropFrom::Attribute,
        attr: None,
        bind: None,
        ref_prop: Some("expr"),
    },
    PropDef {
        name: "correct",
        default: f64::NAN,
        from: PropFrom::Children,
        attr: None,
        bind: None,
        ref_prop: None,
    },
    attr("submitted", f64::NAN),
    planned("credit"),
    attr("weight", 1.0),
];
pub(super) const OP_PROPS: &[PropDef] = &[PropDef {
    name: "value",
    default: f64::NAN,
    from: PropFrom::Derived,
    attr: None,
    bind: None,
    ref_prop: None,
}];
pub(super) const COLLECT_PROPS: &[PropDef] = &[attr("count", 0.0)];
pub(super) const STICKY_GROUP_PROPS: &[PropDef] = &[
    attr("threshold", f64::NAN),
    attr("relativeToGraphScales", 0.0),
];
// `creditAchieved` and `number` are wired after expansion; the
// flags are literals the builder reads, since they decide the wiring.
pub(super) const DOCUMENT_PROPS: &[PropDef] = &[
    planned("creditAchieved"),
    computed("percentCreditAchieved", OpSpec::Scale { k: 100.0 }, &[0]),
];
pub(super) const SECTION_PROPS: &[PropDef] = &[
    planned("creditAchieved"),
    computed("percentCreditAchieved", OpSpec::Scale { k: 100.0 }, &[0]),
    attr("weight", 1.0),
    planned("aggregateScores"),
    planned("number"),
    planned("includeParentNumber"),
    planned("label"),
];
pub(super) const TEXT_PROPS: &[PropDef] = &[PropDef {
    name: "value",
    default: f64::NAN,
    from: PropFrom::Children,
    attr: None,
    bind: None,
    ref_prop: None,
}];
// `hide` hides what the choice shows, not copies of its names.
pub(super) const CONDITIONAL_CONTENT_PROPS: &[PropDef] = &[planned("choice"), planned("hide")];
pub(super) const CASE_PROPS: &[PropDef] = &[planned("active")];
pub(super) const SELECT_PROPS: &[PropDef] = &[planned("hide")];

const fn row(
    kind: ComponentKind,
    tags: &'static [&'static str],
    props: &'static [PropDef],
    flags: u8,
) -> KindInfo {
    KindInfo {
        kind,
        tags,
        props,
        default_prop: None,
        views: &[],
        arrays: &[],
        aliases: &[],
        flags,
        sticky: None,
    }
}

pub(super) const LINE_POINTS: &[ArrayProp] = &[ArrayProp {
    names: &["points", "endpoints"],
    item: "point",
    items: &[["x1", "y1"], ["x2", "y2"]],
}];

pub const KINDS: [KindInfo; 31] = {
    use crate::tick::snap::Shape;
    use ComponentKind as K;
    [
        row(K::Document, &["document"], DOCUMENT_PROPS, 0),
        row(K::Graph, &["graph"], GRAPH_PROPS, CONTAINER),
        row(K::Point, &["point"], POINT_PROPS, COPYABLE | PLANNED)
            .default_prop("coords")
            .views(&[("coords", ["x", "y"])])
            .sticky(Shape::Point, prop::point::X, 1),
        row(K::Number, &["number"], NUMBER_PROPS, COPYABLE).default_prop("value"),
        row(
            K::NumberInput,
            &["numberInput"],
            NUMBER_INPUT_PROPS,
            COPYABLE,
        )
        .default_prop("value"),
        row(K::Op, &["op"], OP_PROPS, COPYABLE).default_prop("value"),
        row(K::Slider, &["slider"], SLIDER_PROPS, COPYABLE).default_prop("value"),
        row(
            K::RepeatForSequence,
            &["repeatForSequence"],
            REPEAT_PROPS,
            0,
        ),
        row(K::Collect, &["collect"], COLLECT_PROPS, 0),
        row(
            K::SequenceValue,
            &["sequenceValue"],
            SEQUENCE_VALUE_PROPS,
            COPYABLE | INTERNAL,
        )
        .default_prop("value"),
        row(
            K::BooleanInput,
            &["booleanInput"],
            BOOLEAN_INPUT_PROPS,
            COPYABLE,
        )
        .default_prop("value"),
        row(K::Math, &["math"], MATH_PROPS, COPYABLE).default_prop("value"),
        row(K::Evaluate, &["evaluate"], EVALUATE_PROPS, COPYABLE).default_prop("value"),
        row(K::MathInput, &["mathInput"], MATH_INPUT_PROPS, COPYABLE).default_prop("value"),
        row(K::Circle, &["circle"], CIRCLE_PROPS, COPYABLE | PLANNED)
            .views(&[
                ("center", ["centerX", "centerY"]),
                ("numericalCenter", ["cx", "cy"]),
            ])
            .arrays(&[ArrayProp {
                names: &["throughPoints"],
                item: "throughPoint",
                items: &[
                    ["throughX1", "throughY1"],
                    ["throughX2", "throughY2"],
                    ["throughX3", "throughY3"],
                ],
            }])
            .aliases(&[
                ("centerX1", "centerX"),
                ("centerX2", "centerY"),
                ("throughPointX1_1", "throughX1"),
                ("throughPointX1_2", "throughY1"),
                ("throughPointX2_1", "throughX2"),
                ("throughPointX2_2", "throughY2"),
                ("throughPointX3_1", "throughX3"),
                ("throughPointX3_2", "throughY3"),
            ]),
        row(K::Line, &["line"], LINE_PROPS, COPYABLE | PLANNED).arrays(LINE_POINTS),
        row(
            K::LineSegment,
            &["lineSegment"],
            LINE_SEGMENT_PROPS,
            COPYABLE | PLANNED,
        )
        .arrays(LINE_POINTS)
        .sticky(Shape::Open, prop::segment::X1, 2),
        row(
            K::Polygon,
            &["polygon", "triangle"],
            POLYGON_PROPS,
            COPYABLE | PLANNED,
        )
        .arrays(&[ArrayProp {
            names: &["vertices"],
            item: "vertex",
            items: &VERTEX_PARTS,
        }])
        .sticky(Shape::Closed, prop::polygon::X1, MAX_VERTICES),
        row(K::PointList, &["pointList"], &[], 0),
        row(K::P, &["p"], &[], CONTAINER),
        row(K::Setup, &["setup"], &[], CONTAINER),
        row(
            K::StickyGroup,
            &["stickyGroup"],
            STICKY_GROUP_PROPS,
            CONTAINER,
        ),
        row(K::Function, &["function"], CURVE_PROPS, COPYABLE | SYMBOLIC).default_prop("expr"),
        row(
            K::Derivative,
            &["derivative"],
            CURVE_PROPS,
            COPYABLE | SYMBOLIC,
        )
        .default_prop("expr"),
        row(K::Answer, &["answer"], ANSWER_PROPS, COPYABLE | SYMBOLIC).default_prop("credit"),
        row(K::Text, &["text"], TEXT_PROPS, COPYABLE).default_prop("value"),
        row(
            K::ConditionalContent,
            &["conditionalContent"],
            CONDITIONAL_CONTENT_PROPS,
            0,
        ),
        row(K::Case, &["case"], CASE_PROPS, 0),
        row(K::Select, &["select"], SELECT_PROPS, 0),
        // Containers the prototype renders nothing special for.
        row(K::Group, &["group", "label"], &[], CONTAINER),
        // The parser writes `<section>` as `<division type="section">`.
        row(
            K::Section,
            &[
                "section",
                "division",
                "subsection",
                "subsubsection",
                "problem",
                "exercise",
                "example",
            ],
            SECTION_PROPS,
            CONTAINER,
        ),
    ]
};

const _: () = {
    let mut i = 0;
    while i < KINDS.len() {
        assert!(
            KINDS[i].kind as usize == i,
            "KINDS is in discriminant order"
        );
        i += 1;
    }
};
