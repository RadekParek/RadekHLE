# BioShock launch and ARM32 performance

Status: completed
Date: 2026-09-21

## Scope

Fix the saved/app-picker `--device-family=iphone5` launch failure so device family remains unset unless the user explicitly selects one. Add safe, runtime-independent ARM32 performance improvements without changing guest-visible behaviour or enabling software rendering. Validate the CLI/parser and the available emulator build; the requested BioShock IPA is currently absent from `/home/.z/chat-uploads`, so IPA launch validation is blocked until it is uploaded again.

## Protected behaviour

- Native/hardware rendering remains the default; no CPU software renderer changes.
- Explicit device-family selections continue to work, including canonical model names and hardware identifiers.
- ARM32 execution remains Dynarmic JIT with direct memory access enabled by default.
- Do not enable unsafe Dynarmic optimisations or change guest scheduling semantics without a measured compatibility reason.

## Implementation

1. Accept legacy compact device-family aliases emitted by older saved picker settings, including `iphone5`, while retaining the no-override default.
2. Add regression coverage for compact aliases and the unset default.
3. Make low-risk ARM32 hot paths cheaper: cache the performance-environment decision, avoid unnecessary per-slice instrumentation work when disabled, and increase the JIT code cache only if the Dynarmic configuration supports it without changing correctness.
4. Run formatting, focused tests, a build/check, and CLI smoke tests. Inspect the final diff, commit, and push `trunk`.

## Validation

- `cargo test --lib device_family -- --nocapture`: 2 passed.
- `cargo build --bin radekhle`: passed with `RUSTFLAGS="-C link-arg=-latomic -C debuginfo=0"`.
- `--headless --device-family=iphone5`: accepted the legacy alias and reached the expected no-app error.
- Full BioShock launch validation remains blocked because the referenced IPA is not present in `/home/.z/chat-uploads`.
