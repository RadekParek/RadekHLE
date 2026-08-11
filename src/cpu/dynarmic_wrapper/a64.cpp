/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#include <array>
#include <atomic>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdarg>
#include <cstdlib>
#include <memory>
#include <optional>
#include <string>
#include <thread>

#if !defined(_WIN32)
#include <pthread.h>
#include <signal.h>
#include <sys/types.h>
#include <unistd.h>
#include <unwind.h>
#if defined(__linux__)
#include <sys/syscall.h>
#include <ucontext.h>
#if defined(__x86_64__)
#include <sys/ucontext.h>
#endif
#endif
#endif

#include "dynarmic/interface/A64/a64.h"
#include "dynarmic/interface/A64/config.h"
#include "dynarmic/interface/exclusive_monitor.h"

namespace touchHLE::cpu {

struct DynarmicWrapper;
using A64Vector = Dynarmic::A64::Vector;
using VAddr = std::uint64_t;

extern "C" {
struct touchHLE_Mem;
std::uint8_t touchHLE_cpu_read_u8_64(touchHLE_Mem*, VAddr, bool*);
std::uint16_t touchHLE_cpu_read_u16_64(touchHLE_Mem*, VAddr, bool*);
std::uint32_t touchHLE_cpu_read_u32_64(touchHLE_Mem*, VAddr, bool*);
std::uint64_t touchHLE_cpu_read_u64_64(touchHLE_Mem*, VAddr, bool*);
std::array<std::uint64_t, 2> touchHLE_cpu_read_u128_64(touchHLE_Mem*, VAddr, bool*);
bool touchHLE_cpu_write_u8_64(touchHLE_Mem*, VAddr, std::uint8_t);
bool touchHLE_cpu_write_u16_64(touchHLE_Mem*, VAddr, std::uint16_t);
bool touchHLE_cpu_write_u32_64(touchHLE_Mem*, VAddr, std::uint32_t);
bool touchHLE_cpu_write_u64_64(touchHLE_Mem*, VAddr, std::uint64_t);
bool touchHLE_cpu_write_u128_64(touchHLE_Mem*, VAddr, std::array<std::uint64_t, 2>);
void touchHLE_cpu_a64_log(const char* message);
struct touchHLE_DynarmicA64Context {
  std::array<std::uint64_t, 31> regs;
  std::array<std::array<std::uint64_t, 2>, 32> vectors;
  std::uint64_t sp;
  std::uint64_t pc;
  std::uint32_t pstate;
  std::uint32_t fpcr;
  std::uint32_t fpsr;
};
}

const auto HaltReasonSvc = Dynarmic::HaltReason::UserDefined1;
const auto HaltReasonUndefinedInstruction = Dynarmic::HaltReason::UserDefined2;
const auto HaltReasonBreakpoint = Dynarmic::HaltReason::UserDefined3;

namespace {

std::atomic<std::uint64_t> watchdog_guest_pc{0};
std::atomic<std::uint64_t> watchdog_guest_sp{0};
std::atomic<std::uint64_t> watchdog_guest_lr{0};

#if !defined(_WIN32)
struct UnwindState {
  unsigned frame = 0;
};

_Unwind_Reason_Code unwind_frame(_Unwind_Context* context, void* raw_state) {
  auto* state = static_cast<UnwindState*>(raw_state);
  const auto pc = _Unwind_GetIP(context);
  if (pc == 0 || state->frame >= 32) {
    return _URC_END_OF_STACK;
  }
  dprintf(STDERR_FILENO, "#%u native_pc=%#llx\n", state->frame++, static_cast<unsigned long long>(pc));
  return _URC_NO_REASON;
}

void watchdog_signal_handler(int signal_number, siginfo_t*, void* raw_context) {
  std::uintptr_t native_pc = 0;
  std::uintptr_t native_sp = 0;
  std::uintptr_t native_lr = 0;
#if defined(__aarch64__) && defined(__linux__)
  const auto* context = static_cast<const ucontext_t*>(raw_context);
  native_pc = context->uc_mcontext.pc;
  native_sp = context->uc_mcontext.sp;
  native_lr = context->uc_mcontext.regs[30];
#elif defined(__x86_64__) && defined(__linux__)
  const auto* context = static_cast<const ucontext_t*>(raw_context);
  native_pc = context->uc_mcontext.gregs[REG_RIP];
  native_sp = context->uc_mcontext.gregs[REG_RSP];
  native_lr = context->uc_mcontext.gregs[REG_RBP];
#elif defined(__aarch64__) && defined(__APPLE__)
  const auto* context = static_cast<const ucontext_t*>(raw_context);
  native_pc = context->uc_mcontext->__ss.__pc;
  native_sp = context->uc_mcontext->__ss.__sp;
  native_lr = context->uc_mcontext->__ss.__lr;
#elif defined(__x86_64__) && defined(__APPLE__)
  const auto* context = static_cast<const ucontext_t*>(raw_context);
  native_pc = context->uc_mcontext->__ss.__rip;
  native_sp = context->uc_mcontext->__ss.__rsp;
  native_lr = context->uc_mcontext->__ss.__rbp;
#endif
#if defined(__linux__)
  const auto thread_id = static_cast<unsigned long long>(syscall(SYS_gettid));
#else
  const auto thread_id = static_cast<unsigned long long>(reinterpret_cast<std::uintptr_t>(pthread_self()));
#endif
  dprintf(STDERR_FILENO, "ARM64 dynarmic watchdog signal=%d thread_id=%llu native_pc=%#llx native_sp=%#llx native_lr=%#llx guest_pc=%#llx guest_sp=%#llx guest_lr=%#llx\n", signal_number, thread_id, static_cast<unsigned long long>(native_pc), static_cast<unsigned long long>(native_sp), static_cast<unsigned long long>(native_lr), static_cast<unsigned long long>(watchdog_guest_pc.load(std::memory_order_relaxed)), static_cast<unsigned long long>(watchdog_guest_sp.load(std::memory_order_relaxed)), static_cast<unsigned long long>(watchdog_guest_lr.load(std::memory_order_relaxed)));
  UnwindState state;
  _Unwind_Backtrace(unwind_frame, &state);
  _exit(134);
}

void install_watchdog_signal_handler() {
  static bool installed = false;
  if (installed) return;
  installed = true;
  struct sigaction action{};
  action.sa_sigaction = &watchdog_signal_handler;
  action.sa_flags = SA_SIGINFO | SA_ONSTACK;
  sigemptyset(&action.sa_mask);
  sigaction(SIGUSR2, &action, nullptr);
}
#else
void install_watchdog_signal_handler() {}
#endif

void update_watchdog_guest_state(const Dynarmic::A64::Jit* cpu) {
  if (!cpu) {
    return;
  }
  watchdog_guest_pc.store(cpu->GetPC(), std::memory_order_relaxed);
  watchdog_guest_sp.store(cpu->GetSP(), std::memory_order_relaxed);
  watchdog_guest_lr.store(cpu->GetRegister(30), std::memory_order_relaxed);
}

}

void tracef(const char* format, ...) {
  char message[768];
  va_list args;
  va_start(args, format);
  std::vsnprintf(message, sizeof(message), format, args);
  va_end(args);
  touchHLE_cpu_a64_log(message);
}

const char* halt_reason_name(Dynarmic::HaltReason reason) {
  if (Dynarmic::Has(reason, Dynarmic::HaltReason::MemoryAbort)) return "memory-abort";
  if (Dynarmic::Has(reason, HaltReasonUndefinedInstruction)) return "undefined-instruction";
  if (Dynarmic::Has(reason, HaltReasonBreakpoint)) return "breakpoint";
  if (Dynarmic::Has(reason, HaltReasonSvc)) return "svc";
  if (Dynarmic::Has(reason, Dynarmic::HaltReason::Step)) return "step";
  if (Dynarmic::Has(reason, Dynarmic::HaltReason::CacheInvalidation)) return "cache-invalidation";
  if (!reason) return "normal";
  return "other";
}

std::string register_dump(const Dynarmic::A64::Jit& cpu) {
  std::string dump;
  for (std::size_t i = 0; i < 31; ++i) {
    char field[64];
    std::snprintf(field, sizeof(field), "x%zu=%#018llx%s", i,
                  static_cast<unsigned long long>(cpu.GetRegister(i)),
                  i == 30 ? "" : " ");
    dump += field;
  }
  return dump;
}

class Environment final : public Dynarmic::A64::UserCallbacks {
public:
  Dynarmic::A64::Jit* cpu = nullptr;
  touchHLE_Mem* mem = nullptr;
  std::uint64_t ticks_remaining = 0;
  std::uint32_t halting_svc = 0;
  bool trace_enabled = false;
  std::uint64_t code_fetches = 0;
  std::uint64_t memory_faults = 0;

  void trace(const char* format, ...) {
    if (!trace_enabled) return;
    update_watchdog_guest_state(cpu);
    char message[768];
    va_list args;
    va_start(args, format);
    std::vsnprintf(message, sizeof(message), format, args);
    va_end(args);
    touchHLE_cpu_a64_log(message);
  }

private:
  template <typename T, typename F>
  T read(VAddr addr, F f, const char* kind) {
    update_watchdog_guest_state(cpu);
    bool error = false;
    T value = f(mem, addr, &error);
    if (error) {
      ++memory_faults;
      trace("invalid %s: address=%#llx pc=%#llx sp=%#llx lr=%#llx", kind,
            static_cast<unsigned long long>(addr),
            static_cast<unsigned long long>(cpu ? cpu->GetPC() : 0),
            static_cast<unsigned long long>(cpu ? cpu->GetSP() : 0),
            static_cast<unsigned long long>(cpu ? cpu->GetRegister(30) : 0));
      cpu->HaltExecution(Dynarmic::HaltReason::MemoryAbort);
    }
    return value;
  }

  template <typename T, typename F>
  void write(VAddr addr, T value, F f, const char* kind) {
    update_watchdog_guest_state(cpu);
    if (f(mem, addr, value)) {
      ++memory_faults;
      trace("invalid %s: address=%#llx pc=%#llx sp=%#llx lr=%#llx", kind,
            static_cast<unsigned long long>(addr),
            static_cast<unsigned long long>(cpu ? cpu->GetPC() : 0),
            static_cast<unsigned long long>(cpu ? cpu->GetSP() : 0),
            static_cast<unsigned long long>(cpu ? cpu->GetRegister(30) : 0));
      cpu->HaltExecution(Dynarmic::HaltReason::MemoryAbort);
    }
  }

  std::uint8_t MemoryRead8(VAddr a) override { return read<std::uint8_t>(a, touchHLE_cpu_read_u8_64, "read8"); }
  std::uint16_t MemoryRead16(VAddr a) override { return read<std::uint16_t>(a, touchHLE_cpu_read_u16_64, "read16"); }
  std::uint32_t MemoryRead32(VAddr a) override { return read<std::uint32_t>(a, touchHLE_cpu_read_u32_64, "read32"); }
  std::uint64_t MemoryRead64(VAddr a) override { return read<std::uint64_t>(a, touchHLE_cpu_read_u64_64, "read64"); }
  A64Vector MemoryRead128(VAddr a) override { return read<A64Vector>(a, touchHLE_cpu_read_u128_64, "read128"); }

  std::optional<std::uint32_t> MemoryReadCode(VAddr a) override {
    update_watchdog_guest_state(cpu);
    bool error = false;
    auto value = touchHLE_cpu_read_u32_64(mem, a, &error);
    ++code_fetches;
    if (trace_enabled && (code_fetches <= 128 || error)) {
      trace("DYNARMIC_TRANSLATION_FETCH #%llu: address=%#llx instruction=%#010x result=%s current_pc=%#llx",
            static_cast<unsigned long long>(code_fetches),
            static_cast<unsigned long long>(a),
            value,
            error ? "fault" : "ok",
            static_cast<unsigned long long>(cpu ? cpu->GetPC() : 0));
    }
    if (error) {
      ++memory_faults;
      trace("invalid execute: address=%#llx current_pc=%#llx code_fetches=%llu",
            static_cast<unsigned long long>(a),
            static_cast<unsigned long long>(cpu ? cpu->GetPC() : 0),
            static_cast<unsigned long long>(code_fetches));
      return std::nullopt;
    }
    return value;
  }

  void MemoryWrite8(VAddr a, std::uint8_t v) override { write(a, v, touchHLE_cpu_write_u8_64, "write8"); }
  void MemoryWrite16(VAddr a, std::uint16_t v) override { write(a, v, touchHLE_cpu_write_u16_64, "write16"); }
  void MemoryWrite32(VAddr a, std::uint32_t v) override { write(a, v, touchHLE_cpu_write_u32_64, "write32"); }
  void MemoryWrite64(VAddr a, std::uint64_t v) override {
    trace("JIT memory write64: address=%#llx value=%#llx pc=%#llx sp=%#llx", static_cast<unsigned long long>(a), static_cast<unsigned long long>(v), static_cast<unsigned long long>(cpu ? cpu->GetPC() : 0), static_cast<unsigned long long>(cpu ? cpu->GetSP() : 0));
    write(a, v, touchHLE_cpu_write_u64_64, "write64");
    trace("JIT memory write64 complete: address=%#llx pc=%#llx sp=%#llx", static_cast<unsigned long long>(a), static_cast<unsigned long long>(cpu ? cpu->GetPC() : 0), static_cast<unsigned long long>(cpu ? cpu->GetSP() : 0));
  }
  void MemoryWrite128(VAddr a, A64Vector v) override { write(a, v, touchHLE_cpu_write_u128_64, "write128"); }

  bool MemoryWriteExclusive8(VAddr a, std::uint8_t v, std::uint8_t e) override { if (MemoryRead8(a) != e) return false; MemoryWrite8(a, v); return true; }
  bool MemoryWriteExclusive16(VAddr a, std::uint16_t v, std::uint16_t e) override { if (MemoryRead16(a) != e) return false; MemoryWrite16(a, v); return true; }
  bool MemoryWriteExclusive32(VAddr a, std::uint32_t v, std::uint32_t e) override { if (MemoryRead32(a) != e) return false; MemoryWrite32(a, v); return true; }
  bool MemoryWriteExclusive64(VAddr a, std::uint64_t v, std::uint64_t e) override { if (MemoryRead64(a) != e) return false; MemoryWrite64(a, v); return true; }
  bool MemoryWriteExclusive128(VAddr a, A64Vector v, A64Vector e) override { if (MemoryRead128(a) != e) return false; MemoryWrite128(a, v); return true; }

  void InterpreterFallback(VAddr pc, size_t count) override {
    bool error = false;
    const auto instruction = touchHLE_cpu_read_u32_64(mem, pc, &error);
    trace("unsupported instruction: pc=%#llx instruction=%#010x fetch=%s count=%zu sp=%#llx lr=%#llx regs={%s}",
          static_cast<unsigned long long>(pc), instruction, error ? "fault" : "ok", count,
          static_cast<unsigned long long>(cpu->GetSP()),
          static_cast<unsigned long long>(cpu->GetRegister(30)),
          register_dump(*cpu).c_str());
    cpu->HaltExecution(HaltReasonUndefinedInstruction);
  }
  void CallSVC(std::uint32_t svc) override {
    halting_svc = svc;
    trace("SVC: number=%u pc=%#llx sp=%#llx lr=%#llx", svc,
          static_cast<unsigned long long>(cpu->GetPC()),
          static_cast<unsigned long long>(cpu->GetSP()),
          static_cast<unsigned long long>(cpu->GetRegister(30)));
    cpu->HaltExecution(HaltReasonSvc);
  }
  void ExceptionRaised(VAddr pc, Dynarmic::A64::Exception e) override {
    bool error = false;
    const auto instruction = touchHLE_cpu_read_u32_64(mem, pc, &error);
    trace("exception: type=%u pc=%#llx instruction=%#010x fetch=%s sp=%#llx lr=%#llx fp=%#llx regs={%s}",
          unsigned(e), static_cast<unsigned long long>(pc), instruction, error ? "fault" : "ok",
          static_cast<unsigned long long>(cpu->GetSP()),
          static_cast<unsigned long long>(cpu->GetRegister(30)),
          static_cast<unsigned long long>(cpu->GetRegister(29)),
          register_dump(*cpu).c_str());
    if (e == Dynarmic::A64::Exception::NoExecuteFault) {
      cpu->HaltExecution(Dynarmic::HaltReason::MemoryAbort);
    } else if (e == Dynarmic::A64::Exception::Breakpoint) {
      cpu->HaltExecution(HaltReasonBreakpoint);
    } else {
      cpu->HaltExecution(HaltReasonUndefinedInstruction);
    }
  }
  void AddTicks(std::uint64_t n) override {
    trace("DYNARMIC_TICKS_ADD n=%llu before=%llu", static_cast<unsigned long long>(n), static_cast<unsigned long long>(ticks_remaining));
    ticks_remaining = n > ticks_remaining ? 0 : ticks_remaining - n;
  }
  std::uint64_t GetTicksRemaining() override {
    trace("DYNARMIC_TICKS_GET remaining=%llu", static_cast<unsigned long long>(ticks_remaining));
    return ticks_remaining;
  }
  std::uint64_t GetCNTPCT() override { return 0x10000000000ULL - ticks_remaining; }
};

class A64Wrapper {
  Environment env;
  std::unique_ptr<Dynarmic::A64::Jit> cpu;
  std::unique_ptr<Dynarmic::ExclusiveMonitor> monitor;
  std::uint64_t execution_calls = 0;
public:
  A64Wrapper() {
    install_watchdog_signal_handler();
    tracef("jit construction: begin");
    Dynarmic::A64::UserConfig config;
    config.callbacks = &env;
    config.optimizations = Dynarmic::all_safe_optimizations;
    config.check_halt_on_memory_access = true;
    config.enable_cycle_counting = false;
    config.very_verbose_debugging_output = false;
    monitor = std::make_unique<Dynarmic::ExclusiveMonitor>(1);
    config.global_monitor = monitor.get();
    cpu = std::make_unique<Dynarmic::A64::Jit>(config);
    env.cpu = cpu.get();
    tracef("jit construction: complete");
  }
  void load_context(const touchHLE_DynarmicA64Context* c) {
    cpu->SetRegisters(c->regs);
    cpu->SetVectors(c->vectors);
    cpu->SetSP(c->sp);
    cpu->SetPC(c->pc);
    cpu->SetPstate(c->pstate);
    cpu->SetFpcr(c->fpcr);
    cpu->SetFpsr(c->fpsr);
  }
  void save_context(touchHLE_DynarmicA64Context* c) const {
    c->regs = cpu->GetRegisters();
    c->vectors = cpu->GetVectors();
    c->sp = cpu->GetSP();
    c->pc = cpu->GetPC();
    c->pstate = cpu->GetPstate();
    c->fpcr = cpu->GetFpcr();
    c->fpsr = cpu->GetFpsr();
  }
  void swap_context(touchHLE_DynarmicA64Context* c) {
    touchHLE_DynarmicA64Context old{};
    save_context(&old);
    load_context(c);
    *c = old;
  }
  std::int32_t run_or_step(touchHLE_Mem* mem, std::uint64_t* ticks) {
    env.mem = mem;
    env.halting_svc = 0;
    ++execution_calls;
    const auto pc = cpu->GetPC();
    update_watchdog_guest_state(cpu.get());
    bool code_error = false;
    const auto instruction = touchHLE_cpu_read_u32_64(mem, pc, &code_error);
    env.trace("execution enter #%llu: mode=%s pc=%#llx instruction=%#010x fetch=%s sp=%#llx lr=%#llx ticks=%s%llu",
              static_cast<unsigned long long>(execution_calls),
              ticks ? "run" : "step",
              static_cast<unsigned long long>(pc),
              instruction,
              code_error ? "fault" : "ok",
              static_cast<unsigned long long>(cpu->GetSP()),
              static_cast<unsigned long long>(cpu->GetRegister(30)),
              ticks ? "" : "none",
              ticks ? static_cast<unsigned long long>(*ticks) : 0);
    if (code_error) {
      env.trace("execution entry fetch failed: pc=%#llx; Dynarmic will be allowed to report the execution fault", static_cast<unsigned long long>(pc));
    }
    Dynarmic::HaltReason reason;
    const auto watchdog_ms = [] {
      const char* value = std::getenv("TOUCHHLE_ARM64_DYNARMIC_WATCHDOG_MS");
      if (!value) return std::uint64_t{2000};
      char* end = nullptr;
      const auto parsed = std::strtoull(value, &end, 10);
      return static_cast<std::uint64_t>(end != value && *end == '\0' && parsed > 0 ? parsed : 2000ULL);
    }();
    std::atomic<bool> execution_returned{false};
#if !defined(_WIN32)
    const auto execution_thread = pthread_self();
#endif
    std::thread watchdog([&] {
      const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(watchdog_ms);
      while (!execution_returned.load(std::memory_order_acquire) && std::chrono::steady_clock::now() < deadline) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
      }
      if (!execution_returned.load(std::memory_order_acquire)) {
        tracef("ARM64 dynarmic watchdog: Run/Step did not return within %llu ms; guest_pc=%#llx guest_sp=%#llx guest_lr=%#llx; requesting native backtrace", static_cast<unsigned long long>(watchdog_ms), static_cast<unsigned long long>(watchdog_guest_pc.load(std::memory_order_relaxed)), static_cast<unsigned long long>(watchdog_guest_sp.load(std::memory_order_relaxed)), static_cast<unsigned long long>(watchdog_guest_lr.load(std::memory_order_relaxed)));
#if !defined(_WIN32)
        pthread_kill(execution_thread, SIGUSR2);
        std::this_thread::sleep_for(std::chrono::milliseconds(100));
#endif
        std::abort();
      }
    });
    if (ticks) {
      env.ticks_remaining = *ticks;
      env.trace("Dynarmic configuration: unrestricted Run mode, single_step=false, cycle_counting=false, tick_budget=disabled, watchdog_ms=%llu", static_cast<unsigned long long>(watchdog_ms));
      env.trace("DYNARMIC_RUN_ENTER");
      env.trace("DEBUG_MARKER_BEFORE_DYNARMIC");
      reason = cpu->Run();
      env.trace("DEBUG_MARKER_AFTER_DYNARMIC");
      env.trace("DYNARMIC_RUN_RETURN reason=%#x pc=%#llx", static_cast<unsigned>(reason), static_cast<unsigned long long>(cpu->GetPC()));
    } else {
      env.trace("Dynarmic configuration: Step mode, cycle_counting=false, watchdog_ms=%llu", static_cast<unsigned long long>(watchdog_ms));
      env.trace("DYNARMIC_STEP_ENTER");
      env.trace("DEBUG_MARKER_BEFORE_DYNARMIC");
      reason = cpu->Step();
      env.trace("DEBUG_MARKER_AFTER_DYNARMIC");
      env.trace("DYNARMIC_STEP_RETURN reason=%#x pc=%#llx", static_cast<unsigned>(reason), static_cast<unsigned long long>(cpu->GetPC()));
      const auto step_bit = Dynarmic::HaltReason::Step;
      const bool completed_step = Dynarmic::Has(reason, step_bit);
      env.trace("single-step completion: completed=%s reason=%#x pc=%#llx", completed_step ? "true" : "false", static_cast<unsigned>(reason), static_cast<unsigned long long>(cpu->GetPC()));
      if (completed_step) {
        cpu->ClearHalt(step_bit);
      }
    }
    execution_returned.store(true, std::memory_order_release);
    watchdog.join();
    env.trace("execution return #%llu: reason=%#x (%s) pc=%#llx sp=%#llx lr=%#llx code_fetches=%llu memory_faults=%llu regs={%s}",
              static_cast<unsigned long long>(execution_calls),
              static_cast<unsigned>(reason), halt_reason_name(reason),
              static_cast<unsigned long long>(cpu->GetPC()),
              static_cast<unsigned long long>(cpu->GetSP()),
              static_cast<unsigned long long>(cpu->GetRegister(30)),
              static_cast<unsigned long long>(env.code_fetches),
              static_cast<unsigned long long>(env.memory_faults),
              register_dump(*cpu).c_str());
    std::int32_t result = ((!ticks && Dynarmic::Has(reason, Dynarmic::HaltReason::Step)) || (ticks && !reason)) ? -1 : -5;
    if (Dynarmic::Has(reason, Dynarmic::HaltReason::MemoryAbort)) result = -2;
    else if (Dynarmic::Has(reason, HaltReasonUndefinedInstruction)) result = -3;
    else if (Dynarmic::Has(reason, HaltReasonBreakpoint)) result = -4;
    else if (Dynarmic::Has(reason, HaltReasonSvc)) result = static_cast<std::int32_t>(env.halting_svc);
    if (ticks) *ticks = env.ticks_remaining;
    env.mem = nullptr;
    return result;
  }

  void clear_halt(std::uint32_t reason) {
    cpu->ClearHalt(static_cast<Dynarmic::HaltReason>(reason));
  }

  void set_trace(bool enabled) {
    env.trace_enabled = enabled;
#if !defined(_WIN32)
    if (enabled) {
      setenv("DYNARMIC_TRACE_BLOCKS", "1", 1);
    } else {
      unsetenv("DYNARMIC_TRACE_BLOCKS");
    }
#endif
    tracef("trace configuration: enabled=%s", enabled ? "true" : "false");
  }
};

extern "C" {
DynarmicWrapper* touchHLE_DynarmicA64Wrapper_new() { return reinterpret_cast<DynarmicWrapper*>(new A64Wrapper()); }
void touchHLE_DynarmicA64Wrapper_delete(DynarmicWrapper* p) { delete reinterpret_cast<A64Wrapper*>(p); }
void touchHLE_DynarmicA64Wrapper_swap_context(DynarmicWrapper* p, touchHLE_DynarmicA64Context* c) { reinterpret_cast<A64Wrapper*>(p)->swap_context(c); }
void touchHLE_DynarmicA64Wrapper_load_context(DynarmicWrapper* p, const touchHLE_DynarmicA64Context* c) { reinterpret_cast<A64Wrapper*>(p)->load_context(c); }
void touchHLE_DynarmicA64Wrapper_save_context(DynarmicWrapper* p, touchHLE_DynarmicA64Context* c) { reinterpret_cast<A64Wrapper*>(p)->save_context(c); }
std::int32_t touchHLE_DynarmicA64Wrapper_run_or_step(DynarmicWrapper* p, touchHLE_Mem* mem, std::uint64_t* ticks) { return reinterpret_cast<A64Wrapper*>(p)->run_or_step(mem, ticks); }
void touchHLE_DynarmicA64Wrapper_clear_halt(DynarmicWrapper* p, std::uint32_t reason) { reinterpret_cast<A64Wrapper*>(p)->clear_halt(reason); }
void touchHLE_DynarmicA64Wrapper_set_trace(DynarmicWrapper* p, bool enabled) { reinterpret_cast<A64Wrapper*>(p)->set_trace(enabled); }
}
}
