//! A flat, columnar DAST. Nodes live in parallel arrays indexed by `NodeId`;
//! every string (tag names, attribute names, text, reference path parts)
//! lives once in a shared [`StringTable`]. Two loaders fill the same
//! structure: [`Dast::from_json`] for the JSON emitted by `@doenet/parser`,
//! and [`Dast::from_binary`] for the compact wire format written by
//! `scripts/cdast-encode.mjs` (see `docs/adr/0002-binary-wire-format.md`).
//!
//! Node 0 is a synthetic root element whose children are the document's
//! top-level nodes. Macros (`$a.b`) reuse the attribute range fields to index
//! the `path` array instead.

use std::collections::HashMap;

pub type NodeId = u32;
pub type AttrId = u32;
pub type StrId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NodeKind {
    Element = 0,
    Text = 1,
    Macro = 2,
    Other = 3,
}

impl NodeKind {
    fn from_u8(b: u8) -> NodeKind {
        match b {
            0 => NodeKind::Element,
            1 => NodeKind::Text,
            2 => NodeKind::Macro,
            _ => NodeKind::Other,
        }
    }
}

/// Deduplicated strings: `bytes[offsets[i]..offsets[i+1]]` is string `i`.
#[derive(Debug, Clone, Default)]
pub struct StringTable {
    pub offsets: Vec<u32>,
    pub bytes: Vec<u8>,
}

impl StringTable {
    pub fn new() -> Self {
        StringTable { offsets: vec![0], bytes: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn get(&self, i: StrId) -> &str {
        let (a, b) = (self.offsets[i as usize] as usize, self.offsets[i as usize + 1] as usize);
        // Validated once at load (binary) or produced from `str` (JSON).
        unsafe { std::str::from_utf8_unchecked(&self.bytes[a..b]) }
    }

    /// Append without deduplication.
    pub fn push(&mut self, s: &str) -> StrId {
        self.bytes.extend_from_slice(s.as_bytes());
        self.offsets.push(self.bytes.len() as u32);
        (self.offsets.len() - 2) as StrId
    }

    pub fn heap_bytes(&self) -> usize {
        self.offsets.capacity() * 4 + self.bytes.capacity()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Dast {
    pub strings: StringTable,
    kind: Vec<u8>,
    /// element: tag string; text: value string; macro/other: unused
    str_: Vec<StrId>,
    /// element: attribute range; macro: path range
    a_start: Vec<u32>,
    a_count: Vec<u32>,
    c_start: Vec<u32>,
    c_count: Vec<u32>,
    attr_name: Vec<StrId>,
    attr_c_start: Vec<u32>,
    attr_c_count: Vec<u32>,
    children: Vec<NodeId>,
    path: Vec<StrId>,
}

impl Dast {
    pub const ROOT: NodeId = 0;

    pub fn len(&self) -> usize {
        self.kind.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kind.is_empty()
    }

    #[inline]
    pub fn kind(&self, n: NodeId) -> NodeKind {
        NodeKind::from_u8(self.kind[n as usize])
    }

    /// Tag of an element or value of a text node.
    #[inline]
    pub fn str(&self, n: NodeId) -> &str {
        self.strings.get(self.str_[n as usize])
    }

    #[inline]
    pub fn str_id(&self, n: NodeId) -> StrId {
        self.str_[n as usize]
    }

    #[inline]
    pub fn children(&self, n: NodeId) -> &[NodeId] {
        let (s, c) = (self.c_start[n as usize] as usize, self.c_count[n as usize] as usize);
        &self.children[s..s + c]
    }

    pub fn attrs(&self, n: NodeId) -> std::ops::Range<AttrId> {
        let s = self.a_start[n as usize];
        s..s + self.a_count[n as usize]
    }

    #[inline]
    pub fn attr_name(&self, a: AttrId) -> &str {
        self.strings.get(self.attr_name[a as usize])
    }

    #[inline]
    pub fn attr_children(&self, a: AttrId) -> &[NodeId] {
        let (s, c) = (self.attr_c_start[a as usize] as usize, self.attr_c_count[a as usize] as usize);
        &self.children[s..s + c]
    }

    pub fn attr(&self, n: NodeId, name: &str) -> Option<AttrId> {
        self.attrs(n).find(|&a| self.attr_name(a) == name)
    }

    /// Path parts of a macro node, as string ids.
    #[inline]
    pub fn macro_path(&self, n: NodeId) -> &[StrId] {
        let (s, c) = (self.a_start[n as usize] as usize, self.a_count[n as usize] as usize);
        &self.path[s..s + c]
    }

    pub fn macro_display(&self, n: NodeId) -> String {
        self.macro_path(n).iter().map(|&p| self.strings.get(p)).collect::<Vec<_>>().join(".")
    }

    pub fn heap_bytes(&self) -> usize {
        self.strings.heap_bytes()
            + self.kind.capacity()
            + 4 * (self.str_.capacity()
                + self.a_start.capacity()
                + self.a_count.capacity()
                + self.c_start.capacity()
                + self.c_count.capacity()
                + self.attr_name.capacity()
                + self.attr_c_start.capacity()
                + self.attr_c_count.capacity()
                + self.children.capacity()
                + self.path.capacity())
    }

    // ---- construction ----------------------------------------------------

    fn push_node(&mut self, kind: NodeKind, s: StrId) -> NodeId {
        self.kind.push(kind as u8);
        self.str_.push(s);
        self.a_start.push(0);
        self.a_count.push(0);
        self.c_start.push(0);
        self.c_count.push(0);
        (self.kind.len() - 1) as NodeId
    }

    fn set_children(&mut self, n: NodeId, kids: &[NodeId]) {
        self.c_start[n as usize] = self.children.len() as u32;
        self.c_count[n as usize] = kids.len() as u32;
        self.children.extend_from_slice(kids);
    }

    /// Load from DAST JSON as produced by `@doenet/parser`.
    pub fn from_json(json: &str) -> crate::Result<Dast> {
        let mut b = JsonBuilder { dast: Dast { strings: StringTable::new(), ..Default::default() }, intern: HashMap::new() };
        let empty = b.intern("");
        let root = b.dast.push_node(NodeKind::Element, empty);
        let kids = b.parse_root(json)?;
        b.dast.set_children(root, &kids);
        Ok(b.dast)
    }

    /// Load from the binary wire format (little-endian, see module docs).
    pub fn from_binary(bytes: &[u8]) -> crate::Result<Dast> {
        let bad = |what: &str| crate::Error::WireFormat(what.to_string());
        if bytes.len() < 8 + 6 * 4 || &bytes[0..4] != b"CDST" {
            return Err(bad("missing CDST magic"));
        }
        let mut r = Reader { b: bytes, pos: 4 };
        let version = r.u32();
        if version != 1 {
            return Err(bad("unsupported version"));
        }
        let n_strings = r.u32() as usize;
        let strings_len = r.u32() as usize;
        let n_nodes = r.u32() as usize;
        let n_attrs = r.u32() as usize;
        let n_children = r.u32() as usize;
        let n_path = r.u32() as usize;
        let need = 8 + 6 * 4 + (n_strings + 1) * 4 + strings_len.div_ceil(4) * 4 + n_nodes.div_ceil(4) * 4 + n_nodes * 4 * 5 + n_attrs * 4 * 3 + n_children * 4 + n_path * 4;
        if bytes.len() < need {
            return Err(bad("truncated"));
        }
        let offsets = r.u32s(n_strings + 1);
        let string_bytes = r.bytes(strings_len).to_vec();
        std::str::from_utf8(&string_bytes).map_err(|_| bad("strings are not UTF-8"))?;
        r.align4();
        let kind = r.bytes(n_nodes).to_vec();
        r.align4();
        let dast = Dast {
            strings: StringTable { offsets, bytes: string_bytes },
            kind,
            str_: r.u32s(n_nodes),
            a_start: r.u32s(n_nodes),
            a_count: r.u32s(n_nodes),
            c_start: r.u32s(n_nodes),
            c_count: r.u32s(n_nodes),
            attr_name: r.u32s(n_attrs),
            attr_c_start: r.u32s(n_attrs),
            attr_c_count: r.u32s(n_attrs),
            children: r.u32s(n_children),
            path: r.u32s(n_path),
        };
        // Bounds checks so accessors can index without panicking on bad input.
        let ns = dast.strings.len() as u32;
        if dast.str_.iter().any(|&s| s >= ns) || dast.attr_name.iter().any(|&s| s >= ns) || dast.path.iter().any(|&s| s >= ns) {
            return Err(bad("string index out of range"));
        }
        if dast.children.iter().any(|&c| c as usize >= n_nodes) {
            return Err(bad("child index out of range"));
        }
        for n in 0..n_nodes {
            let (cs, cc) = (dast.c_start[n] as usize, dast.c_count[n] as usize);
            if cs + cc > n_children {
                return Err(bad("child range out of range"));
            }
            let (a, ac) = (dast.a_start[n] as usize, dast.a_count[n] as usize);
            let limit = if dast.kind[n] == NodeKind::Macro as u8 { n_path } else { n_attrs };
            if a + ac > limit {
                return Err(bad("attribute or path range out of range"));
            }
        }
        for a in 0..n_attrs {
            if dast.attr_c_start[a] as usize + dast.attr_c_count[a] as usize > n_children {
                return Err(bad("attribute child range out of range"));
            }
        }
        Ok(dast)
    }

    /// Serialize to the binary wire format (used by tests and tooling).
    pub fn to_binary(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.strings.bytes.len() + self.len() * 24);
        out.extend_from_slice(b"CDST");
        for v in [1u32, self.strings.len() as u32, self.strings.bytes.len() as u32, self.len() as u32, self.attr_name.len() as u32, self.children.len() as u32, self.path.len() as u32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let u32s = |out: &mut Vec<u8>, v: &[u32]| for x in v { out.extend_from_slice(&x.to_le_bytes()) };
        u32s(&mut out, &self.strings.offsets);
        out.extend_from_slice(&self.strings.bytes);
        while out.len() % 4 != 0 { out.push(0); }
        out.extend_from_slice(&self.kind);
        while out.len() % 4 != 0 { out.push(0); }
        for v in [&self.str_, &self.a_start, &self.a_count, &self.c_start, &self.c_count, &self.attr_name, &self.attr_c_start, &self.attr_c_count, &self.children, &self.path] {
            u32s(&mut out, v);
        }
        out
    }
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes(self.b[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        v
    }
    fn u32s(&mut self, n: usize) -> Vec<u32> {
        let mut v = Vec::with_capacity(n);
        for chunk in self.b[self.pos..self.pos + n * 4].chunks_exact(4) {
            v.push(u32::from_le_bytes(chunk.try_into().unwrap()));
        }
        self.pos += n * 4;
        v
    }
    fn bytes(&mut self, n: usize) -> &'a [u8] {
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        s
    }
    fn align4(&mut self) {
        self.pos = self.pos.div_ceil(4) * 4;
    }
}

// ---- JSON loader -------------------------------------------------------------
//
// A hand-written serde visitor: serde's internally tagged enums buffer every
// node before dispatching on `type`, which doubled deserialization time.
// Children are deserialized recursively into a scratch list and then
// appended to the flat `children` array so each node's range is contiguous.

struct JsonBuilder {
    dast: Dast,
    intern: HashMap<String, StrId>,
}

impl JsonBuilder {
    fn intern(&mut self, s: &str) -> StrId {
        if let Some(&id) = self.intern.get(s) {
            return id;
        }
        let id = self.dast.strings.push(s);
        self.intern.insert(s.to_string(), id);
        id
    }

    fn parse_root(&mut self, json: &str) -> crate::Result<Vec<NodeId>> {
        use serde::de::DeserializeSeed;
        let mut de = serde_json::Deserializer::from_str(json);
        let kids = RootSeed(self).deserialize(&mut de)?;
        Ok(kids)
    }
}

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::fmt;

struct RootSeed<'b>(&'b mut JsonBuilder);

impl<'de, 'b> DeserializeSeed<'de> for RootSeed<'b> {
    type Value = Vec<NodeId>;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        struct V<'b>(&'b mut JsonBuilder);
        impl<'de, 'b> Visitor<'de> for V<'b> {
            type Value = Vec<NodeId>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a DAST root object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut kids = Vec::new();
                while let Some(key) = m.next_key::<std::borrow::Cow<str>>()? {
                    if key == "children" {
                        kids = m.next_value_seed(NodeListSeed(self.0))?;
                    } else {
                        m.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(kids)
            }
        }
        d.deserialize_map(V(self.0))
    }
}

/// A JSON array of nodes, returned as their ids (not yet attached).
struct NodeListSeed<'b>(&'b mut JsonBuilder);

impl<'de, 'b> DeserializeSeed<'de> for NodeListSeed<'b> {
    type Value = Vec<NodeId>;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        struct V<'b>(&'b mut JsonBuilder);
        impl<'de, 'b> Visitor<'de> for V<'b> {
            type Value = Vec<NodeId>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an array of DAST nodes")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut s: A) -> Result<Self::Value, A::Error> {
                let mut ids = Vec::with_capacity(s.size_hint().unwrap_or(0));
                while let Some(id) = s.next_element_seed(NodeSeed(self.0))? {
                    ids.push(id);
                }
                Ok(ids)
            }
        }
        d.deserialize_seq(V(self.0))
    }
}

struct NodeSeed<'b>(&'b mut JsonBuilder);

impl<'de, 'b> DeserializeSeed<'de> for NodeSeed<'b> {
    type Value = NodeId;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        struct V<'b>(&'b mut JsonBuilder);
        impl<'de, 'b> Visitor<'de> for V<'b> {
            type Value = NodeId;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a DAST node object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let b = self.0;
                let mut kind = NodeKind::Other;
                let mut name: Option<StrId> = None;
                let mut value: Option<StrId> = None;
                let mut path: Vec<StrId> = Vec::new();
                // (name, children) per attribute
                let mut attrs: Vec<(StrId, Vec<NodeId>)> = Vec::new();
                let mut kids: Vec<NodeId> = Vec::new();
                while let Some(key) = m.next_key::<std::borrow::Cow<str>>()? {
                    match key.as_ref() {
                        "type" => {
                            let t: std::borrow::Cow<str> = m.next_value()?;
                            kind = match t.as_ref() {
                                "element" => NodeKind::Element,
                                "text" => NodeKind::Text,
                                "macro" => NodeKind::Macro,
                                _ => NodeKind::Other,
                            };
                        }
                        "name" => {
                            let s: std::borrow::Cow<str> = m.next_value()?;
                            name = Some(b.intern(&s));
                        }
                        "value" => {
                            let s: std::borrow::Cow<str> = m.next_value()?;
                            value = Some(b.intern(&s));
                        }
                        "path" => path = m.next_value_seed(PathSeed(b))?,
                        "attributes" => attrs = m.next_value_seed(AttrsSeed(b))?,
                        "children" => kids = m.next_value_seed(NodeListSeed(b))?,
                        _ => {
                            m.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }
                let s = match kind {
                    NodeKind::Element => name.unwrap_or_else(|| b.intern("")),
                    NodeKind::Text => value.unwrap_or_else(|| b.intern("")),
                    _ => 0,
                };
                let id = b.dast.push_node(kind, s);
                match kind {
                    NodeKind::Element => {
                        b.dast.a_start[id as usize] = b.dast.attr_name.len() as u32;
                        b.dast.a_count[id as usize] = attrs.len() as u32;
                        for (n, c) in attrs {
                            b.dast.attr_name.push(n);
                            b.dast.attr_c_start.push(b.dast.children.len() as u32);
                            b.dast.attr_c_count.push(c.len() as u32);
                            b.dast.children.extend_from_slice(&c);
                        }
                        b.dast.set_children(id, &kids);
                    }
                    NodeKind::Macro => {
                        b.dast.a_start[id as usize] = b.dast.path.len() as u32;
                        b.dast.a_count[id as usize] = path.len() as u32;
                        b.dast.path.extend_from_slice(&path);
                    }
                    _ => {}
                }
                Ok(id)
            }
        }
        d.deserialize_map(V(self.0))
    }
}

struct PathSeed<'b>(&'b mut JsonBuilder);

impl<'de, 'b> DeserializeSeed<'de> for PathSeed<'b> {
    type Value = Vec<StrId>;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        struct V<'b>(&'b mut JsonBuilder);
        impl<'de, 'b> Visitor<'de> for V<'b> {
            type Value = Vec<StrId>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a macro path array")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut s: A) -> Result<Self::Value, A::Error> {
                #[derive(serde::Deserialize)]
                struct Part<'a> {
                    #[serde(borrow)]
                    name: std::borrow::Cow<'a, str>,
                }
                let mut v = Vec::new();
                while let Some(p) = s.next_element::<Part>()? {
                    v.push(self.0.intern(&p.name));
                }
                Ok(v)
            }
        }
        d.deserialize_seq(V(self.0))
    }
}

struct AttrsSeed<'b>(&'b mut JsonBuilder);

impl<'de, 'b> DeserializeSeed<'de> for AttrsSeed<'b> {
    type Value = Vec<(StrId, Vec<NodeId>)>;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        struct V<'b>(&'b mut JsonBuilder);
        impl<'de, 'b> Visitor<'de> for V<'b> {
            type Value = Vec<(StrId, Vec<NodeId>)>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an attributes object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut v = Vec::new();
                while let Some(key) = m.next_key::<std::borrow::Cow<str>>()? {
                    let name = self.0.intern(&key);
                    let kids = m.next_value_seed(AttrSeed(self.0))?;
                    v.push((name, kids));
                }
                Ok(v)
            }
        }
        d.deserialize_map(V(self.0))
    }
}

/// One attribute object: only its `children` matter.
struct AttrSeed<'b>(&'b mut JsonBuilder);

impl<'de, 'b> DeserializeSeed<'de> for AttrSeed<'b> {
    type Value = Vec<NodeId>;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        struct V<'b>(&'b mut JsonBuilder);
        impl<'de, 'b> Visitor<'de> for V<'b> {
            type Value = Vec<NodeId>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an attribute object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut kids = Vec::new();
                while let Some(key) = m.next_key::<std::borrow::Cow<str>>()? {
                    if key == "children" {
                        kids = m.next_value_seed(NodeListSeed(self.0))?;
                    } else {
                        m.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(kids)
            }
        }
        d.deserialize_map(V(self.0))
    }
}

/// Load from either format, detected by the magic bytes.
pub fn load(bytes: &[u8]) -> crate::Result<Dast> {
    if bytes.starts_with(b"CDST") {
        Dast::from_binary(bytes)
    } else {
        let s = std::str::from_utf8(bytes).map_err(|_| crate::Error::WireFormat("not UTF-8 JSON".into()))?;
        Dast::from_json(s)
    }
}
