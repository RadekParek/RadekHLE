# RadekHLE 9.0 settings and polish

- [x] Rebrand user-facing RadekHLE strings to 9.0 and retain release packaging consistency.
- [x] Make PVRTC automatic by default and keep native GLES queries one-shot in logs.
- [x] Reorganize the picker settings into Performance, Graphics, Compatibility, and Video & display with visible category controls.
- [x] Replace the custom-driver switch with add-folder and selectable-driver controls.
- [x] Preserve alpha when rounding icons so transparent app-picker artwork has no opaque square.
- [x] Format, run focused Rust checks, commit, and push trunk.

## Phase 1 — defaults, branding, and rendering polish

Affected files: `src/options.rs`, `src/gles.rs`, `src/gles/gles1_native.rs`, `src/gles/gles3_native.rs`, `src/image.rs`, release metadata.

Change user-facing branding, make PVRTC automatic, and preserve image alpha through icon rounding. Existing one-shot GLES unsupported-call logging remains the diagnostic mechanism.

## Phase 2 — picker settings organization

Affected file: `src/environment/app_picker.rs`.

Use four category buckets with ordered general-to-specific options, explicit category title colors, and a custom-driver add/list control backed by the existing driver loader.

## Phase 3 — verification and delivery

Affected files: formatted implementation and plan.

Run formatting, focused tests/compile checks available in the Cloud VM, inspect the diff, commit the result, and push `trunk`.
