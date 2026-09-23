/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use super::*;
use crate::mem::{MutVoidPtr, PAGE_SIZE};

#[test]
fn keywords_are_complete_case_insensitive_and_can_conflict() {
    assert_eq!(word_mask(b"\0player_coins\0"), 1 << Category::Money.index());
    assert_eq!(word_mask(b"\0AmmoCount\0"), 1 << Category::Ammo.index());
    assert_eq!(word_mask(b"\0health\0"), 1 << Category::Health.index());
    assert_eq!(word_mask(b"\0score\0"), 1 << Category::Score.index());
    assert_eq!(word_mask(b"\0cooldown\0"), 1 << Category::Timer.index());
    assert_eq!(word_mask(b"\0golden healthcare 123ammo ammunition123\0"), 0);
    assert_eq!(word_mask(b"ammo"), 0, "clipped tokens aren't evidence");
    let utf16: Vec<u8> = "\0Money\0"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert_eq!(word_mask(&utf16), 1 << Category::Money.index());
    let mut analysis = Analysis {
        text_mask: word_mask(b"\0money health\0"),
        inspected: true,
        ..Analysis::default()
    };
    analysis.update_hint();
    assert_eq!(analysis.category, Category::Unknown);
    assert_eq!(analysis.evidence, Evidence::ConflictingText);
}

#[test]
fn arbitrary_addresses_and_initial_values_remain_unknown() {
    for value in [0, 1, 30, 100, 135, 999, 1_000_000, u32::MAX as u64] {
        let mut a = Analysis::default();
        a.observe(VType::I32, value, value);
        assert_eq!(a.category, Category::Unknown);
    }
    let mut a = Analysis::default();
    a.observe(VType::I32, 100, 99);
    assert_eq!(a.category, Category::Unknown, "one event is not enough");
}

#[test]
fn unit_decrements_alone_are_ambiguous_but_reload_cycles_add_evidence() {
    let mut a = Analysis::default();
    for (old, new) in [(30, 29), (29, 28), (28, 27)] {
        a.observe(VType::I32, old, new);
    }
    assert_eq!(a.category, Category::Unknown);
    assert_eq!(a.evidence, Evidence::UnitDrops);
    a.observe(VType::I32, 27, 30);
    for (old, new) in [(30, 29), (29, 28), (28, 27), (27, 30)] {
        a.observe(VType::I32, old, new);
    }
    assert_eq!(a.category, Category::Ammo);
    assert_eq!(a.evidence, Evidence::ReloadCycles);
    a.text_mask = word_mask(b"\0ammo\0");
    a.update_hint();
    assert_eq!(a.evidence, Evidence::TextAndChanges);
    a.reset_history();
    assert_eq!(a.evidence, Evidence::NearbyText);
    a.text_mask = 0;
    a.update_hint();
    assert_eq!(a.category, Category::Unknown);
    a.observe(VType::I32, 10000, 9999);
    assert_eq!(a.category, Category::Unknown);
}

#[test]
fn timer_guess_needs_repeated_fractional_float_decreases() {
    let mut a = Analysis::default();
    for (old, new) in [
        (3.5f32, 3.25f32),
        (3.25, 3.0),
        (3.0, 2.75),
        (2.75, 2.5),
        (2.5, 2.25),
        (2.25, 2.0),
    ] {
        a.observe(VType::F32, old.to_bits() as u64, new.to_bits() as u64);
    }
    assert_eq!(a.category, Category::Timer);
    assert_eq!(a.evidence, Evidence::SmoothDrops);
    a.observe(
        VType::F32,
        2.75f32.to_bits() as u64,
        f32::NAN.to_bits() as u64,
    );
    assert_eq!(a.category, Category::Unknown);
}

#[test]
fn memory_inspection_is_bounded_read_only_and_does_not_cross_allocations() {
    let mut mem = Mem::new();
    mem.set_null_segment_size(PAGE_SIZE);
    let ptr = mem.alloc(128);
    mem.bytes_at_mut(ptr.cast(), 128).fill(0);
    let base = ptr.to_bits();
    let bytes = mem.bytes_at_mut(ptr.cast(), 128);
    bytes[8..14].copy_from_slice(b"money\0");
    let result = SearchResult {
        addr: base + 32,
        vtype: VType::I32,
        bits: 135,
        changed: false,
        analysis: Analysis::default(),
    };
    assert!(result.vtype.write_at(&mut mem, result.addr, result.bits));
    let before = mem
        .get_bytes_fallible(ptr.cast_const(), 128)
        .unwrap()
        .to_vec();
    let mut results = [result];
    let mut cursor = 0;
    analyze_batch(&mem, &mut results, &mut cursor);
    assert_eq!(results[0].analysis.category, Category::Money);
    assert_eq!(
        mem.get_bytes_fallible(ptr.cast_const(), 128).unwrap(),
        before
    );

    let other = mem.alloc(16);
    mem.bytes_at_mut(other.cast(), 16).fill(0);
    results[0].addr = other.to_bits();
    analyze_batch(&mem, &mut results, &mut cursor);
    assert_eq!(results[0].analysis.category, Category::Unknown);
    mem.free(MutVoidPtr::from_bits(base));
    results[0] = result;
    analyze_batch(&mem, &mut results, &mut cursor);
    assert_eq!(results[0].analysis.category, Category::Unknown);
}

#[test]
fn large_searches_are_inspected_incrementally() {
    let mut mem = Mem::new();
    mem.set_null_segment_size(PAGE_SIZE);
    let base = mem.alloc(8192).to_bits();
    let mut results: Vec<_> = (0..1325)
        .map(|i| SearchResult {
            addr: base + i * 4,
            vtype: VType::I32,
            bits: 0,
            changed: false,
            analysis: Analysis::default(),
        })
        .collect();
    let mut cursor = 0;
    analyze_batch(&mem, &mut results, &mut cursor);
    assert_eq!(cursor, 1024);
    assert_eq!(
        results.iter().filter(|r| r.analysis.inspected).count(),
        1024
    );
    analyze_batch(&mem, &mut results, &mut cursor);
    assert!(results.iter().all(|r| r.analysis.inspected));
}

#[test]
fn filter_cycle_reaches_each_category_then_all() {
    let mut filter = ResultFilter::All;
    for category in Category::ALL {
        filter = filter.next();
        assert_eq!(filter, ResultFilter::Category(category));
        for other in Category::ALL {
            assert_eq!(filter.matches(other), category == other);
        }
    }
    assert_eq!(filter.next(), ResultFilter::All);
}

#[test]
fn field_names_are_stronger_than_unrelated_neighbouring_words() {
    assert_eq!(field_mask("_playerMoney"), 1 << Category::Money.index());
    assert_eq!(field_mask("currentAmmo"), 1 << Category::Ammo.index());
    assert_eq!(field_mask("currentHP"), 1 << Category::Health.index());
    assert_eq!(field_mask("healthPoints"), 1 << Category::Health.index());
    let mut a = Analysis {
        field_mask: field_mask("_coins"),
        text_mask: word_mask(b"\0health timer\0"),
        ..Analysis::default()
    };
    a.update_hint();
    assert_eq!(a.category, Category::Money);
    assert_eq!(a.evidence, Evidence::ScalarField);
}

#[test]
fn irregular_float_changes_do_not_look_like_a_steady_timer() {
    let mut a = Analysis::default();
    let mut value = 20.5f32;
    for step in 0..12 {
        let next = value - if step % 2 == 0 { 0.1 } else { 1.5 };
        a.observe_timed(
            VType::F32,
            value.to_bits() as u64,
            next.to_bits() as u64,
            0.25,
        );
        assert_eq!(a.category, Category::Unknown);
        value = next;
    }
}
