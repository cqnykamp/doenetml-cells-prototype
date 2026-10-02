//! A minimal mirror of the normalized DAST emitted by `@doenet/parser`.
//! Only the node kinds this prototype consumes are modeled; anything else
//! becomes [`DastNode::Other`] and is ignored.
//!
//! Deserialization is hand-written (see the bottom of this file) because a
//! serde internally tagged enum buffers every node before dispatching on
//! `type`, which roughly doubles deserialization time on large documents.

use serde::Deserialize;

#[derive(Debug)]
pub struct DastRoot {
    pub children: Vec<DastNode>,
}

#[derive(Debug)]
pub enum DastNode {
    Element(DastElement),
    Text(DastText),
    Macro(DastMacro),
    Other,
}

#[derive(Debug)]
pub struct DastElement {
    pub name: String,
    pub attributes: Vec<(String, DastAttribute)>,
    pub children: Vec<DastNode>,
}

impl DastElement {
    pub fn attr(&self, name: &str) -> Option<&DastAttribute> {
        self.attributes.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }
    pub fn has_attr(&self, name: &str) -> bool {
        self.attr(name).is_some()
    }
}

#[derive(Debug)]
pub struct DastText {
    pub value: String,
}

#[derive(Debug)]
pub struct DastAttribute {
    pub children: Vec<DastNode>,
}

#[derive(Debug)]
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

// ---- deserialization --------------------------------------------------------
//
// Nodes are deserialized in a single pass with a map visitor that reads each
// field into a typed local and builds the enum at the end. This avoids both
// serde's internally tagged buffering and a second tree conversion pass; the
// latter scatters nodes across the heap and slows every later traversal.

impl<'de> Deserialize<'de> for DastRoot {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            children: Vec<DastNode>,
        }
        Raw::deserialize(d).map(|r| DastRoot { children: r.children })
    }
}

impl<'de> Deserialize<'de> for DastAttribute {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            children: Vec<DastNode>,
        }
        Raw::deserialize(d).map(|r| DastAttribute { children: r.children })
    }
}

struct Attributes(Vec<(String, DastAttribute)>);

impl<'de> Deserialize<'de> for Attributes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Attributes;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an attributes object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut v = Vec::with_capacity(m.size_hint().unwrap_or(0));
                while let Some((k, a)) = m.next_entry::<String, DastAttribute>()? {
                    v.push((k, a));
                }
                Ok(Attributes(v))
            }
        }
        d.deserialize_map(V)
    }
}

impl<'de> Deserialize<'de> for DastNode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = DastNode;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a DAST node object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut kind: Option<String> = None;
                let mut name: Option<String> = None;
                let mut value: Option<String> = None;
                let mut path: Option<Vec<PathPart>> = None;
                let mut attributes: Option<Attributes> = None;
                let mut children: Option<Vec<DastNode>> = None;
                while let Some(key) = m.next_key::<std::borrow::Cow<str>>()? {
                    match key.as_ref() {
                        "type" => kind = Some(m.next_value()?),
                        "name" => name = Some(m.next_value()?),
                        "value" => value = Some(m.next_value()?),
                        "path" => path = Some(m.next_value()?),
                        "attributes" => attributes = Some(m.next_value()?),
                        "children" => children = Some(m.next_value()?),
                        _ => {
                            m.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(match kind.as_deref().unwrap_or("") {
                    "element" => DastNode::Element(DastElement {
                        name: name.unwrap_or_default(),
                        attributes: attributes.map(|a| a.0).unwrap_or_default(),
                        children: children.unwrap_or_default(),
                    }),
                    "text" => DastNode::Text(DastText { value: value.unwrap_or_default() }),
                    "macro" => DastNode::Macro(DastMacro { path: path.unwrap_or_default() }),
                    _ => DastNode::Other,
                })
            }
        }
        d.deserialize_map(V)
    }
}

pub fn parse_json(json: &str) -> crate::Result<DastRoot> {
    Ok(serde_json::from_str(json)?)
}
