//! A minimal serde mirror of the normalized DAST emitted by `@doenet/parser`.
//! Only the node kinds this prototype consumes are modeled; anything else
//! deserializes to [`DastNode::Other`] and is ignored.

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct DastRoot {
    #[serde(default)]
    pub children: Vec<DastNode>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DastNode {
    Element(DastElement),
    Text(DastText),
    Macro(DastMacro),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct DastElement {
    pub name: String,
    #[serde(default)]
    pub attributes: HashMap<String, DastAttribute>,
    #[serde(default)]
    pub children: Vec<DastNode>,
}

#[derive(Debug, Deserialize)]
pub struct DastText {
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct DastAttribute {
    #[serde(default)]
    pub children: Vec<DastNode>,
}

#[derive(Debug, Deserialize)]
pub struct DastMacro {
    pub path: Vec<PathPart>,
}

#[derive(Debug, Deserialize)]
pub struct PathPart {
    pub name: String,
}

impl DastMacro {
    pub fn display(&self) -> String {
        self.path.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(".")
    }
}

pub fn parse_json(json: &str) -> crate::Result<DastRoot> {
    Ok(serde_json::from_str(json)?)
}
