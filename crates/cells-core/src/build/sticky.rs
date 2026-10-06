//! Plan 4, wiring A: each `<stickyGroup>` becomes one `Sticky` identity
//! instruction over its members' points. The members' coordinate slots
//! become the instruction's outputs, and what they held before moves to
//! fresh slots that become its inputs, so every reader of a member's
//! coordinates (the renderer, references, copies) reads through the group,
//! and every request on them reaches the group's inverse.
//!
//! This runs once expansion has resolved every reference: membership is the
//! group's expanded children, so members from a repeat or a copy count.
//! A group may own more points than an element has slots, so the
//! instruction is built on slots directly rather than planned per element.

use std::collections::HashSet;

use super::*;
use crate::geo::sticky_header;
use crate::sticky::Member;

/// A group's members and their distinct points, as slots: the point's x
/// and y slots, the last ones on each coordinate's alias chain that belong
/// to a member of this group. A polygon whose vertex is a member point
/// shares that point rather than feeding it in twice.
struct StickyPlan {
    group: CompIdx,
    members: Vec<Member>,
    points: Vec<[SlotId; 2]>,
}

impl<'c, 'a> Builder<'c, 'a> {
    pub(super) fn wire_sticky_groups(&mut self) {
        let groups: Vec<CompIdx> = (0..self.comps.len() as CompIdx).filter(|&c| self.comps.kind[c as usize] == ComponentKind::StickyGroup).collect();
        // Plan every group before rewiring any: one group's members may be
        // copies of another's.
        let plans: Vec<StickyPlan> = groups.into_iter().map(|g| self.plan_sticky(g)).collect();
        for p in plans {
            if !p.points.is_empty() {
                self.install_sticky(p);
            }
        }
    }

    fn alias_root(&self, mut s: SlotId) -> SlotId {
        while let Source::Alias(t) = self.sources[s as usize] {
            s = t;
        }
        s
    }

    /// A rigid polygon's coordinates are outputs of its `Shape` instruction.
    fn is_rigid(&self, s: SlotId) -> bool {
        let shape = |src: &Source| matches!(src, Source::Vec(VecOp::Shape { .. }, ..));
        match &self.sources[self.alias_root(s) as usize] {
            Source::VecOut(h, _) => shape(&self.sources[*h as usize]),
            src => shape(src),
        }
    }

    fn plan_sticky(&self, group: CompIdx) -> StickyPlan {
        let mut raw: Vec<(crate::sticky::Shape, bool, Vec<[SlotId; 2]>)> = Vec::new();
        for k in self.comps.sticky_members(group) {
            let kind = self.comps.kind[k as usize];
            let Some((shape, first, max)) = kind.sticky_layout() else { continue };
            let n = if kind == ComponentKind::Polygon {
                match self.sources[self.alias_root(self.slot(k, 0)) as usize] {
                    Source::Fixed(v) if v >= 0.0 => (v as usize).min(max),
                    _ => 0,
                }
            } else {
                max
            };
            let coords: Vec<[SlotId; 2]> = (0..n).map(|i| [self.slot(k, first + 2 * i), self.slot(k, first + 2 * i + 1)]).collect();
            let rigid = coords.first().is_some_and(|c| self.is_rigid(c[0]));
            raw.push((shape, rigid, coords));
        }
        let in_group: HashSet<SlotId> = raw.iter().flat_map(|m| m.2.iter().flatten().copied()).collect();
        let identity = |mut s: SlotId| -> SlotId {
            let mut last = s;
            while let Source::Alias(t) = self.sources[s as usize] {
                s = t;
                if in_group.contains(&s) {
                    last = s;
                }
            }
            last
        };
        let mut points: Vec<[SlotId; 2]> = Vec::new();
        let mut index: HashMap<SlotId, u32> = HashMap::new();
        let members = raw
            .into_iter()
            .map(|(shape, rigid, coords)| {
                let ids = coords
                    .iter()
                    .map(|&[x, y]| {
                        let ix = identity(x);
                        *index.entry(ix).or_insert_with(|| {
                            points.push([ix, identity(y)]);
                            points.len() as u32 - 1
                        })
                    })
                    .collect();
                Member { shape, rigid, points: ids }
            })
            .collect();
        StickyPlan { group, members, points }
    }

    fn install_sticky(&mut self, p: StickyPlan) {
        // Move each point slot's source to a fresh slot: the instruction's input.
        let mut moved: HashMap<SlotId, SlotId> = HashMap::new();
        let mut raws = Vec::with_capacity(2 * p.points.len());
        for &s in p.points.iter().flatten() {
            let r = match moved.get(&s) {
                Some(&r) => r,
                None => {
                    let src = std::mem::replace(&mut self.sources[s as usize], Source::Unset);
                    let r = self.anon_slot(src);
                    self.moved_from.insert(r, s);
                    moved.insert(s, r);
                    r
                }
            };
            raws.push(r);
        }
        // A rigid polygon's coordinates were outputs of its `Shape` head,
        // which has just moved too.
        for &r in moved.values() {
            if let Source::VecOut(h, k) = self.sources[r as usize]
                && let Some(&nh) = moved.get(&h)
            {
                self.sources[r as usize] = Source::VecOut(nh, k);
            }
        }
        let start = self.op_inputs.len() as u32;
        let g = p.group;
        let (threshold, relative) = (self.slot(g, 0), self.slot(g, 1));
        self.op_inputs.push(threshold);
        self.op_inputs.push(relative);
        let parent = self.comps.parent[g as usize];
        for i in 0..4 {
            let s = if parent != NONE && self.comps.kind[parent as usize] == ComponentKind::Graph { self.slot(parent, i) } else { self.missing_slot() };
            self.op_inputs.push(s);
        }
        let header = sticky_header(&p.members);
        for &v in &header {
            let s = self.anon_slot(Source::Fixed(v));
            self.op_inputs.push(s);
        }
        self.op_inputs.extend_from_slice(&raws);
        let op = VecOp::Sticky { header: header.len() as u32, points: p.points.len() as u32 };
        let count = self.op_inputs.len() as u32 - start;
        let head = p.points[0][0];
        self.sources[head as usize] = Source::Vec(op, start, count);
        for (k, &s) in p.points.iter().flatten().enumerate().skip(1) {
            self.sources[s as usize] = Source::VecOut(head, k as u32);
        }
    }
}
