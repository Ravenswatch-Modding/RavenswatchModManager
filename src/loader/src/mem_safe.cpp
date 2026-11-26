#include "mem_safe.h"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

namespace rsmm {

namespace {
// Highest canonical user-mode address on x64. Anything at or above this is a
// kernel / non-canonical value (the -1 sentinels that appear in half-built
// engine structs land here), and VirtualQuery on it is a waste of a syscall.
constexpr std::uintptr_t kUserMax = 0x0000800000000000ull;

constexpr DWORD kReadable = PAGE_READONLY | PAGE_READWRITE | PAGE_WRITECOPY |
                            PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE |
                            PAGE_EXECUTE_WRITECOPY;
constexpr DWORD kWritable = PAGE_READWRITE | PAGE_WRITECOPY |
                            PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY;
} // namespace

// ---------------------------------------------------------------------------
// Fault-tolerant copy.
//
// mem_accessible() answers "was this page readable a moment ago", not "will it
// still be when we read it". On 2026-11-26 a client crashed in lua_read_u64
// with rax=1 (the guard said yes) and the faulting page MEM_FREE in the dump:
// the engine's heap released it on another thread during LevelGs::_Unload,
// between the VirtualQuery and the load. MinGW has no __try/__except, so the
// copy itself must be recoverable: rsmm_fault_copy is a stack-less leaf in
// asm, and a vectored handler that only claims faults whose RIP lies inside it
// moves RIP to rsmm_fault_copy_fault, which returns 0. Nothing else is touched
// — any fault outside that range is passed on untouched.
//
// Copies 8 bytes at a time while it can, then one 4-, 2- and 1-byte move for
// the tail, so every aligned 1/2/4/8-byte read or write is still a single mov
// (a pointer, float or u32 the game is rewriting can't come back torn).
// ---------------------------------------------------------------------------
extern "C" int rsmm_fault_copy(void* dst, const void* src, std::size_t n);
extern "C" char rsmm_fault_copy_fault[];

asm(R"(
    .text
    .p2align 4
    .globl rsmm_fault_copy
rsmm_fault_copy:
1:  cmpq $8, %r8
    jb 2f
    movq (%rdx), %rax
    movq %rax, (%rcx)
    addq $8, %rdx
    addq $8, %rcx
    subq $8, %r8
    jmp 1b
2:  cmpq $4, %r8
    jb 4f
    movl (%rdx), %eax
    movl %eax, (%rcx)
    addq $4, %rdx
    addq $4, %rcx
    subq $4, %r8
4:  cmpq $2, %r8
    jb 5f
    movw (%rdx), %ax
    movw %ax, (%rcx)
    addq $2, %rdx
    addq $2, %rcx
    subq $2, %r8
5:  testq %r8, %r8
    jz 3f
    movb (%rdx), %al
    movb %al, (%rcx)
3:  movl $1, %eax
    ret
    .globl rsmm_fault_copy_fault
rsmm_fault_copy_fault:
    xorl %eax, %eax
    ret
)");

namespace {

LONG CALLBACK fault_copy_veh(EXCEPTION_POINTERS* ep) {
    const DWORD code = ep->ExceptionRecord->ExceptionCode;
    if (code != EXCEPTION_ACCESS_VIOLATION && code != EXCEPTION_IN_PAGE_ERROR &&
        code != EXCEPTION_GUARD_PAGE) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    const auto rip = ep->ContextRecord->Rip;
    const auto begin = reinterpret_cast<DWORD64>(&rsmm_fault_copy);
    const auto fault = reinterpret_cast<DWORD64>(&rsmm_fault_copy_fault);
    if (rip < begin || rip >= fault) return EXCEPTION_CONTINUE_SEARCH;
    // Leaf with no stack use: RSP still holds the caller's return address,
    // so jumping to the `xor eax,eax; ret` tail returns 0 to the caller.
    ep->ContextRecord->Rip = fault;
    return EXCEPTION_CONTINUE_EXECUTION;
}

void ensure_fault_handler() {
    // Thread-safe static init. FIRST in the chain so a crash reporter's
    // handler can't see a fault we are about to recover from. If registration
    // fails the copy still runs — that is the pre-handler behaviour, no worse.
    static PVOID handle = AddVectoredExceptionHandler(1, &fault_copy_veh);
    (void)handle;
}

} // namespace

bool mem_copy_from(void* dst, std::uintptr_t src, std::size_t n) {
    if (!dst || !mem_accessible(src, n, false)) return false;
    ensure_fault_handler();
    return rsmm_fault_copy(dst, reinterpret_cast<const void*>(src), n) != 0;
}

bool mem_copy_to(std::uintptr_t dst, const void* src, std::size_t n) {
    if (!src || !mem_accessible(dst, n, true)) return false;
    ensure_fault_handler();
    return rsmm_fault_copy(reinterpret_cast<void*>(dst), src, n) != 0;
}

bool mem_store_unchecked(std::uintptr_t dst, const void* src, std::size_t n) {
    if (!src || dst == 0 || n == 0) return false;
    ensure_fault_handler();
    return rsmm_fault_copy(reinterpret_cast<void*>(dst), src, n) != 0;
}

bool mem_accessible(std::uintptr_t addr, std::size_t size, bool need_write) {
    if (addr == 0 || size == 0) return false;
    // Reject the first 64 KiB (never mappable on Windows) and anything that
    // is not a canonical user-space address.
    if (addr < 0x10000) return false;
    if (addr >= kUserMax) return false;
    // Wrap guard: without it a near-max `addr` makes `addr + size` overflow,
    // the loop below runs zero iterations, and the function returns true
    // vacuously — handing the bad pointer straight through.
    if (size > kUserMax - addr) return false;

    const std::uintptr_t end = addr + size;
    for (std::uintptr_t a = addr; a < end; ) {
        MEMORY_BASIC_INFORMATION mbi{};
        if (VirtualQuery(reinterpret_cast<void*>(a), &mbi, sizeof(mbi)) == 0) return false;
        if (mbi.State != MEM_COMMIT) return false;
        // PAGE_GUARD raises a one-shot guard exception on touch (that is how
        // stack growth works) — reading it would fault the caller.
        if (mbi.Protect & (PAGE_NOACCESS | PAGE_GUARD)) return false;
        if (!(mbi.Protect & kReadable)) return false;
        if (need_write && !(mbi.Protect & kWritable)) return false;
        const auto region_end =
            reinterpret_cast<std::uintptr_t>(mbi.BaseAddress) + mbi.RegionSize;
        // A region that doesn't advance past `a` would spin forever, hanging
        // whichever game thread called in. VirtualQuery shouldn't return one,
        // but a hang is a worse failure than a refused read.
        if (region_end <= a) return false;
        a = region_end;
    }
    return true;
}

std::size_t mem_read_cstr(std::uintptr_t addr, char* out, std::size_t cap) {
    if (!out || cap == 0) return 0;
    out[0] = '\0';
    if (cap == 1) return 0;
    std::size_t n = 0;
    while (n < cap - 1) {
        // Probe in chunks rather than per byte: one VirtualQuery per 64 bytes
        // instead of per character. Halve on refusal so a string that ends
        // near a page boundary still copies its readable prefix.
        std::size_t chunk = cap - 1 - n;
        if (chunk > 64) chunk = 64;
        char p[64];
        while (chunk > 0 && !mem_copy_from(p, addr + n, chunk)) chunk /= 2;
        if (chunk == 0) break;
        for (std::size_t i = 0; i < chunk; ++i) {
            if (p[i] == '\0') { out[n] = '\0'; return n; }
            out[n++] = p[i];
        }
    }
    out[n] = '\0';
    return n;
}

} // namespace rsmm
