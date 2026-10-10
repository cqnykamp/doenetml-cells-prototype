//! Stripped-down component definitions: which props each tag has, where each
//! prop's value comes from, and its default. This is the only place that
//! knows DoenetML tag vocabulary.
//!
//! A component's props are its slots; a prop may be computed from the
//! component's other props by an operator (`PropFrom::Computed`), which is how
//! a component such as `<slider>` describes its whole value chain without any
//! code of its own: the chain is data, and inversion through it is the
//! generic operator inverse.
//!
//! This file holds the types and the lookups the build uses; `types.rs` holds
//! the data (every type's prop table and its row in [`COMPONENT_TYPES`]); `prop.rs`
//! names prop positions for code that reads or sets a prop by position.

use crate::program::OpSpec;

pub mod prop;
mod types;

pub use types::COMPONENT_TYPES;

/// `repr(u8)` so the renderer can read the type column as a byte array; the
/// discriminant order matches `ComponentType::ALL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ComponentType {
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
    /// how the circle is specified (ADR 0006). Chains are planned in `build/compile/geometry/`.
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
    /// dragged (ADR 0007). `threshold` NaN means the default.
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
    /// string id of its text (ADR 0009). A cell's meaning is a property of
    /// the operators around it, so a text value never reaches a numeric one.
    Text = 25,
    /// `<conditionalContent>`, a reactive choice (ADR 0009). Its
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
    /// wired after expansion (`build/expand/scoring.rs`).
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

/// What the core knows about a type apart from how it is planned and
/// expanded: one row per type in `COMPONENT_TYPES`, indexed by discriminant.
pub struct ComponentTypeInfo {
    pub component_type: ComponentType,
    /// The tag the type shows as, then other tags that make it.
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
/// coordinate props, as many as the type can hold.
pub struct ArrayProp {
    pub names: &'static [&'static str],
    pub item: &'static str,
    pub items: &'static [[&'static str; 2]],
}

/// `$name` as a child may produce a copy of the component, and
/// `<collect componentType>` may name the type.
pub const COPYABLE: u8 = 1;
/// The children are rendered; `extend` copies them deeply.
pub const CONTAINER: u8 = 2;
/// The builder plans the props from the element's attributes and children
/// rather than from `PropFrom` (the geometric types).
pub const PLANNED: u8 = 4;
/// Planned as math cells (`plan_symbolic`); not allowed in a branch
/// interface.
pub const SYMBOLIC: u8 = 8;
/// Made by the builder, never by a tag in the source.
pub const INTERNAL: u8 = 16;

impl ComponentTypeInfo {
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

impl ComponentType {
    pub const ALL: [ComponentType; 31] = {
        let mut out = [ComponentType::Document; 31];
        let mut i = 0;
        while i < COMPONENT_TYPES.len() {
            out[i] = COMPONENT_TYPES[i].component_type;
            i += 1;
        }
        out
    };

    pub fn info(self) -> &'static ComponentTypeInfo {
        &COMPONENT_TYPES[self as usize]
    }

    pub fn from_tag(tag: &str) -> Option<Self> {
        static BY_TAG: std::sync::OnceLock<std::collections::HashMap<&'static str, ComponentType>> =
            std::sync::OnceLock::new();
        let by_tag = BY_TAG.get_or_init(|| {
            COMPONENT_TYPES
                .iter()
                .filter(|k| k.flags & INTERNAL == 0)
                .flat_map(|k| k.tags.iter().map(move |&t| (t, k.component_type)))
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

    /// Multi-cell props that are views over single-cell props: the type's
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
    /// cells, as many items as the type can hold. A polygon's live count is
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
    /// `<collect componentType>` name the type.
    pub fn copyable(self) -> bool {
        self.info().flags & COPYABLE != 0
    }

    /// Containers whose children are rendered; `extend` copies them deeply.
    pub fn container(self) -> bool {
        self.info().flags & CONTAINER != 0
    }

    /// Types whose prop sources the builder plans from the element's
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
    /// Planned by the builder from the whole element (geometric types).
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
