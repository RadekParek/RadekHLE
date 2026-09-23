/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Checked bulk editing: preview a filtered batch, then revalidate on confirm.
//! This detects structural hazards, not the meaning of a game's memory.

use super::{Mem, ResultFilter, SearchResult, VType};

type Allocation = (u32, u32);

#[derive(Debug, PartialEq, Eq)]
pub(super) struct BulkWrite {
    index: usize,
    addr: u32,
    vtype: VType,
    before: u64,
    after: u64,
    allocation: Allocation,
}

impl BulkWrite {
    fn end(&self) -> u64 {
        self.addr as u64 + self.vtype.size() as u64
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct BulkPlan {
    // Include the input even when two types happen to produce equal bits.
    requested_type: VType,
    text: String,
    safe_mode: bool,
    filter: ResultFilter,
    pub(super) writes: Vec<BulkWrite>,
    pub(super) skipped: usize,
}

/// Allocations are disjoint and sorted by base. Avoid an O(hits * allocations)
/// scan now that a bulk operation can cover all stored search results.
pub(super) fn containing_allocation(
    allocations: &[Allocation],
    addr: u32,
    size: u32,
) -> Option<Allocation> {
    let i = allocations.partition_point(|&(base, _)| base <= addr);
    let &(base, length) = allocations.get(i.checked_sub(1)?)?;
    (addr as u64 + size as u64 <= base as u64 + length as u64).then_some((base, length))
}

pub(super) fn plan_bulk(
    mem: &Mem,
    results: &[SearchResult],
    requested_type: VType,
    text: &str,
    safe_mode: bool,
    filter: ResultFilter,
) -> Result<BulkPlan, &'static str> {
    if results.is_empty() {
        return Err("NO RESULTS TO SET");
    }
    requested_type.parse(text).ok_or("BAD VALUE: CHECK TYPE")?;
    let mut allocations = mem.live_allocations();
    allocations.sort_unstable_by_key(|&(base, _)| base);
    let mut candidates = Vec::new();
    let mut matching = 0;
    for (index, result) in results.iter().enumerate() {
        if !filter.matches(result.analysis.category) {
            continue;
        }
        matching += 1;
        let t = result.vtype;
        if t == VType::Auto || (requested_type != VType::Auto && requested_type != t) {
            if !safe_mode {
                return Err("TYPE CHANGED: SEARCH AGAIN");
            }
            continue;
        }
        let Some(after) = t.parse(text) else {
            if !safe_mode {
                return Err("VALUE OUT OF RANGE: CHECK TYPE");
            }
            continue;
        };
        if (safe_mode && result.addr % t.size() != 0) || after == result.bits {
            continue;
        }
        let Some(allocation) = containing_allocation(&allocations, result.addr, t.size()) else {
            if !safe_mode {
                return Err("EXPIRED ADDRESS: SEARCH AGAIN");
            }
            continue;
        };
        if t.read_at(mem, result.addr) != Some(result.bits) {
            if !safe_mode {
                return Err("VALUES CHANGED: REFINE FIRST");
            }
            continue;
        }
        candidates.push(BulkWrite {
            index,
            addr: result.addr,
            vtype: t,
            before: result.bits,
            after,
            allocation,
        });
    }
    candidates.sort_unstable_by_key(|write| (write.addr, write.end()));

    // Safe mode rejects every member of an overlapping group, not an
    // arbitrary winner. With it off, the user explicitly opts into writing
    // these matches too; bounds, type and confirmation checks still apply.
    let mut conflicts = vec![false; candidates.len()];
    let mut first = 0;
    while first < candidates.len() {
        let mut end = candidates[first].end();
        let mut next = first + 1;
        while next < candidates.len() && (candidates[next].addr as u64) < end {
            end = end.max(candidates[next].end());
            next += 1;
        }
        if safe_mode && next > first + 1 {
            conflicts[first..next].fill(true);
        }
        first = next;
    }
    let writes: Vec<_> = candidates
        .into_iter()
        .zip(conflicts)
        .filter_map(|(write, conflict)| (!conflict).then_some(write))
        .collect();
    if writes.is_empty() {
        return Err("NO ELIGIBLE HITS: REFINE / CHECK TYPE");
    }
    Ok(BulkPlan {
        filter,
        safe_mode,
        requested_type,
        text: text.trim().to_string(),
        skipped: matching - writes.len(),
        writes,
    })
}

pub(super) fn apply_bulk(
    mem: &mut Mem,
    results: &mut [SearchResult],
    plan: &BulkPlan,
) -> Result<usize, &'static str> {
    let mut allocations = mem.live_allocations();
    allocations.sort_unstable_by_key(|&(base, _)| base);
    // Validate the whole confirmed plan before the first write. No guest
    // execution can take place between these checks and writes on this thread.
    for write in &plan.writes {
        let Some(result) = results.get(write.index) else {
            return Err("RESULTS CHANGED: PREVIEW AGAIN");
        };
        if !plan.filter.matches(result.analysis.category)
            || result.addr != write.addr
            || result.vtype != write.vtype
            || result.bits != write.before
            || containing_allocation(&allocations, write.addr, write.vtype.size())
                != Some(write.allocation)
            || write.vtype.read_at(mem, write.addr) != Some(write.before)
        {
            return Err("MEMORY CHANGED: PREVIEW AGAIN");
        }
    }
    for write in &plan.writes {
        if !write.vtype.write_at(mem, write.addr, write.after) {
            return Err("WRITE FAILED (BAD ADDR?)");
        }
    }
    // Re-read every affected alias, including hidden/non-targeted results.
    // A sorted prefix maximum handles nested/overlapping writes in O(N log W)
    // rather than doing a full results scan once per write. Trainer edits
    // must never be interpreted as evidence of in-game value changes.
    let ends: Vec<u64> = plan
        .writes
        .iter()
        .scan(0u64, |end, write| {
            *end = (*end).max(write.end());
            Some(*end)
        })
        .collect();
    for result in results {
        let end = result.addr as u64 + result.vtype.size() as u64;
        let i = plan.writes.partition_point(|w| (w.addr as u64) < end);
        if i > 0 && ends[i - 1] > result.addr as u64 {
            if let Some(bits) = result.vtype.read_at(mem, result.addr) {
                result.bits = bits;
            }
            result.changed = false;
            result.analysis.reset_history();
        }
    }
    Ok(plan.writes.len())
}
