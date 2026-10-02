//! Stripped-down component definitions: which props each tag has, where each
//! prop's value comes from, and its default. This is the only place that
//! knows DoenetML tag vocabulary.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    Document,
    Graph,
    Point,
    Number,
    NumberInput,
    /// Prototype-only tag that applies a numeric operator to referenced cells.
    Op,
}

impl ComponentKind {
    pub fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "document" => Self::Document,
            "graph" => Self::Graph,
            "point" => Self::Point,
            "number" => Self::Number,
            "numberInput" => Self::NumberInput,
            "op" => Self::Op,
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
        }
    }

    /// Single-cell props, in declaration order.
    pub fn prop_defs(self) -> &'static [PropDef] {
        match self {
            Self::Document => &[],
            Self::Graph => &[
                PropDef { name: "xmin", default: -10.0, from: PropFrom::Attribute },
                PropDef { name: "xmax", default: 10.0, from: PropFrom::Attribute },
                PropDef { name: "ymin", default: -10.0, from: PropFrom::Attribute },
                PropDef { name: "ymax", default: 10.0, from: PropFrom::Attribute },
            ],
            Self::Point => &[
                PropDef { name: "x", default: 0.0, from: PropFrom::Attribute },
                PropDef { name: "y", default: 0.0, from: PropFrom::Attribute },
            ],
            Self::Number => &[PropDef { name: "value", default: f64::NAN, from: PropFrom::Children }],
            Self::NumberInput => &[PropDef { name: "value", default: f64::NAN, from: PropFrom::Attribute }],
            Self::Op => &[PropDef { name: "value", default: f64::NAN, from: PropFrom::Derived }],
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
            Self::Number | Self::NumberInput | Self::Op => Some("value"),
            Self::Document | Self::Graph => None,
        }
    }

    /// Whether `$name` as a child may produce a copy of this component.
    pub fn copyable(self) -> bool {
        !matches!(self, Self::Document | Self::Graph)
    }

    /// Whether the component contributes nodes to the rendered tree.
    pub fn rendered(self) -> bool {
        !matches!(self, Self::Op)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropFrom {
    /// Value given by the attribute of the same name.
    Attribute,
    /// Value given by the element's children (a literal or one reference).
    Children,
    /// Value computed by an operator from `args`.
    Derived,
}

#[derive(Debug, Clone, Copy)]
pub struct PropDef {
    pub name: &'static str,
    pub default: f64,
    pub from: PropFrom,
}
