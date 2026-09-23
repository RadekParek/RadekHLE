/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Bounded activity feed and explicit before/after experiment snapshots.
use super::bulk::containing_allocation;
use super::{classify::number, Mem, SearchResult, VType};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

pub const ACTIVITY_LIMIT: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct Change {
    pub addr: u32,
    pub vtype: VType,
    pub before: u64,
    pub after: u64,
    pub at: Instant,
}

#[derive(Default)]
pub(super) struct Activity {
    pub entries: VecDeque<Change>,
    pub changed_last_sample: usize,
}

impl Activity {
    pub fn record(&mut self, change: Change) {
        self.changed_last_sample += 1;
        if let Some(i) = self
            .entries
            .iter()
            .position(|c| c.addr == change.addr && c.vtype == change.vtype)
        {
            self.entries.remove(i);
        }
        self.entries.push_front(change);
        self.entries.truncate(ACTIVITY_LIMIT);
    }
}

#[derive(Clone, Copy, Debug)]
pub enum WatchFilter {
    Changed,
    Same,
    Increased,
    Decreased,
}

impl WatchFilter {
    fn matches(self, t: VType, before: u64, after: u64) -> bool {
        match self {
            Self::Changed => before != after,
            Self::Same => before == after,
            Self::Increased => number(t, after) > number(t, before),
            Self::Decreased => number(t, after) < number(t, before),
        }
    }
}

#[derive(Default)]
pub(super) struct Snapshot {
    // Allocation identity helps reject frees/moves. Reuse with identical
    // boundaries cannot be distinguished without allocator generations.
    values: HashMap<(u32, VType), (u64, (u32, u32))>,
}

impl Snapshot {
    pub fn capture(mem: &Mem, results: &[SearchResult]) -> Self {
        let mut allocations = mem.live_allocations();
        allocations.sort_unstable_by_key(|a| a.0);
        let values = results
            .iter()
            .filter_map(|r| {
                let allocation = containing_allocation(&allocations, r.addr, r.vtype.size())?;
                Some((
                    (r.addr, r.vtype),
                    (r.vtype.read_at(mem, r.addr)?, allocation),
                ))
            })
            .collect();
        Self { values }
    }

    pub fn retain(&self, mem: &Mem, results: &mut Vec<SearchResult>, filter: WatchFilter) {
        let mut allocations = mem.live_allocations();
        allocations.sort_unstable_by_key(|a| a.0);
        results.retain_mut(|r| {
            let Some(&(before, allocation)) = self.values.get(&(r.addr, r.vtype)) else { return false; };
            if containing_allocation(&allocations, r.addr, r.vtype.size()) != Some(allocation) { return false; }
            let Some(after) = r.vtype.read_at(mem, r.addr) else { return false; };
            if !filter.matches(r.vtype, before, after) { return false; }
            // Do not double-count this transition at the next live sample.
            r.bits = after;
            r.changed = before != after;
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trainer::tests::{memory, result};

    #[test]
    fn explicit_snapshot_survives_live_value_refreshes() {
        let (mut mem, base) = memory(64);
        let mut hits = vec![
            result(&mut mem, base, VType::I32, "30"),
            result(&mut mem, base + 8, VType::I32, "30"),
        ];
        let snapshot = Snapshot::capture(&mem, &hits);
        VType::I32.write_at(&mut mem, base, 29);
        hits[0].bits = 29; // ordinary live refresh must not move the baseline
        snapshot.retain(&mem, &mut hits, WatchFilter::Decreased);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].addr, base);
    }

    #[test]
    fn comparisons_use_signed_and_float_values_not_unsigned_bits() {
        assert!(WatchFilter::Increased.matches(VType::I8, 255, 0));
        assert!(WatchFilter::Decreased.matches(
            VType::F32,
            2.5f32.to_bits() as u64,
            1.5f32.to_bits() as u64
        ));
        assert!(!WatchFilter::Increased.matches(VType::F32, 0, f32::NAN.to_bits() as u64));
        assert!(WatchFilter::Same.matches(VType::I32, 2, 2));
        assert!(!WatchFilter::Changed.matches(VType::I32, 2, 2));
    }

    #[test]
    fn freed_results_do_not_survive_a_snapshot_comparison() {
        let (mut mem, base) = memory(64);
        let mut hits = vec![result(&mut mem, base, VType::I32, "30")];
        let snapshot = Snapshot::capture(&mem, &hits);
        mem.free(crate::mem::MutVoidPtr::from_bits(base));
        snapshot.retain(&mem, &mut hits, WatchFilter::Same);
        assert!(hits.is_empty());
    }

    #[test]
    fn feed_is_bounded_coalesced_and_keeps_latest_transition() {
        let mut feed = Activity::default();
        let at = Instant::now();
        for addr in 0..200 {
            feed.record(Change {
                addr,
                vtype: VType::I32,
                before: 5,
                after: 4,
                at,
            });
        }
        assert_eq!(feed.entries.len(), ACTIVITY_LIMIT);
        assert_eq!(feed.changed_last_sample, 200);
        feed.record(Change {
            addr: 199,
            vtype: VType::I32,
            before: 4,
            after: 3,
            at,
        });
        assert_eq!(feed.entries.len(), ACTIVITY_LIMIT);
        assert_eq!(feed.entries[0].after, 3);
    }
}
