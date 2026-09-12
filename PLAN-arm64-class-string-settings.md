# ARM64 class-string and picker settings fix

Status: complete
Date: 2026-09-12

## Scope

Fix the ARM64 `NSStringFromClass` startup path from the supplied Angry Birds analysis without changing the ARM32 runtime. Keep Foundation semantics: return the real class name and return nil for a null or unknown class. Improve the picker settings presentation: white background, visible GLES override default, and non-overlapping controls.

## Implemented

- Added ARM64 handlers for `NSStringFromClass`, `NSClassFromString`, and `NSStringFromSelector` using real guest objects and nil-safe inputs.
- Corrected ARM64 class lookup and object class metadata helpers, including `objc_getClass`, `objc_lookUpClass`, `object_getClass`, and `object_getClassName`.
- Extended the ARM64 bootstrap grace window without changing the 32-bit runtime or synthesising lifecycle termination events.
- Made picker settings neutral/white, made the GLES override visibly `Default`, spaced iOS/device controls, and moved multi-line button groups down to avoid overlap.
- Added an ARM64 class-string round-trip regression test covering null and unknown-class cases.

## Protected behaviour

- No changes to `src/environment.rs` ARM32 launch paths.
- Unknown `NSClassFromString` names remain nil; no fabricated class string is returned.
- Existing graphics/GLES option defaults remain unchanged except for the clearer UI label.

## Validation

- `RUSTFLAGS='-C link-arg=-latomic' cargo check`
- `RUSTFLAGS='-C link-arg=-latomic' cargo test arm64_foundation_class_string_round_trip_is_nil_safe --lib`
- `RUSTFLAGS='-C link-arg=-latomic' cargo test --lib -- --skip test_app`
