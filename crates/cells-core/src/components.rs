//! Stripped-down component definitions: which props each tag has, where each
//! prop's value comes from, and its default. This is the only place that
//! knows DoenetML tag vocabulary.
//!
//! A component's props are its slots; a prop may be computed from the
//! component's other props by an operator (`PropFrom::Computed`), which is how
//! a component such as `<slider>` describes its whole value chain without any
//! code of its own: the chain is data, and inversion through it is the
//! generic operator inverse.

use crate::ops::OpSpec;

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
    /// `<math>`: `expr` is a fixed handle into the expression arena; `value`
    /// is the lowered numeric chain when the expression has no free symbols,
    /// else an `Evaluate` of the handle (NaN).
    Math = 11,
    /// `<evaluate function="$m" input="$a"/>`: the math's expression with
    /// its free symbol set to the input.
    Evaluate = 12,
    /// `<mathInput>`: a numeric input whose value may be bound to another
    /// cell by a child reference or `bindValueTo`.
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
}

/// Largest polygon the fixed prop layout holds.
pub const MAX_VERTICES: usize = 16;

const fn attr(name: &'static str, default: f64) -> PropDef {
    PropDef { name, default, from: PropFrom::Attribute, attr: None, bind: None, ref_prop: None }
}

const fn computed(name: &'static str, op: OpSpec, args: &'static [u8]) -> PropDef {
    PropDef { name, default: f64::NAN, from: PropFrom::Computed { op, args }, attr: None, bind: None, ref_prop: None }
}

/// A prop whose source the builder plans from the element's specification.
const fn planned(name: &'static str) -> PropDef {
    PropDef { name, default: f64::NAN, from: PropFrom::Planned, attr: None, bind: None, ref_prop: None }
}

/// `<circle>`. `cx`, `cy`, `radius` are the numerical center and radius;
/// how they are produced depends on which of `center`, `radius` and
/// `through` the author gave (see `plan_circle` in `build.rs`).
const CIRCLE_PROPS: &[PropDef] = &[
    /* 0 */ planned("cx"),
    /* 1 */ planned("cy"),
    /* 2 */ planned("radius"),
    /* 3 */ computed("diameter", OpSpec::Scale { k: 2.0 }, &[2]),
    /* 4 */ computed("circumference", OpSpec::Scale { k: std::f64::consts::TAU }, &[2]),
    /* 5 */ planned("area"),
    // The center as a *reference*: the prescribed center when there is one
    // (so a point extending `$c.center` drags that point alone, as in the
    // current core), else the derived center.
    /* 6 */ planned("centerX"),
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

const LINE_SEGMENT_PROPS: &[PropDef] = &[/* 0 */ planned("x1"), /* 1 */ planned("y1"), /* 2 */ planned("x2"), /* 3 */ planned("y2")];

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
    const NAMES: [[&str; 2]; MAX_VERTICES] = [
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
    const fn build() -> [PropDef; 1 + 2 * MAX_VERTICES] {
        let mut out = [planned(""); 1 + 2 * MAX_VERTICES];
        out[0] = planned("numVertices");
        let mut i = 0;
        while i < MAX_VERTICES {
            out[1 + 2 * i] = planned(NAMES[i][0]);
            out[2 + 2 * i] = planned(NAMES[i][1]);
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
    PropDef { name: "preliminaryValue", default: 0.0, from: PropFrom::Attribute, attr: Some("initialValue"), bind: Some("bindValueTo"), ref_prop: None },
    /* 4 */ computed("span", OpSpec::Sub, &[1, 0]),
    /* 5 */ computed("spanSteps", OpSpec::Div, &[4, 2]),
    /* 6 */ computed("spanStepsEps", OpSpec::Offset { k: 1e-10 }, &[5]),
    /* 7 */ computed("maxIndex", OpSpec::Floor, &[6]),
    /* 8 */ computed("offset", OpSpec::Sub, &[3, 0]),
    /* 9 */ computed("rawIndex", OpSpec::Div, &[8, 2]),
    /* 10 */ computed("roundedIndex", OpSpec::Round, &[9]),
    /* 11 */
    computed("nonNegIndex", OpSpec::Clamp { lo: 0.0, hi: f64::INFINITY }, &[10]),
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
    PropDef { name: "length", default: f64::NAN, from: PropFrom::AttributeOr { alias: 7 }, attr: None, bind: None, ref_prop: None },
    /* 9 */ computed("lengthFloor", OpSpec::Floor, &[8]),
    /* 10 */
    computed("lengthNonNeg", OpSpec::Clamp { lo: 0.0, hi: f64::INFINITY }, &[9]),
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

impl ComponentKind {
    pub const ALL: [ComponentKind; 22] = [
        Self::Document,
        Self::Graph,
        Self::Point,
        Self::Number,
        Self::NumberInput,
        Self::Op,
        Self::Slider,
        Self::RepeatForSequence,
        Self::Collect,
        Self::SequenceValue,
        Self::BooleanInput,
        Self::Math,
        Self::Evaluate,
        Self::MathInput,
        Self::Circle,
        Self::Line,
        Self::LineSegment,
        Self::Polygon,
        Self::PointList,
        Self::P,
        Self::Setup,
        Self::StickyGroup,
    ];

    pub fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "document" => Self::Document,
            "graph" => Self::Graph,
            "point" => Self::Point,
            "number" => Self::Number,
            "numberInput" => Self::NumberInput,
            "op" => Self::Op,
            "slider" => Self::Slider,
            "repeatForSequence" => Self::RepeatForSequence,
            "collect" => Self::Collect,
            "booleanInput" => Self::BooleanInput,
            "math" => Self::Math,
            "evaluate" => Self::Evaluate,
            "mathInput" => Self::MathInput,
            "circle" => Self::Circle,
            "line" => Self::Line,
            "lineSegment" => Self::LineSegment,
            "polygon" | "triangle" => Self::Polygon,
            "pointList" => Self::PointList,
            "p" => Self::P,
            "setup" => Self::Setup,
            "stickyGroup" => Self::StickyGroup,
            _ => return None,
        })
    }

    pub fn tag(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Graph => "graph",
            Self::Point => "point",
            Self::Number => "number",
            Self::NumberInput => "numberInput",
            Self::Op => "op",
            Self::Slider => "slider",
            Self::RepeatForSequence => "repeatForSequence",
            Self::Collect => "collect",
            Self::SequenceValue => "sequenceValue",
            Self::BooleanInput => "booleanInput",
            Self::Math => "math",
            Self::Evaluate => "evaluate",
            Self::MathInput => "mathInput",
            Self::Circle => "circle",
            Self::Line => "line",
            Self::LineSegment => "lineSegment",
            Self::Polygon => "polygon",
            Self::PointList => "pointList",
            Self::P => "p",
            Self::Setup => "setup",
            Self::StickyGroup => "stickyGroup",
        }
    }

    /// Single-cell props, in declaration order.
    pub fn prop_defs(self) -> &'static [PropDef] {
        const GRAPH: &[PropDef] = &[attr("xmin", -10.0), attr("xmax", 10.0), attr("ymin", -10.0), attr("ymax", 10.0)];
        // `hide` is a boolean in the current core; here it is a 0/1 cell.
        const POINT: &[PropDef] = &[planned("x"), planned("y"), planned("hide")];
        const BOOLEAN_INPUT: &[PropDef] = &[attr("value", 0.0)];
        // Both of a math's props are set by the builder from its children.
        const MATH: &[PropDef] = &[
            PropDef { name: "expr", default: f64::NAN, from: PropFrom::Children, attr: None, bind: None, ref_prop: None },
            PropDef { name: "value", default: f64::NAN, from: PropFrom::Children, attr: None, bind: None, ref_prop: None },
        ];
        const EVALUATE: &[PropDef] =
            &[PropDef { name: "function", default: f64::NAN, from: PropFrom::Attribute, attr: None, bind: None, ref_prop: Some("expr") }, attr("input", f64::NAN), computed("value", OpSpec::EvalAt, &[0, 1])];
        const NUMBER: &[PropDef] = &[PropDef { name: "value", default: f64::NAN, from: PropFrom::Children, attr: None, bind: None, ref_prop: None }];
        const NUMBER_INPUT: &[PropDef] = &[attr("value", f64::NAN)];
        // A mathInput's value is bound by a child reference or `bindValueTo`,
        // else it is the `prefill` (the builder reads it).
        const MATH_INPUT: &[PropDef] = &[PropDef { name: "value", default: f64::NAN, from: PropFrom::Children, attr: Some("prefill"), bind: Some("bindValueTo"), ref_prop: None }];
        const OP: &[PropDef] = &[PropDef { name: "value", default: f64::NAN, from: PropFrom::Derived, attr: None, bind: None, ref_prop: None }];
        const COLLECT: &[PropDef] = &[attr("count", 0.0)];
        const STICKY_GROUP: &[PropDef] = &[attr("threshold", f64::NAN), attr("relativeToGraphScales", 0.0)];
        match self {
            Self::Document => &[],
            Self::Graph => GRAPH,
            Self::Point => POINT,
            Self::Number => NUMBER,
            Self::NumberInput => NUMBER_INPUT,
            Self::Op => OP,
            Self::Slider => SLIDER_PROPS,
            Self::RepeatForSequence => REPEAT_PROPS,
            Self::Collect => COLLECT,
            Self::SequenceValue => SEQUENCE_VALUE_PROPS,
            Self::BooleanInput => BOOLEAN_INPUT,
            Self::Math => MATH,
            Self::Evaluate => EVALUATE,
            Self::MathInput => MATH_INPUT,
            Self::Circle => CIRCLE_PROPS,
            Self::Line => LINE_PROPS,
            Self::LineSegment => LINE_SEGMENT_PROPS,
            Self::Polygon => POLYGON_PROPS,
            Self::StickyGroup => STICKY_GROUP,
            Self::PointList | Self::P | Self::Setup => &[],
        }
    }

    pub fn prop_index(self, name: &str) -> Option<usize> {
        let name = self.canonical_prop(name);
        self.prop_defs().iter().position(|p| p.name == name)
    }

    /// The current core's spellings of a few props, accepted in references
    /// so its documents resolve unchanged.
    pub fn canonical_prop<'a>(self, name: &'a str) -> &'a str {
        match (self, name) {
            (Self::Circle, "centerX1") => "centerX",
            (Self::Circle, "centerX2") => "centerY",
            (Self::Circle, "throughPointX1_1") => "throughX1",
            (Self::Circle, "throughPointX1_2") => "throughY1",
            (Self::Circle, "throughPointX2_1") => "throughX2",
            (Self::Circle, "throughPointX2_2") => "throughY2",
            (Self::Circle, "throughPointX3_1") => "throughX3",
            (Self::Circle, "throughPointX3_2") => "throughY3",
            _ => name,
        }
    }

    /// Multi-cell props that are views over single-cell props.
    pub fn virtual_prop(self, name: &str) -> Option<&'static [&'static str]> {
        match (self, name) {
            (Self::Point, "coords") => Some(&["x", "y"]),
            (Self::Circle, "center") => Some(&["centerX", "centerY"]),
            (Self::Circle, "numericalCenter") => Some(&["cx", "cy"]),
            (Self::Circle, "throughPoint1") => Some(&["throughX1", "throughY1"]),
            (Self::Circle, "throughPoint2") => Some(&["throughX2", "throughY2"]),
            (Self::Circle, "throughPoint3") => Some(&["throughX3", "throughY3"]),
            (Self::Line | Self::LineSegment, "point1") => Some(&["x1", "y1"]),
            (Self::Line | Self::LineSegment, "point2") => Some(&["x2", "y2"]),
            (Self::Polygon, v) if v.starts_with("vertex") => {
                let k: usize = v["vertex".len()..].parse().ok()?;
                (1..=MAX_VERTICES).contains(&k).then(|| &VERTEX_PARTS[k - 1][..])
            }
            _ => None,
        }
    }

    /// Array props whose items are points: the prop names of each item's
    /// cells, as many items as the kind can hold. A polygon's live count is
    /// its `numVertices` cell; the builder trims the list.
    pub fn array_prop(self, name: &str) -> Option<Vec<[&'static str; 2]>> {
        match (self, name) {
            (Self::Line | Self::LineSegment, "points" | "endpoints") => Some(vec![["x1", "y1"], ["x2", "y2"]]),
            (Self::Polygon, "vertices") => Some(POLYGON_PROPS[1..].chunks(2).map(|c| [c[0].name, c[1].name]).collect()),
            (Self::Circle, "throughPoints") => Some(vec![["throughX1", "throughY1"], ["throughX2", "throughY2"], ["throughX3", "throughY3"]]),
            _ => None,
        }
    }

    /// The point-valued virtual prop for item `k` (1-based) of an array prop.
    pub fn array_item_prop(self, name: &str, k: usize) -> Option<String> {
        match (self, name) {
            (Self::Line | Self::LineSegment, "points" | "endpoints") => Some(format!("point{k}")),
            (Self::Polygon, "vertices") => Some(format!("vertex{k}")),
            (Self::Circle, "throughPoints") => Some(format!("throughPoint{k}")),
            _ => None,
        }
    }

    /// The prop a bare `$name` reference resolves to.
    pub fn default_prop(self) -> Option<&'static str> {
        match self {
            Self::Point => Some("coords"),
            Self::Number | Self::NumberInput | Self::Op | Self::Slider | Self::SequenceValue | Self::BooleanInput | Self::Math | Self::Evaluate | Self::MathInput => Some("value"),
            Self::Document | Self::Graph | Self::RepeatForSequence | Self::Collect | Self::Circle | Self::Line | Self::LineSegment | Self::Polygon | Self::PointList | Self::P | Self::Setup | Self::StickyGroup => None,
        }
    }

    /// Whether `$name` as a child may produce a copy of this component.
    pub fn copyable(self) -> bool {
        !matches!(self, Self::Document | Self::Graph | Self::RepeatForSequence | Self::Collect | Self::PointList | Self::P | Self::Setup | Self::StickyGroup)
    }

    /// Containers whose children are rendered; `extend` copies them deeply.
    pub fn container(self) -> bool {
        matches!(self, Self::Graph | Self::P | Self::Setup | Self::StickyGroup)
    }

    /// How a member of a sticky group attracts and snaps: its shape, the
    /// prop of its first coordinate, and how many points it has at most (a
    /// polygon's live count is its `numVertices` cell). None for kinds that
    /// do not take part.
    pub fn sticky_layout(self) -> Option<(crate::sticky::Shape, usize, usize)> {
        use crate::sticky::Shape;
        match self {
            Self::Point => Some((Shape::Point, 0, 1)),
            Self::LineSegment => Some((Shape::Open, 0, 2)),
            Self::Polygon => Some((Shape::Closed, 1, MAX_VERTICES)),
            _ => None,
        }
    }

    /// Kinds whose prop sources the builder plans from the element's
    /// attributes and children rather than from `PropFrom`.
    pub fn planned(self) -> bool {
        matches!(self, Self::Point | Self::Circle | Self::Line | Self::LineSegment | Self::Polygon)
    }

    /// Whether `<collect componentType="...">` may name this kind.
    pub fn collectable(self) -> bool {
        self.copyable()
    }

    /// Whether the component contributes nodes to the rendered tree.
    pub fn rendered(self) -> bool {
        !matches!(self, Self::Op | Self::Setup)
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
