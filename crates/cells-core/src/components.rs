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
}

const fn attr(name: &'static str, default: f64) -> PropDef {
    PropDef { name, default, from: PropFrom::Attribute, attr: None, bind: None, ref_prop: None }
}

const fn computed(name: &'static str, op: OpSpec, args: &'static [u8]) -> PropDef {
    PropDef { name, default: f64::NAN, from: PropFrom::Computed { op, args }, attr: None, bind: None, ref_prop: None }
}

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
    /* 3 */ PropDef { name: "preliminaryValue", default: 0.0, from: PropFrom::Attribute, attr: Some("initialValue"), bind: Some("bindValueTo"), ref_prop: None },
    /* 4 */ computed("span", OpSpec::Sub, &[1, 0]),
    /* 5 */ computed("spanSteps", OpSpec::Div, &[4, 2]),
    /* 6 */ computed("spanStepsEps", OpSpec::Offset { k: 1e-10 }, &[5]),
    /* 7 */ computed("maxIndex", OpSpec::Floor, &[6]),
    /* 8 */ computed("offset", OpSpec::Sub, &[3, 0]),
    /* 9 */ computed("rawIndex", OpSpec::Div, &[8, 2]),
    /* 10 */ computed("roundedIndex", OpSpec::Round, &[9]),
    /* 11 */ computed("nonNegIndex", OpSpec::Clamp { lo: 0.0, hi: f64::INFINITY }, &[10]),
    /* 12 */ computed("clampedIndex", OpSpec::Min, &[11, 7]),
    // A non-finite stored value is index 0, so the slider shows `from`,
    // as in the current core.
    /* 13 */ computed("index", OpSpec::NanTo { k: 0.0 }, &[12]),
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
    /* 8 */ PropDef { name: "length", default: f64::NAN, from: PropFrom::AttributeOr { alias: 7 }, attr: None, bind: None, ref_prop: None },
    /* 9 */ computed("lengthFloor", OpSpec::Floor, &[8]),
    /* 10 */ computed("lengthNonNeg", OpSpec::Clamp { lo: 0.0, hi: f64::INFINITY }, &[9]),
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
    pub const ALL: [ComponentKind; 13] = [
        Self::Document, Self::Graph, Self::Point, Self::Number, Self::NumberInput, Self::Op, Self::Slider,
        Self::RepeatForSequence, Self::Collect, Self::SequenceValue, Self::BooleanInput, Self::Math, Self::Evaluate,
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
        }
    }

    /// Single-cell props, in declaration order.
    pub fn prop_defs(self) -> &'static [PropDef] {
        const GRAPH: &[PropDef] = &[attr("xmin", -10.0), attr("xmax", 10.0), attr("ymin", -10.0), attr("ymax", 10.0)];
        // `hide` is a boolean in the current core; here it is a 0/1 cell.
        const POINT: &[PropDef] = &[attr("x", 0.0), attr("y", 0.0), attr("hide", 0.0)];
        const BOOLEAN_INPUT: &[PropDef] = &[attr("value", 0.0)];
        // Both of a math's props are set by the builder from its children.
        const MATH: &[PropDef] = &[
            PropDef { name: "expr", default: f64::NAN, from: PropFrom::Children, attr: None, bind: None, ref_prop: None },
            PropDef { name: "value", default: f64::NAN, from: PropFrom::Children, attr: None, bind: None, ref_prop: None },
        ];
        const EVALUATE: &[PropDef] = &[
            PropDef { name: "function", default: f64::NAN, from: PropFrom::Attribute, attr: None, bind: None, ref_prop: Some("expr") },
            attr("input", f64::NAN),
            computed("value", OpSpec::EvalAt, &[0, 1]),
        ];
        const NUMBER: &[PropDef] = &[PropDef { name: "value", default: f64::NAN, from: PropFrom::Children, attr: None, bind: None, ref_prop: None }];
        const NUMBER_INPUT: &[PropDef] = &[attr("value", f64::NAN)];
        const OP: &[PropDef] = &[PropDef { name: "value", default: f64::NAN, from: PropFrom::Derived, attr: None, bind: None, ref_prop: None }];
        const COLLECT: &[PropDef] = &[attr("count", 0.0)];
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
        }
    }

    pub fn prop_index(self, name: &str) -> Option<usize> {
        self.prop_defs().iter().position(|p| p.name == name)
    }

    /// Multi-cell props that are views over single-cell props.
    pub fn virtual_prop(self, name: &str) -> Option<&'static [&'static str]> {
        match (self, name) {
            (Self::Point, "coords") => Some(&["x", "y"]),
            _ => None,
        }
    }

    /// The prop a bare `$name` reference resolves to.
    pub fn default_prop(self) -> Option<&'static str> {
        match self {
            Self::Point => Some("coords"),
            Self::Number | Self::NumberInput | Self::Op | Self::Slider | Self::SequenceValue | Self::BooleanInput | Self::Math | Self::Evaluate => Some("value"),
            Self::Document | Self::Graph | Self::RepeatForSequence | Self::Collect => None,
        }
    }

    /// Whether `$name` as a child may produce a copy of this component.
    pub fn copyable(self) -> bool {
        !matches!(self, Self::Document | Self::Graph | Self::RepeatForSequence | Self::Collect)
    }

    /// Whether `<collect componentType="...">` may name this kind.
    pub fn collectable(self) -> bool {
        self.copyable()
    }

    /// Whether the component contributes nodes to the rendered tree.
    pub fn rendered(self) -> bool {
        !matches!(self, Self::Op)
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
