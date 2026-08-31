use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);
static ARM32_CPU_COUNT: AtomicU64 = AtomicU64::new(0);
static ARM32_CPU_TIME_NS: AtomicU64 = AtomicU64::new(0);
static MEMORY_COUNT: AtomicU64 = AtomicU64::new(0);
static MEMORY_TIME_NS: AtomicU64 = AtomicU64::new(0);
static OBJC_COUNT: AtomicU64 = AtomicU64::new(0);
static OBJC_TIME_NS: AtomicU64 = AtomicU64::new(0);
static GLES_COUNT: AtomicU64 = AtomicU64::new(0);
static GLES_TIME_NS: AtomicU64 = AtomicU64::new(0);

pub fn configure(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub struct Scope {
    start: Instant,
    counter: &'static AtomicU64,
    time_ns: &'static AtomicU64,
}

impl Scope {
    pub fn new(counter: &'static AtomicU64, time_ns: &'static AtomicU64) -> Option<Self> {
        if enabled() {
            Some(Self {
                start: Instant::now(),
                counter,
                time_ns,
            })
        } else {
            None
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        self.counter.fetch_add(1, Ordering::Relaxed);
        self.time_ns.fetch_add(
            self.start.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }
}

pub fn interpreter_scope() -> Option<Scope> {
    Scope::new(&ARM32_CPU_COUNT, &ARM32_CPU_TIME_NS)
}

pub fn memory_scope() -> Option<Scope> {
    Scope::new(&MEMORY_COUNT, &MEMORY_TIME_NS)
}

pub fn objc_scope() -> Option<Scope> {
    Scope::new(&OBJC_COUNT, &OBJC_TIME_NS)
}

pub fn gles_scope() -> Option<Scope> {
    Scope::new(&GLES_COUNT, &GLES_TIME_NS)
}

pub fn configure_from_environment() {
    configure(std::env::var_os("TOUCHHLE_PERF").is_some());
}

pub fn report() {
    if !enabled() {
        return;
    }
    eprintln!("=== PERF REPORT ===");
    report_one("ARM32 CPU", &ARM32_CPU_COUNT, &ARM32_CPU_TIME_NS);
    report_one("Memory access", &MEMORY_COUNT, &MEMORY_TIME_NS);
    report_one("ObjC dispatch", &OBJC_COUNT, &OBJC_TIME_NS);
    report_one("GLES calls", &GLES_COUNT, &GLES_TIME_NS);
}

pub fn reset() {
    for (count, time_ns) in [
        (&ARM32_CPU_COUNT, &ARM32_CPU_TIME_NS),
        (&MEMORY_COUNT, &MEMORY_TIME_NS),
        (&OBJC_COUNT, &OBJC_TIME_NS),
        (&GLES_COUNT, &GLES_TIME_NS),
    ] {
        count.store(0, Ordering::Relaxed);
        time_ns.store(0, Ordering::Relaxed);
    }
}

fn report_one(name: &str, count: &AtomicU64, time_ns: &AtomicU64) {
    let count = count.load(Ordering::Relaxed);
    let time_ns = time_ns.load(Ordering::Relaxed);
    let total_ms = time_ns as f64 / 1_000_000.0;
    let per_call_us = time_ns as f64 / 1_000.0 / count.max(1) as f64;
    eprintln!("{name}: {count} calls, {total_ms:.2}ms total, {per_call_us:.3}us/call");
}
