// Encode a normalized DAST (from @doenet/parser) into the compact binary
// wire format read by cells-core (`Dast::from_binary`). Little-endian.
//
//   "CDST" u32 version=1
//   u32 nStrings, stringBytesLen, nNodes, nAttrs, nChildren, nPath
//   u32[nStrings+1] string offsets; u8[stringBytesLen] UTF-8 (padded to 4)
//   u8[nNodes] kind (0 element, 1 text, 2 macro, 3 other) (padded to 4)
//   u32[nNodes] str, attrStart, attrCount, childStart, childCount
//   u32[nAttrs] name, childStart, childCount
//   u32[nChildren] child node ids; u32[nPath] path part string ids
//
// Node 0 is a synthetic root element whose children are the top-level nodes.
// Macros use attrStart/attrCount to index the path array.

export function encodeDast(dast) {
  const strings = [];
  const intern = new Map();
  const str = (s) => {
    let id = intern.get(s);
    if (id === undefined) { id = strings.length; strings.push(s); intern.set(s, id); }
    return id;
  };
  const kind = [], nstr = [], aStart = [], aCount = [], cStart = [], cCount = [];
  const attrName = [], attrCStart = [], attrCCount = [];
  const children = [], path = [];

  function pushNode(k, s) {
    kind.push(k); nstr.push(s); aStart.push(0); aCount.push(0); cStart.push(0); cCount.push(0);
    return kind.length - 1;
  }
  function setChildren(id, kids) {
    cStart[id] = children.length; cCount[id] = kids.length;
    for (const k of kids) children.push(k);
  }
  function node(n) {
    switch (n.type) {
      case "element": {
        const id = pushNode(0, str(n.name ?? ""));
        const attrs = Object.entries(n.attributes ?? {});
        // Deserialize children first so their ids exist, then record ranges.
        const attrKids = attrs.map(([, a]) => (a.children ?? []).map(node));
        const kids = (n.children ?? []).map(node);
        aStart[id] = attrName.length; aCount[id] = attrs.length;
        attrs.forEach(([name], i) => {
          attrName.push(str(name));
          attrCStart.push(children.length); attrCCount.push(attrKids[i].length);
          for (const k of attrKids[i]) children.push(k);
        });
        setChildren(id, kids);
        return id;
      }
      case "text": return pushNode(1, str(n.value ?? ""));
      case "macro": {
        const id = pushNode(2, 0);
        aStart[id] = path.length; aCount[id] = n.path.length;
        for (const p of n.path) path.push(str(p.name));
        return id;
      }
      default: return pushNode(3, 0);
    }
  }
  const root = pushNode(0, str(""));
  setChildren(root, (dast.children ?? []).map(node));

  const enc = new TextEncoder();
  const encoded = strings.map((s) => enc.encode(s));
  const offsets = new Uint32Array(strings.length + 1);
  let total = 0;
  encoded.forEach((b, i) => { offsets[i] = total; total += b.length; });
  offsets[strings.length] = total;
  const pad4 = (n) => (n + 3) & ~3;
  const n = kind.length;
  const size = 8 + 6 * 4 + offsets.byteLength + pad4(total) + pad4(n) + n * 4 * 5 + attrName.length * 4 * 3 + children.length * 4 + path.length * 4;
  const buf = new ArrayBuffer(size);
  const dv = new DataView(buf);
  const u8 = new Uint8Array(buf);
  let pos = 0;
  u8.set([0x43, 0x44, 0x53, 0x54], 0); pos = 4; // "CDST"
  for (const v of [1, strings.length, total, n, attrName.length, children.length, path.length]) { dv.setUint32(pos, v, true); pos += 4; }
  const putU32s = (arr) => { for (let i = 0; i < arr.length; i++) { dv.setUint32(pos, arr[i], true); pos += 4; } };
  putU32s(offsets);
  for (const b of encoded) { u8.set(b, pos); pos += b.length; }
  pos = pad4(pos);
  u8.set(kind, pos); pos = pad4(pos + n);
  for (const arr of [nstr, aStart, aCount, cStart, cCount, attrName, attrCStart, attrCCount, children, path]) putU32s(arr);
  return u8;
}
