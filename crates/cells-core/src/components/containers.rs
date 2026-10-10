//! Document, graph, p, setup, group, sticky group and section: types that
//! hold other components. Document and section also hold credit.

use super::define::*;
use super::{ComponentTypeInfo, PropDef};

/// The root. `creditAchieved` is wired after expansion
/// (`build/expand/scoring.rs`).
pub(super) const DOCUMENT: ComponentTypeInfo = info(&["document"], DOCUMENT_PROPS);

const DOCUMENT_PROPS: &[PropDef] = &props([
    planned("creditAchieved"),
    computed(
        "percentCreditAchieved",
        OpSpec::Scale { k: 100.0 },
        &["creditAchieved"],
    ),
]);

pub mod document {
    use super::*;
    pub const CREDIT: usize = at(DOCUMENT_PROPS, "creditAchieved");
    pub const PERCENT_CREDIT: usize = at(DOCUMENT_PROPS, "percentCreditAchieved");
}

/// `<graph>`: its x and y range.
pub(super) const GRAPH: ComponentTypeInfo = info(&["graph"], GRAPH_PROPS).container();

const GRAPH_PROPS: &[PropDef] = &props([
    attr("xmin", -10.0),
    attr("xmax", 10.0),
    attr("ymin", -10.0),
    attr("ymax", 10.0),
]);

pub mod graph {
    use super::*;
    pub const XMIN: usize = at(GRAPH_PROPS, "xmin");
    pub const XMAX: usize = at(GRAPH_PROPS, "xmax");
    pub const YMIN: usize = at(GRAPH_PROPS, "ymin");
    pub const YMAX: usize = at(GRAPH_PROPS, "ymax");
}

/// `<p>`: a rendered container with no props of its own.
pub(super) const P: ComponentTypeInfo = info(&["p"], &[]).container();

/// `<setup>`: an unrendered container.
pub(super) const SETUP: ComponentTypeInfo = info(&["setup"], &[]).container();

/// `<group>`: a rendered container with no props of its own. The
/// prototype renders `<label>` the same way.
pub(super) const GROUP: ComponentTypeInfo = info(&["group", "label"], &[]).container();

/// `<stickyGroup>`: a container whose members snap to one another when
/// dragged (ADR 0007). `threshold` NaN means the default.
pub(super) const STICKY_GROUP: ComponentTypeInfo =
    info(&["stickyGroup"], STICKY_GROUP_PROPS).container();

const STICKY_GROUP_PROPS: &[PropDef] = &props([
    attr("threshold", f64::NAN),
    attr("relativeToGraphScales", 0.0),
]);

pub mod sticky_group {
    use super::*;
    pub const THRESHOLD: usize = at(STICKY_GROUP_PROPS, "threshold");
    pub const RELATIVE: usize = at(STICKY_GROUP_PROPS, "relativeToGraphScales");
}

/// `<section>`, `<subsection>`, `<subsubsection>`, `<problem>`,
/// `<exercise>`, `<example>`: a rendered container that is numbered
/// among its sibling sections and, when it aggregates scores, holds the
/// weighted credit of the answers and sections inside it. Both are
/// wired after expansion (`build/expand/scoring.rs`). The parser writes
/// `<section>` as `<division type="section">`.
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

// `creditAchieved` and `number` are wired after expansion; the
// flags are literals the builder reads, since they decide the wiring.
const SECTION_PROPS: &[PropDef] = &props([
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
