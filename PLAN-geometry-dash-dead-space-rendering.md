# Geometry Dash and Dead Space rendering

Status: implementation ready; device verification pending
Date: 2026-09-24

## Scope

Fix the high-confidence GLES and drawable-layer problems that can produce Geometry Dash's broken/offset 2D rendering or a stale/black EAGL layer, while preserving guest-specified GL bindings and avoiding global viewport changes. Recheck the existing Dead Space startup workaround without duplicating it.

## Findings

- The supplied Geometry Dash log is from RadekHLE 9.1 (`4d3b8b8`), not the current 9.2 source. It shows a live render loop, a 480x320 landscape surface, and an early `TOUCHHLE_PRESENT_STRETCH_TO_VIEWPORT=1` override; the reported FPS later recovers to roughly 60–75. RadekHLE 9.2 no longer applies that stretch override to Geometry Dash, so the old log cannot verify the current path.
- KlugKlugT history contains relevant fixes for guest GLES attribute bindings, scissor state, renderbuffer lookup, and compositor buffer state. Scissor restoration, sole-renderbuffer mismatch recovery, pixel-pack alignment, and compositor array/element-buffer restoration are already present in this checkout. The pending attribute-binding and layer-presentation changes address the remaining relevant gaps.
- The existing layer finder could stop at the first non-fullscreen layer and follow only one sublayer path; the presenter could skip a drawable when another layer was chosen for the fast path. This can leave a second game/loading layer stale or black.
- The pending GLES2 change records guest `glBindAttribLocation` calls per shared context group, removes records on program deletion, and injects canonical aliases only when the app has not provided bindings. The app's explicit bindings therefore take precedence.
- The Dead Space startup orientation notification and its per-bundle `--force-composition` defaults are already in the source. No Dead Space IPA or fresh Dead Space runtime log is available here, so a device-side black-screen fix cannot be confirmed.
- The Angry Birds Star Wars II IPA archive and `Info.plist` are readable and consistent (bundle ID `com.rovio.angrybirdstarwarsii`, v1.9.19, ARMv7/GLES2, iOS 8+). There is no run log for it; network access was not enabled by default without an explicit request.
- The Granny log's incomplete-IPA warning names Slendrina; the Granny-specific nil-key warning is ignored by `NSMutableDictionary`. Silent Ops reports shader compilation and music-player integration failures that are unrelated to these rendering fixes and were left unchanged.

## Changes

1. Traverse the visible layer tree conservatively and select the largest opaque EAGL layer that exactly qualifies for the fullscreen fast path.
2. Preserve every non-selected EAGL drawable through the existing readback/Core Animation path instead of silently skipping its presentation. The selected fullscreen drawable retains the direct GPU path.
3. Preserve explicit guest GLES2 attribute bindings and apply the canonical Cocos-style aliases only when no guest bindings were supplied; clean up the per-sharegroup tracking when a program is deleted.
4. Leave Dead Space's existing orientation/composition workaround and all unrelated app defaults unchanged.

## Validation

- `cargo check --all-targets` passed with Rust 1.98.1. It reported existing repository warnings, with no new warning in the changed logic.
- `rustfmt --check` passed for the three modified Rust files; `git diff --check` passed.
- The requested unit-test command exceeded the 10-minute tool window during code generation and was stopped. Tests therefore have not been executed.
- No graphical runtime is available in this environment to verify Geometry Dash, Dead Space, or Angry Birds Star Wars II on-device. A fresh 9.2 log is needed to confirm the remaining game-specific symptoms.
