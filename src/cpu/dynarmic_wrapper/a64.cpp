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
    write(a, v, touchHLE_cpu_write_u64_64, "write64");
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
    tracef("jit construction: begin");
    Dynarmic::A64::UserConfig config;
    config.callbacks = &env;
    config.optimizations = Dynarmic::all_safe_optimizations;
    config.check_halt_on_memory_access = true;
    config.enable_cycle_counting = true;
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
    std::thread watchdog([&] {
      const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(watchdog_ms);
      while (!execution_returned.load(std::memory_order_acquire) && std::chrono::steady_clock::now() < deadline) {
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
      }
      if (!execution_returned.load(std::memory_order_acquire)) {
        tracef("ARM64 dynarmic watchdog: Run/Step did not return within %llu ms; pc=%#llx sp=%#llx lr=%#llx ticks=%s%llu; aborting", static_cast<unsigned long long>(watchdog_ms), static_cast<unsigned long long>(cpu->GetPC()), static_cast<unsigned long long>(cpu->GetSP()), static_cast<unsigned long long>(cpu->GetRegister(30)), ticks ? "" : "none", ticks ? static_cast<unsigned long long>(*ticks) : 0);
        std::abort();
      }
    });
    if (ticks) {
      env.ticks_remaining = *ticks;
      env.trace("Dynarmic configuration: Run mode, single_step=false, cycle_counting=true, tick_budget=%llu, watchdog_ms=%llu", static_cast<unsigned long long>(*ticks), static_cast<unsigned long long>(watchdog_ms));
      env.trace("DYNARMIC_RUN_ENTER");
      reason = cpu->Run();
      env.trace("DYNARMIC_RUN_RETURN reason=%#x pc=%#llx", static_cast<unsigned>(reason), static_cast<unsigned long long>(cpu->GetPC()));
    } else {
      env.trace("Dynarmic configuration: Step mode, cycle_counting=true, watchdog_ms=%llu", static_cast<unsigned long long>(watchdog_ms));
      env.trace("DYNARMIC_STEP_ENTER");
      reason = cpu->Step();
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
