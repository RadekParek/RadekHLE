# Runtime performance, scheduler diagnostics, and ARM32 backend

Status: complete
Scope: shared runtime performance and warning fixes; do not add a redundant ARM32 interpreter switch.

## Findings

- ARM32 already executes through Dynarmic's `Dynarmic::A32::Jit`; there is no ARM32 interpreter path to expose as a JIT toggle.
- The GLES guest wrapper calls `glGetError()` after every guest GLES call even when GL tracing is disabled. That is unnecessary driver work and can clear guest-visible errors.
- The scheduler watchdog treats normal runnable-thread ping-pong as suspicious and calls `yield_now()` after the warning.
- Missing Objective-C host-object reads cache only mutable fallbacks; immutable reads leak a new phantom object on every miss.
- `NSDictionary` class-cluster initialisation is an expected compatibility path and should be debug-only, not a normal warning.
- Android already has an ADPF-style performance-hint bridge. Do not invent a fake clock boost or blindly increase cache sizes; keep the bridge and improve only measurable host-side overhead.

## Changes

- Gate post-call GLES error polling on the existing trace option.
- Make the scheduler watchdog detect only a rapid sustained alternation, remove its performance-affecting host yield, and reuse one timestamp per scheduling pass.
- Cache immutable missing-object compatibility state safely and keep diagnostics once-per-site.
- Downgrade the expected NSDictionary class-cluster message to debug-only.
- Keep ARM32 on Dynarmic JIT, add an explicit startup diagnostic and preserve the existing performance counters.
- Buffer log-file writes without changing log levels, messages, or urgent flush behaviour.

## Verification

- `cargo fmt --all -- --check` is blocked by pre-existing formatting drift elsewhere in the repository; `git diff --check` passes.
- `PATH=/root/.cargo/bin:$PATH CARGO_BUILD_JOBS=1 RUSTFLAGS="-C link-arg=-latomic" cargo check` passed.
- Scheduler watchdog, options, and full library unit tests passed.
- No software-rendering default was changed; ARM32 remains Dynarmic JIT by default.
- Commit and push completed on `origin/trunk`.
