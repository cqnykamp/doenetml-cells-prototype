//! Stripped-down component definitions: which props each tag has, where each
//! prop's value comes from, and its default. This is the only place that
//! knows DoenetML tag vocabulary.
//!
//! A component's props are its slots; a prop may be computed from the
//! component's other props by an operator (`PropFrom::Computed`), which is how
//! a component such as `<slider>` describes its whole value chain without any
//! code of its own: the chain is data, and inversion through it is the
//! generic operator inverse.

use crate::program::ops::{OpSpec, SymKind};

/// `repr(u8)` so the renderer can read the kind column as a byte array; the
/// discriminant order matches `ComponentKind::ALL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ComponentKind {
    Document = 0,
    Graph = 1,
    Point = 2,
    Number = 3,
    NumberInput = 4,
    /// Prototype-only tag that applies a numeric operator to referenced cells.
    Op = 5,
    Slider = 6,
    /// `<repeatForSequence>`: its children are every iteration's expanded
    /// template, flattened; its `count` prop is the structural cell.
    RepeatForSequence = 7,
    /// `<collect>`: its children are copies of the collected components.
    Collect = 8,
    /// The hidden component behind a repeat's `valueName`: `from + (k-1) * step`.
    SequenceValue = 9,
    /// A checkbox: its value cell holds 0 or 1 like any other `f64` cell.
    BooleanInput = 10,
    /// `<math>`: lowered to a numeric chain in `value` when its expression
    /// is all numbers and numeric cells (`expr` is then NaN). Otherwise a
    /// math cell: `expr` holds an engine handle, instantiated from the
    /// template whenever a leaf changes (then simplified or expanded if the
    /// attribute says so), and `value` evaluates it (NaN with free symbols).
    Math = 11,
    /// `<evaluate function="$m" input="$a"/>`: the math's expression with
    /// its free symbol set to the input.
    Evaluate = 12,
    /// `<mathInput>`: bound to another cell by a child reference or
    /// `bindValueTo`, it is a numeric input (`expr` NaN). Unbound, its `expr`
    /// is an essential math cell (typing writes a parsed handle) and `value`
    /// evaluates it; a request on `value` writes a constant expression.
    MathInput = 13,
    /// `<circle>`: center and radius are derived or essential depending on
    /// how the circle is specified (plan 3). Chains are planned in `build.rs`.
    Circle = 14,
    /// `<line>`: its own two points are derived cells (ADR 0006); slope,
    /// intercepts and coefficients follow from them or from the equation.
    Line = 15,
    /// `<lineSegment endpoints="$a $b">`.
    LineSegment = 16,
    /// `<polygon vertices="...">`, optionally rigid. Up to `MAX_VERTICES`.
    Polygon = 17,
    /// `<pointList extend="$l.points">`: its children are points aliasing
    /// the items of an array prop.
    PointList = 18,
    /// `<p>`: a rendered container with no props of its own.
    P = 19,
    /// `<setup>`: an unrendered container.
    Setup = 20,
    /// `<stickyGroup>`: a container whose members snap to one another when
    /// dragged (plan 4). `threshold` NaN means the default.
    StickyGroup = 21,
    /// `<function>`: a math cell `expr` (variable `x`) and, as a curve, the
    /// `SAMPLES` cells from `samples` on, filled by a `Sample` instruction
    /// over the enclosing graph's x-range.
    Function = 22,
    /// `<derivative>$f</derivative>`: d/dx of a function or math, sampled
    /// like a function.
    Derivative = 23,
    /// `<answer response="$mi">correct</answer>`: `submitted` is an
    /// essential math cell that a submit request sets to the response;
    /// `credit` compares it with `correct` (`symbolicEquality`: as written).
    Answer = 24,
    /// `<text>` with literal content: `value` is a fixed cell holding the
    /// string id of its text (plan 6). A cell's meaning is a property of
    /// the operators around it, so a text value never reaches a numeric one.
    Text = 25,
    /// `<conditionalContent>`, a reactive choice (plan 6, ADR 0009). Its
    /// `choice` cell is the 1-based position of the first case whose
    /// condition holds, or 0. Its children are `Case` components.
    ConditionalContent = 26,
    /// One built branch of a reactive choice: `active` is 1 while it is the
    /// chosen one. Its children are the branch's content. A renderer shows
    /// the children of an active case only.
    Case = 27,
    /// `<select>`, a load-time choice: its children are the content of the
    /// options it picked, flattened like a repeat's iterations.
    Select = 28,
    /// `<group>`: a rendered container with no props of its own.
    Group = 29,
    /// `<section>`, `<subsection>`, `<subsubsection>`, `<problem>`,
    /// `<exercise>`, `<example>`: a rendered container that is numbered
    /// among its sibling sections and, when it aggregates scores, holds the
    /// weighted credit of the answers and sections inside it. Both are
    /// wired after expansion (`build/scoring.rs`).
    Section = 30,
}

/// The tags that make a `Section`, in the order of its `label` cell, with
/// the word a title shows and whether the tag aggregates scores and
/// includes its parent section's number by default (the current core's
/// `Sectioning.js`).
pub const SECTION_TAGS: [(&str, &str, bool, bool); 6] = [
    ("section", "Section", false, true),
    ("subsection", "Section", false, true),
    ("subsubsection", "Section", false, true),
    ("problem", "Problem", true, false),
    ("exercise", "Exercise", true, false),
    ("example", "Example", false, false),
];

/// Largest polygon the fixed prop layout holds.
pub const MAX_VERTICES: usize = 16;

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
/// `through` the author gave (see `plan_circle` in `build.rs`).
const CIRCLE_PROPS: &[PropDef] = &[
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
const LINE_PROPS: &[PropDef] = &[
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

const LINE_SEGMENT_PROPS: &[PropDef] = &[
    /* 0 */ planned("x1"),
    /* 1 */ planned("y1"),
    /* 2 */ planned("x2"),
    /* 3 */ planned("y2"),
];

/// `vertexN` of a polygon, as prop name pairs.
const VERTEX_PARTS: [[&str; 2]; MAX_VERTICES] = [
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
const POLYGON_PROPS: &[PropDef] = {
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
const SLIDER_PROPS: &[PropDef] = &[
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
const REPEAT_PROPS: &[PropDef] = &[
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
const SEQUENCE_VALUE_PROPS: &[PropDef] = &[
    /* 0 */ attr("from", 1.0),
    /* 1 */ attr("step", 1.0),
    /* 2 */ attr("k", 1.0),
    /* 3 */ computed("km1", OpSpec::Offset { k: -1.0 }, &[2]),
    /* 4 */ computed("scaled", OpSpec::Mul, &[3, 1]),
    /* 5 */ computed("value", OpSpec::Add, &[4, 0]),
];

const GRAPH_PROPS: &[PropDef] = &[
    attr("xmin", -10.0),
    attr("xmax", 10.0),
    attr("ymin", -10.0),
    attr("ymax", 10.0),
];
// `hide` is a boolean in the current core; here it is a 0/1 cell.
const POINT_PROPS: &[PropDef] = &[planned("x"), planned("y"), planned("hide")];
const BOOLEAN_INPUT_PROPS: &[PropDef] = &[attr("value", 0.0)];
// Both of a math's props are set by the builder from its children.
const MATH_PROPS: &[PropDef] = &[
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
const EVALUATE_PROPS: &[PropDef] = &[
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
const NUMBER_PROPS: &[PropDef] = &[PropDef {
    name: "value",
    default: f64::NAN,
    from: PropFrom::Children,
    attr: None,
    bind: None,
    ref_prop: None,
}];
const NUMBER_INPUT_PROPS: &[PropDef] = &[attr("value", f64::NAN)];
// A mathInput's value is bound by a child reference or `bindValueTo`,
// else it is the `prefill` (the builder reads it). The builder plans
// `expr` from what `value` turned out to be.
const MATH_INPUT_PROPS: &[PropDef] = &[
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
const CURVE_PROPS: &[PropDef] = &[
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
const ANSWER_PROPS: &[PropDef] = &[
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
const OP_PROPS: &[PropDef] = &[PropDef {
    name: "value",
    default: f64::NAN,
    from: PropFrom::Derived,
    attr: None,
    bind: None,
    ref_prop: None,
}];
const COLLECT_PROPS: &[PropDef] = &[attr("count", 0.0)];
const STICKY_GROUP_PROPS: &[PropDef] = &[
    attr("threshold", f64::NAN),
    attr("relativeToGraphScales", 0.0),
];
// `creditAchieved` and `number` are wired after expansion; the
// flags are literals the builder reads, since they decide the wiring.
const DOCUMENT_PROPS: &[PropDef] = &[
    planned("creditAchieved"),
    computed("percentCreditAchieved", OpSpec::Scale { k: 100.0 }, &[0]),
];
const SECTION_PROPS: &[PropDef] = &[
    planned("creditAchieved"),
    computed("percentCreditAchieved", OpSpec::Scale { k: 100.0 }, &[0]),
    attr("weight", 1.0),
    planned("aggregateScores"),
    planned("number"),
    planned("includeParentNumber"),
    planned("label"),
];
const TEXT_PROPS: &[PropDef] = &[PropDef {
    name: "value",
    default: f64::NAN,
    from: PropFrom::Children,
    attr: None,
    bind: None,
    ref_prop: None,
}];
// `hide` hides what the choice shows, not copies of its names.
const CONDITIONAL_CONTENT_PROPS: &[PropDef] = &[planned("choice"), planned("hide")];
const CASE_PROPS: &[PropDef] = &[planned("active")];
const SELECT_PROPS: &[PropDef] = &[planned("hide")];

/// Positions of props in their kind's table, for code that sets or reads a
/// prop by position. Each is looked up by name when compiled, so renaming
/// or reordering a table cannot silently break a reader.
pub mod prop {
    use super::*;

    const fn at(defs: &[PropDef], name: &str) -> usize {
        let mut i = 0;
        while i < defs.len() {
            let (a, b) = (defs[i].name.as_bytes(), name.as_bytes());
            if a.len() == b.len() {
                let mut j = 0;
                while j < a.len() && a[j] == b[j] {
                    j += 1;
                }
                if j == a.len() {
                    return i;
                }
            }
            i += 1;
        }
        panic!("no such prop")
    }

    pub mod point {
        use super::*;
        pub const X: usize = at(POINT_PROPS, "x");
        pub const Y: usize = at(POINT_PROPS, "y");
        pub const HIDE: usize = at(POINT_PROPS, "hide");
    }
    pub mod circle {
        use super::*;
        pub const CX: usize = at(CIRCLE_PROPS, "cx");
        pub const CY: usize = at(CIRCLE_PROPS, "cy");
        pub const RADIUS: usize = at(CIRCLE_PROPS, "radius");
        pub const AREA: usize = at(CIRCLE_PROPS, "area");
        pub const CENTER_X: usize = at(CIRCLE_PROPS, "centerX");
        pub const CENTER_Y: usize = at(CIRCLE_PROPS, "centerY");
        /// The first of three through points' (x, y) pairs.
        pub const THROUGH_X1: usize = at(CIRCLE_PROPS, "throughX1");
        pub const NUM_THROUGH_POINTS: usize = at(CIRCLE_PROPS, "numThroughPoints");
    }
    pub mod line {
        use super::*;
        pub const X1: usize = at(LINE_PROPS, "x1");
        pub const Y1: usize = at(LINE_PROPS, "y1");
        pub const X2: usize = at(LINE_PROPS, "x2");
        pub const Y2: usize = at(LINE_PROPS, "y2");
        pub const SLOPE: usize = at(LINE_PROPS, "slope");
        pub const XINTERCEPT: usize = at(LINE_PROPS, "xintercept");
        pub const YINTERCEPT: usize = at(LINE_PROPS, "yintercept");
        pub const COEFFVAR1: usize = at(LINE_PROPS, "coeffvar1");
        pub const COEFFVAR2: usize = at(LINE_PROPS, "coeffvar2");
        pub const COEFF0: usize = at(LINE_PROPS, "coeff0");
        pub const BASED_ON_DIRECTION: usize = at(LINE_PROPS, "basedOnDirection");
    }
    pub mod segment {
        use super::*;
        pub const X1: usize = at(LINE_SEGMENT_PROPS, "x1");
        pub const Y1: usize = at(LINE_SEGMENT_PROPS, "y1");
        pub const X2: usize = at(LINE_SEGMENT_PROPS, "x2");
        pub const Y2: usize = at(LINE_SEGMENT_PROPS, "y2");
    }
    pub mod polygon {
        use super::*;
        pub const NUM_VERTICES: usize = at(POLYGON_PROPS, "numVertices");
        /// The first vertex's x; vertex k's (x, y) are at `X1 + 2k`, `X1 + 2k + 1`.
        pub const X1: usize = at(POLYGON_PROPS, "x1");
    }
    pub mod graph {
        use super::*;
        pub const XMIN: usize = at(GRAPH_PROPS, "xmin");
        pub const XMAX: usize = at(GRAPH_PROPS, "xmax");
        pub const YMIN: usize = at(GRAPH_PROPS, "ymin");
        pub const YMAX: usize = at(GRAPH_PROPS, "ymax");
    }
    pub mod sticky_group {
        use super::*;
        pub const THRESHOLD: usize = at(STICKY_GROUP_PROPS, "threshold");
        pub const RELATIVE: usize = at(STICKY_GROUP_PROPS, "relativeToGraphScales");
    }
    pub mod math {
        use super::*;
        pub const EXPR: usize = at(MATH_PROPS, "expr");
        pub const VALUE: usize = at(MATH_PROPS, "value");
    }
    pub mod math_input {
        use super::*;
        pub const VALUE: usize = at(MATH_INPUT_PROPS, "value");
        pub const EXPR: usize = at(MATH_INPUT_PROPS, "expr");
    }
    pub mod answer {
        use super::*;
        pub const RESPONSE: usize = at(ANSWER_PROPS, "response");
        pub const SUBMITTED: usize = at(ANSWER_PROPS, "submitted");
        pub const CREDIT: usize = at(ANSWER_PROPS, "credit");
        pub const WEIGHT: usize = at(ANSWER_PROPS, "weight");
    }
    pub mod document {
        use super::*;
        pub const CREDIT: usize = at(DOCUMENT_PROPS, "creditAchieved");
        pub const PERCENT_CREDIT: usize = at(DOCUMENT_PROPS, "percentCreditAchieved");
    }
    pub mod section {
        use super::*;
        pub const CREDIT: usize = at(SECTION_PROPS, "creditAchieved");
        pub const PERCENT_CREDIT: usize = at(SECTION_PROPS, "percentCreditAchieved");
        pub const WEIGHT: usize = at(SECTION_PROPS, "weight");
        pub const AGGREGATE: usize = at(SECTION_PROPS, "aggregateScores");
        pub const NUMBER: usize = at(SECTION_PROPS, "number");
        pub const INCLUDE_PARENT_NUMBER: usize = at(SECTION_PROPS, "includeParentNumber");
        pub const LABEL: usize = at(SECTION_PROPS, "label");
    }
    pub mod text {
        use super::*;
        pub const VALUE: usize = at(TEXT_PROPS, "value");
    }
    pub mod conditional_content {
        use super::*;
        pub const CHOICE: usize = at(CONDITIONAL_CONTENT_PROPS, "choice");
    }
    pub mod case {
        use super::*;
        pub const ACTIVE: usize = at(CASE_PROPS, "active");
    }
}

/// What the core knows about a kind apart from how it is planned and
/// expanded: one row per kind in `KINDS`, indexed by discriminant.
pub struct KindInfo {
    pub kind: ComponentKind,
    /// The tag the kind shows as, then other tags that make it.
    pub tags: &'static [&'static str],
    pub props: &'static [PropDef],
    /// The prop a bare `$name` reference resolves to.
    pub default_prop: Option<&'static str>,
    /// Point-valued props that are views over two single-cell props.
    pub views: &'static [(&'static str, [&'static str; 2])],
    /// Array props whose items are points.
    pub arrays: &'static [ArrayProp],
    /// The current core's spellings of a few props, accepted in references
    /// so its documents resolve unchanged.
    pub aliases: &'static [(&'static str, &'static str)],
    pub flags: u8,
    /// How a member of a sticky group attracts and snaps: its shape, the
    /// prop of its first coordinate, and how many points it has at most (a
    /// polygon's live count is its `numVertices` cell).
    pub sticky: Option<(crate::tick::snap::Shape, usize, usize)>,
}

/// An array prop of points, such as a polygon's `vertices`: the names it
/// goes by, the name of an item (`vertex` for `vertex3`), and each item's
/// coordinate props, as many as the kind can hold.
pub struct ArrayProp {
    pub names: &'static [&'static str],
    pub item: &'static str,
    pub items: &'static [[&'static str; 2]],
}

/// `$name` as a child may produce a copy of the component, and
/// `<collect componentType>` may name the kind.
pub const COPYABLE: u8 = 1;
/// The children are rendered; `extend` copies them deeply.
pub const CONTAINER: u8 = 2;
/// The builder plans the props from the element's attributes and children
/// rather than from `PropFrom` (the geometric kinds).
pub const PLANNED: u8 = 4;
/// Planned as math cells (`plan_symbolic`); not allowed in a branch
/// interface.
pub const SYMBOLIC: u8 = 8;
/// Made by the builder, never by a tag in the source.
pub const INTERNAL: u8 = 16;

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

impl KindInfo {
    const fn default_prop(mut self, prop: &'static str) -> Self {
        self.default_prop = Some(prop);
        self
    }
    const fn views(mut self, views: &'static [(&'static str, [&'static str; 2])]) -> Self {
        self.views = views;
        self
    }
    const fn arrays(mut self, arrays: &'static [ArrayProp]) -> Self {
        self.arrays = arrays;
        self
    }
    const fn aliases(mut self, aliases: &'static [(&'static str, &'static str)]) -> Self {
        self.aliases = aliases;
        self
    }
    const fn sticky(mut self, shape: crate::tick::snap::Shape, first: usize, max: usize) -> Self {
        self.sticky = Some((shape, first, max));
        self
    }
}

const LINE_POINTS: &[ArrayProp] = &[ArrayProp {
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

impl ComponentKind {
    pub const ALL: [ComponentKind; 31] = {
        let mut out = [ComponentKind::Document; 31];
        let mut i = 0;
        while i < KINDS.len() {
            out[i] = KINDS[i].kind;
            i += 1;
        }
        out
    };

    pub fn info(self) -> &'static KindInfo {
        &KINDS[self as usize]
    }

    pub fn from_tag(tag: &str) -> Option<Self> {
        static BY_TAG: std::sync::OnceLock<std::collections::HashMap<&'static str, ComponentKind>> =
            std::sync::OnceLock::new();
        let by_tag = BY_TAG.get_or_init(|| {
            KINDS
                .iter()
                .filter(|k| k.flags & INTERNAL == 0)
                .flat_map(|k| k.tags.iter().map(move |&t| (t, k.kind)))
                .collect()
        });
        by_tag.get(tag).copied()
    }

    pub fn tag(self) -> &'static str {
        self.info().tags[0]
    }

    /// Single-cell props, in declaration order.
    pub fn prop_defs(self) -> &'static [PropDef] {
        self.info().props
    }

    pub fn prop_index(self, name: &str) -> Option<usize> {
        let name = self.canonical_prop(name);
        self.prop_defs().iter().position(|p| p.name == name)
    }

    /// A prop name with the current core's spellings mapped to ours.
    pub fn canonical_prop<'a>(self, name: &'a str) -> &'a str {
        self.info()
            .aliases
            .iter()
            .find(|(from, _)| *from == name)
            .map_or(name, |(_, to)| to)
    }

    /// Multi-cell props that are views over single-cell props: the kind's
    /// views, and each array item by name (`vertex3`, `point1`).
    pub fn virtual_prop(self, name: &str) -> Option<&'static [&'static str]> {
        let info = self.info();
        if let Some((_, parts)) = info.views.iter().find(|(v, _)| *v == name) {
            return Some(parts);
        }
        info.arrays.iter().find_map(|a| {
            let k: usize = name.strip_prefix(a.item)?.parse().ok()?;
            (1..=a.items.len())
                .contains(&k)
                .then(|| &a.items[k - 1][..])
        })
    }

    /// Array props whose items are points: the prop names of each item's
    /// cells, as many items as the kind can hold. A polygon's live count is
    /// its `numVertices` cell; the builder trims the list.
    pub fn array_prop(self, name: &str) -> Option<&'static [[&'static str; 2]]> {
        self.array(name).map(|a| a.items)
    }

    /// The point-valued virtual prop for item `k` (1-based) of an array prop.
    pub fn array_item_prop(self, name: &str, k: usize) -> Option<String> {
        self.array(name).map(|a| format!("{}{k}", a.item))
    }

    fn array(self, name: &str) -> Option<&'static ArrayProp> {
        self.info().arrays.iter().find(|a| a.names.contains(&name))
    }

    /// The prop a bare `$name` reference resolves to.
    pub fn default_prop(self) -> Option<&'static str> {
        self.info().default_prop
    }

    /// Whether `$name` as a child may produce a copy of this component, and
    /// `<collect componentType>` name the kind.
    pub fn copyable(self) -> bool {
        self.info().flags & COPYABLE != 0
    }

    /// Containers whose children are rendered; `extend` copies them deeply.
    pub fn container(self) -> bool {
        self.info().flags & CONTAINER != 0
    }

    /// Kinds whose prop sources the builder plans from the element's
    /// attributes and children rather than from `PropFrom`.
    pub fn planned(self) -> bool {
        self.info().flags & PLANNED != 0
    }

    /// Function, derivative and answer: planned as math cells.
    pub fn symbolic(self) -> bool {
        self.info().flags & SYMBOLIC != 0
    }

    pub fn sticky_layout(self) -> Option<(crate::tick::snap::Shape, usize, usize)> {
        self.info().sticky
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropFrom {
    /// Value given by an attribute (`PropDef::attr`, or the prop's name).
    Attribute,
    /// Value given by the element's children (a literal or one reference).
    Children,
    /// Value computed by an operator from the `<op>` element's `args`.
    Derived,
    /// Value computed by `op` from the component's own props at `args`.
    Computed { op: OpSpec, args: &'static [u8] },
    /// Value given by the attribute if present, else an alias of the
    /// component's own prop at `alias`.
    AttributeOr { alias: u8 },
    /// Planned by the builder from the whole element (geometric kinds).
    Planned,
}

#[derive(Debug, Clone, Copy)]
pub struct PropDef {
    pub name: &'static str,
    pub default: f64,
    pub from: PropFrom,
    /// Attribute to read instead of `name`, for `PropFrom::Attribute`.
    pub attr: Option<&'static str>,
    /// An attribute whose reference, when present, this prop aliases
    /// (a slider's `bindValueTo`). Takes precedence over `attr`.
    pub bind: Option<&'static str>,
    /// When the attribute is a bare component reference, alias this prop of
    /// the referent instead of its default prop (`function="$m"` wants the
    /// math's `expr`, not its `value`).
    pub ref_prop: Option<&'static str>,
}

impl PropDef {
    pub fn attr_name(&self) -> &'static str {
        self.attr.unwrap_or(self.name)
    }
}
