//! Positions of props in their type's table, for code that sets or reads a
//! prop by position. Each is looked up by name when compiled, so renaming
//! or reordering a table cannot silently break a reader.

use super::PropDef;
use super::types::*;

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
