//! The data: every type's prop table and its row in `COMPONENT_TYPES`.

use super::{
    Args, ArrayProp, ComponentTypeInfo, MAX_ARGS, MAX_VERTICES, PropDef, PropFrom, info, prop,
    str_eq,
};
use crate::program::{OpSpec, SymKind};
use crate::tick::snap::Shape;

/// A prop as a table writes it: like a [`PropDef`], but naming the props a
/// computed prop reads. [`props`] resolves the names to positions.
#[derive(Clone, Copy)]
struct Prop {
    name: &'static str,
    default: f64,
    source: Source,
    attr: Option<&'static str>,
    bind: Option<&'static str>,
    ref_prop: Option<&'static str>,
}

/// [`PropFrom`] with props named rather than numbered.
#[derive(Clone, Copy)]
enum Source {
    Attribute,
    Children,
    Derived,
    Computed(OpSpec, &'static [&'static str]),
    AttributeOr(&'static str),
    Planned,
}

const fn prop_from(name: &'static str, default: f64, source: Source) -> Prop {
    Prop {
        name,
        default,
        source,
        attr: None,
        bind: None,
        ref_prop: None,
    }
}

/// Given by the attribute of the same name, else `default`.
const fn attr(name: &'static str, default: f64) -> Prop {
    prop_from(name, default, Source::Attribute)
}

/// Given by the attribute of the same name, else an alias of the prop `or`.
const fn attr_or(name: &'static str, or: &'static str) -> Prop {
    prop_from(name, f64::NAN, Source::AttributeOr(or))
}

/// Given by the element's children: a literal, one reference, or math.
const fn children(name: &'static str) -> Prop {
    prop_from(name, f64::NAN, Source::Children)
}

/// `op` applied to the component's own props `args`.
const fn computed(name: &'static str, op: OpSpec, args: &'static [&'static str]) -> Prop {
    prop_from(name, f64::NAN, Source::Computed(op, args))
}

/// The `<op>` element's operator over its `args`.
const fn derived(name: &'static str) -> Prop {
    prop_from(name, f64::NAN, Source::Derived)
}

/// A prop whose source the builder plans from the element's specification.
const fn planned(name: &'static str) -> Prop {
    prop_from(name, f64::NAN, Source::Planned)
}

impl Prop {
    /// Read the attribute `attr` instead of the prop's name.
    const fn attribute(mut self, attr: &'static str) -> Self {
        self.attr = Some(attr);
        self
    }
    /// When the attribute `attr` is present, alias the component it
    /// references (a slider's `bindValueTo`).
    const fn bind(mut self, attr: &'static str) -> Self {
        self.bind = Some(attr);
        self
    }
    /// When the attribute is a bare reference, alias this prop of the
    /// referent instead of its default prop.
    const fn ref_prop(mut self, prop: &'static str) -> Self {
        self.ref_prop = Some(prop);
        self
    }
}

/// A table's [`PropDef`]s, with each prop name a computed prop reads
/// resolved to its position. A name not in the table fails to compile.
const fn props<const N: usize>(table: [Prop; N]) -> [PropDef; N] {
    let blank = PropDef {
        name: "",
        default: f64::NAN,
        from: PropFrom::Planned,
        attr: None,
        bind: None,
        ref_prop: None,
    };
    let mut out = [blank; N];
    let mut i = 0;
    while i < N {
        let p = table[i];
        let from = match p.source {
            Source::Attribute => PropFrom::Attribute,
            Source::Children => PropFrom::Children,
            Source::Derived => PropFrom::Derived,
            Source::Planned => PropFrom::Planned,
            Source::AttributeOr(or) => PropFrom::AttributeOr {
                alias: position(&table, or),
            },
            Source::Computed(op, names) => {
                assert!(names.len() <= MAX_ARGS, "too many args");
                let mut at = [0; MAX_ARGS];
                let mut j = 0;
                while j < names.len() {
                    at[j] = position(&table, names[j]);
                    j += 1;
                }
                PropFrom::Computed {
                    op,
                    args: Args {
                        at,
                        len: names.len() as u8,
                    },
                }
            }
        };
        out[i] = PropDef {
            name: p.name,
            default: p.default,
            from,
            attr: p.attr,
            bind: p.bind,
            ref_prop: p.ref_prop,
        };
        i += 1;
    }
    out
}

const fn position(table: &[Prop], name: &str) -> u8 {
    let mut i = 0;
    while i < table.len() {
        if str_eq(table[i].name, name) {
            return i as u8;
        }
        i += 1;
    }
    panic!("no such prop")
}

/// `<circle>`. `cx`, `cy`, `radius` are the numerical center and radius;
/// how they are produced depends on which of `center`, `radius` and
/// `through` the author gave (see `plan_circle` in `build/compile/geometry/circle.rs`).
pub(super) const CIRCLE_PROPS: &[PropDef] = &props([
    planned("cx"),
    planned("cy"),
    planned("radius"),
    computed("diameter", OpSpec::Scale { k: 2.0 }, &["radius"]),
    computed(
        "circumference",
        OpSpec::Scale {
            k: std::f64::consts::TAU,
        },
        &["radius"],
    ),
    planned("area"),
    // The center as a *reference*: the prescribed center when there is one
    // (so a point extending `$c.center` drags that point alone, as in the
    // current core), else the derived center.
    planned("centerX"),
    planned("centerY"),
    planned("throughX1"),
    planned("throughY1"),
    planned("throughX2"),
    planned("throughY2"),
    planned("throughX3"),
    planned("throughY3"),
    planned("numThroughPoints"),
]);

/// `<line>` and `<lineSegment>`. The first four props are the shape's own
/// points (ADR 0006). `basedOnDirection` is 1 when the second point is
/// derived from a slope or direction, so a renderer drags only the first.
pub(super) const LINE_PROPS: &[PropDef] = &props([
    planned("x1"),
    planned("y1"),
    planned("x2"),
    planned("y2"),
    planned("slope"),
    planned("xintercept"),
    planned("yintercept"),
    planned("coeffvar1"),
    planned("coeffvar2"),
    planned("coeff0"),
    planned("basedOnDirection"),
]);

pub(super) const LINE_SEGMENT_PROPS: &[PropDef] =
    &props([planned("x1"), planned("y1"), planned("x2"), planned("y2")]);

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
pub(super) const POLYGON_PROPS: &[PropDef] = &props({
    let mut out = [planned(""); 1 + 2 * MAX_VERTICES];
    out[0] = planned("numVertices");
    let mut i = 0;
    while i < MAX_VERTICES {
        out[1 + 2 * i] = planned(VERTEX_PARTS[i][0]);
        out[2 + 2 * i] = planned(VERTEX_PARTS[i][1]);
        i += 1;
    }
    out
});

/// `<slider>` in numeric mode. The chain reproduces the current core's
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
pub(super) const SLIDER_PROPS: &[PropDef] = &props([
    attr("from", 0.0),
    attr("to", 10.0),
    attr("step", 1.0),
    attr("preliminaryValue", 0.0)
        .attribute("initialValue")
        .bind("bindValueTo"),
    computed("span", OpSpec::Sub, &["to", "from"]),
    computed("spanSteps", OpSpec::Div, &["span", "step"]),
    computed("spanStepsEps", OpSpec::Offset { k: 1e-10 }, &["spanSteps"]),
    computed("maxIndex", OpSpec::Floor, &["spanStepsEps"]),
    computed("offset", OpSpec::Sub, &["preliminaryValue", "from"]),
    computed("rawIndex", OpSpec::Div, &["offset", "step"]),
    computed("roundedIndex", OpSpec::Round, &["rawIndex"]),
    computed(
        "nonNegIndex",
        OpSpec::Clamp {
            lo: 0.0,
            hi: f64::INFINITY,
        },
        &["roundedIndex"],
    ),
    computed("clampedIndex", OpSpec::Min, &["nonNegIndex", "maxIndex"]),
    // A non-finite stored value is index 0, so the slider shows `from`,
    // as in the current core.
    computed("index", OpSpec::NanTo { k: 0.0 }, &["clampedIndex"]),
    computed("scaled", OpSpec::Mul, &["index", "step"]),
    computed("value", OpSpec::Add, &["scaled", "from"]),
]);

/// `<repeatForSequence>`. The iteration count is derived like any other
/// cell and read by the builder (see `CONTEXT.md`, structural cell):
///
/// ```text
/// lengthFromTo = floor((to - from) / step + 1 + 1e-10)
/// count        = min(max(floor(length or lengthFromTo), 0), maxNumber)
/// ```
///
/// A NaN count means zero iterations.
pub(super) const REPEAT_PROPS: &[PropDef] = &props([
    attr("from", 1.0),
    attr("to", f64::NAN),
    attr("step", 1.0),
    attr("maxNumber", 10_000.0),
    computed("span", OpSpec::Sub, &["to", "from"]),
    computed("spanSteps", OpSpec::Div, &["span", "step"]),
    computed(
        "spanStepsEps",
        OpSpec::Offset { k: 1.0 + 1e-10 },
        &["spanSteps"],
    ),
    computed("lengthFromTo", OpSpec::Floor, &["spanStepsEps"]),
    attr_or("length", "lengthFromTo"),
    computed("lengthFloor", OpSpec::Floor, &["length"]),
    computed(
        "lengthNonNeg",
        OpSpec::Clamp {
            lo: 0.0,
            hi: f64::INFINITY,
        },
        &["lengthFloor"],
    ),
    computed("count", OpSpec::Min, &["lengthNonNeg", "maxNumber"]),
]);

/// The value an iteration's `valueName` resolves to. `from` and `step`
/// alias the repeat's; `k` is the fixed 1-based position.
pub(super) const SEQUENCE_VALUE_PROPS: &[PropDef] = &props([
    attr("from", 1.0),
    attr("step", 1.0),
    attr("k", 1.0),
    computed("km1", OpSpec::Offset { k: -1.0 }, &["k"]),
    computed("scaled", OpSpec::Mul, &["km1", "step"]),
    computed("value", OpSpec::Add, &["scaled", "from"]),
]);

pub(super) const GRAPH_PROPS: &[PropDef] = &props([
    attr("xmin", -10.0),
    attr("xmax", 10.0),
    attr("ymin", -10.0),
    attr("ymax", 10.0),
]);
// `hide` is a boolean in the current core; here it is a 0/1 cell.
pub(super) const POINT_PROPS: &[PropDef] = &props([planned("x"), planned("y"), planned("hide")]);
pub(super) const BOOLEAN_INPUT_PROPS: &[PropDef] = &props([attr("value", 0.0)]);
// Both of a math's props are set by the builder from its children.
pub(super) const MATH_PROPS: &[PropDef] = &props([children("expr"), children("value")]);
pub(super) const EVALUATE_PROPS: &[PropDef] = &props([
    attr("function", f64::NAN).ref_prop("expr"),
    attr("input", f64::NAN),
    computed(
        "value",
        OpSpec::Sym(SymKind::EvalAt),
        &["function", "input"],
    ),
]);
pub(super) const NUMBER_PROPS: &[PropDef] = &props([children("value")]);
pub(super) const NUMBER_INPUT_PROPS: &[PropDef] = &props([attr("value", f64::NAN)]);
// A mathInput's value is bound by a child reference or `bindValueTo`,
// else it is the `prefill` (the builder reads it). The builder plans
// `expr` from what `value` turned out to be.
pub(super) const MATH_INPUT_PROPS: &[PropDef] = &props([
    children("value").attribute("prefill").bind("bindValueTo"),
    attr("expr", f64::NAN).attribute("(planned)"),
]);
// Planned by the builder (`plan_symbolic`); `samples` is the first of
// `SAMPLES` consecutive cells.
pub(super) const CURVE_PROPS: &[PropDef] = &props([
    children("expr"),
    planned("xmin"),
    planned("xmax"),
    planned("samples"),
]);
pub(super) const ANSWER_PROPS: &[PropDef] = &props([
    attr("response", f64::NAN).ref_prop("expr"),
    children("correct"),
    attr("submitted", f64::NAN),
    planned("credit"),
    attr("weight", 1.0),
]);
pub(super) const OP_PROPS: &[PropDef] = &props([derived("value")]);
pub(super) const COLLECT_PROPS: &[PropDef] = &props([attr("count", 0.0)]);
pub(super) const STICKY_GROUP_PROPS: &[PropDef] = &props([
    attr("threshold", f64::NAN),
    attr("relativeToGraphScales", 0.0),
]);
// `creditAchieved` and `number` are wired after expansion; the
// flags are literals the builder reads, since they decide the wiring.
pub(super) const DOCUMENT_PROPS: &[PropDef] = &props([
    planned("creditAchieved"),
    computed(
        "percentCreditAchieved",
        OpSpec::Scale { k: 100.0 },
        &["creditAchieved"],
    ),
]);
pub(super) const SECTION_PROPS: &[PropDef] = &props([
    planned("creditAchieved"),
    computed(
        "percentCreditAchieved",
        OpSpec::Scale { k: 100.0 },
        &["creditAchieved"],
    ),
    attr("weight", 1.0),
    planned("aggregateScores"),
    planned("number"),
    planned("includeParentNumber"),
    planned("label"),
]);
pub(super) const TEXT_PROPS: &[PropDef] = &props([children("value")]);
// `hide` hides what the choice shows, not copies of its names.
pub(super) const CONDITIONAL_CONTENT_PROPS: &[PropDef] =
    &props([planned("choice"), planned("hide")]);
pub(super) const CASE_PROPS: &[PropDef] = &props([planned("active")]);
pub(super) const SELECT_PROPS: &[PropDef] = &props([planned("hide")]);

pub(super) const LINE_POINTS: &[ArrayProp] = &[ArrayProp {
    names: &["points", "endpoints"],
    item: "point",
    items: &[["x1", "y1"], ["x2", "y2"]],
}];

// Each type's row of `COMPONENT_TYPES`, named in `component_types!`.

pub(super) const DOCUMENT: ComponentTypeInfo = info(&["document"], DOCUMENT_PROPS);

pub(super) const GRAPH: ComponentTypeInfo = info(&["graph"], GRAPH_PROPS).container();

pub(super) const POINT: ComponentTypeInfo = info(&["point"], POINT_PROPS)
    .copyable()
    .planned()
    .default_prop("coords")
    .views(&[("coords", ["x", "y"])])
    .sticky(Shape::Point, prop::point::X, 1);

pub(super) const NUMBER: ComponentTypeInfo = info(&["number"], NUMBER_PROPS)
    .copyable()
    .default_prop("value");

pub(super) const NUMBER_INPUT: ComponentTypeInfo = info(&["numberInput"], NUMBER_INPUT_PROPS)
    .copyable()
    .default_prop("value");

pub(super) const OP: ComponentTypeInfo = info(&["op"], OP_PROPS).copyable().default_prop("value");

pub(super) const SLIDER: ComponentTypeInfo = info(&["slider"], SLIDER_PROPS)
    .copyable()
    .default_prop("value");

pub(super) const REPEAT_FOR_SEQUENCE: ComponentTypeInfo =
    info(&["repeatForSequence"], REPEAT_PROPS);

pub(super) const COLLECT: ComponentTypeInfo = info(&["collect"], COLLECT_PROPS);

pub(super) const SEQUENCE_VALUE: ComponentTypeInfo = info(&["sequenceValue"], SEQUENCE_VALUE_PROPS)
    .copyable()
    .internal()
    .default_prop("value");

pub(super) const BOOLEAN_INPUT: ComponentTypeInfo = info(&["booleanInput"], BOOLEAN_INPUT_PROPS)
    .copyable()
    .default_prop("value");

pub(super) const MATH: ComponentTypeInfo =
    info(&["math"], MATH_PROPS).copyable().default_prop("value");

pub(super) const EVALUATE: ComponentTypeInfo = info(&["evaluate"], EVALUATE_PROPS)
    .copyable()
    .default_prop("value");

pub(super) const MATH_INPUT: ComponentTypeInfo = info(&["mathInput"], MATH_INPUT_PROPS)
    .copyable()
    .default_prop("value");

pub(super) const CIRCLE: ComponentTypeInfo = info(&["circle"], CIRCLE_PROPS)
    .copyable()
    .planned()
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
    ]);

pub(super) const LINE: ComponentTypeInfo = info(&["line"], LINE_PROPS)
    .copyable()
    .planned()
    .arrays(LINE_POINTS);

pub(super) const LINE_SEGMENT: ComponentTypeInfo = info(&["lineSegment"], LINE_SEGMENT_PROPS)
    .copyable()
    .planned()
    .arrays(LINE_POINTS)
    .sticky(Shape::Open, prop::segment::X1, 2);

pub(super) const POLYGON: ComponentTypeInfo = info(&["polygon", "triangle"], POLYGON_PROPS)
    .copyable()
    .planned()
    .arrays(&[ArrayProp {
        names: &["vertices"],
        item: "vertex",
        items: &VERTEX_PARTS,
    }])
    .sticky(Shape::Closed, prop::polygon::X1, MAX_VERTICES);

pub(super) const POINT_LIST: ComponentTypeInfo = info(&["pointList"], &[]);

pub(super) const P: ComponentTypeInfo = info(&["p"], &[]).container();

pub(super) const SETUP: ComponentTypeInfo = info(&["setup"], &[]).container();

pub(super) const STICKY_GROUP: ComponentTypeInfo =
    info(&["stickyGroup"], STICKY_GROUP_PROPS).container();

pub(super) const FUNCTION: ComponentTypeInfo = info(&["function"], CURVE_PROPS)
    .copyable()
    .symbolic()
    .default_prop("expr");

pub(super) const DERIVATIVE: ComponentTypeInfo = info(&["derivative"], CURVE_PROPS)
    .copyable()
    .symbolic()
    .default_prop("expr");

pub(super) const ANSWER: ComponentTypeInfo = info(&["answer"], ANSWER_PROPS)
    .copyable()
    .symbolic()
    .default_prop("credit");

pub(super) const TEXT: ComponentTypeInfo =
    info(&["text"], TEXT_PROPS).copyable().default_prop("value");

pub(super) const CONDITIONAL_CONTENT: ComponentTypeInfo =
    info(&["conditionalContent"], CONDITIONAL_CONTENT_PROPS);

pub(super) const CASE: ComponentTypeInfo = info(&["case"], CASE_PROPS);

pub(super) const SELECT: ComponentTypeInfo = info(&["select"], SELECT_PROPS);

// Containers the prototype renders nothing special for.
pub(super) const GROUP: ComponentTypeInfo = info(&["group", "label"], &[]).container();

// The parser writes `<section>` as `<division type="section">`.
pub(super) const SECTION: ComponentTypeInfo = info(
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
)
.container();
