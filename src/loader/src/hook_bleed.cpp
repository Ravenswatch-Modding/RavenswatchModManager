// Bleed ring-buffer redistribution hook for shortened Bleed durations.
//
// In Ravenswatch.exe (2a88a3bd5647), RemoteDamageTarget_OnModifierAdded calls
// 0x1403d6210, which hardcodes `divss xmm3, 10.0f` and spreads the Bleed total
// across all 10 x 0.5 s slots (+0x68..+0x8c) regardless of modifier duration.
// RemoteDamageTarget_OnModifierRemoved clears bIsBleeding (+0x90) on expiry
// without clearing slots[0..9] or Remaining Bleed Damage Value (+0x1d0).
//
// For any Bleed modifier with 0 < duration < 4.99 s, this hook redistributes
// the newly added Bleed delta across round(duration / 0.5) slots and updates
// the HP-bar preview; on final Bleed removal it zeroes any residual slots.
// Vanilla 5.0 s and -1.0 s bleeds are left untouched.

#include "hook_bleed.h"

#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <mutex>
#include <unordered_set>

#include "hook_util.h"
#include "loader.h"
#include "mem_safe.h"
#include "symbols.gen.h"

namespace rsmm {
namespace {

constexpr std::uint32_t kBleedStatKey      = 0x173fcdacu;
constexpr std::size_t   kSlotsOffset       = 0x68;   // float[10], 10 x 0.5 s ticks
constexpr std::size_t   kIsBleedingOffset  = 0x90;   // uint8_t bIsBleeding
constexpr std::size_t   kObservableOffset  = 0x1d0;  // FloatObservable (+0x00..+0x18)
constexpr std::size_t   kObservableSize    = 0x18;
constexpr std::size_t   kHolderItemsOffset = 0x20;   // oCEntityValueModifier**
constexpr std::size_t   kHolderCountOffset = 0x28;   // uint32_t
constexpr std::size_t   kModDurationOffset = 0x3c;   // float duration
constexpr std::size_t   kModStartOffset    = 0x40;   // float start_time
constexpr float         kTickInterval      = 0.5f;
constexpr int           kSlotCount         = 10;
constexpr std::uint32_t kMaxModifiers      = 4096u;

using OnModFn     = void (*)(void* target, void* store, std::uint32_t stat_key);
using GetHolderFn = void* (*)(void* store, std::uint32_t stat_key);
using SetObsFn    = void (*)(void* observable, float value);

OnModFn                   g_orig_add       = nullptr;
OnModFn                   g_orig_remove    = nullptr;
GetHolderFn                 g_get_holder     = nullptr;
SetObsFn                    g_set_observable = nullptr;
std::uintptr_t              g_add_va         = 0;
std::uintptr_t              g_rem_va         = 0;
std::mutex                  g_redistributed_mu;
std::unordered_set<void*>   g_redistributed_targets;

float latest_bleed_duration(void* store) {
    if (!g_get_holder || !store) return -1.0f;
    void* holder = g_get_holder(store, kBleedStatKey);
    if (!holder) return -1.0f;

    const auto* h = static_cast<const char*>(holder);
    std::uint32_t count = 0;
    void* items = nullptr;
    if (!mem_load(h + kHolderCountOffset, &count) || count == 0 || count > kMaxModifiers) {
        return -1.0f;
    }
    if (!mem_load(h + kHolderItemsOffset, &items) || !items) {
        return -1.0f;
    }
    if (!mem_readable(items, static_cast<std::size_t>(count) * sizeof(void*))) {
        return -1.0f;
    }

    const auto* item_ptrs = static_cast<const char*>(items);
    bool found = false;
    float best_start = 0.0f;
    float best_dur = -1.0f;
    for (std::uint32_t i = 0; i < count; ++i) {
        void* mod = nullptr;
        if (!mem_load(item_ptrs + static_cast<std::size_t>(i) * sizeof(void*), &mod) || !mod) {
            continue;
        }
        const auto* m = static_cast<const char*>(mod);
        float dur = -1.0f;
        float start = 0.0f;
        if (!mem_load(m + kModDurationOffset, &dur) || !mem_load(m + kModStartOffset, &start)) {
            continue;
        }
        if (!found || start >= best_start) {
            best_start = start;
            best_dur = dur;
            found = true;
        }
    }
    return best_dur;
}

void detour_on_modifier_added(void* target, void* store, std::uint32_t stat_key) {
    if (!g_orig_add) return;
    if (stat_key != kBleedStatKey || !target || !store) {
        g_orig_add(target, store, stat_key);
        return;
    }

    hook_note_fire(g_add_va);
    auto* base = static_cast<char*>(target);
    auto* slots = reinterpret_cast<float*>(base + kSlotsOffset);
    if (!mem_writable(slots, sizeof(float) * kSlotCount)) {
        g_orig_add(target, store, stat_key);
        return;
    }

    float before[kSlotCount];
    std::memcpy(before, slots, sizeof(before));

    g_orig_add(target, store, stat_key);

    std::uint8_t bleeding = 0;
    if (!mem_load(base + kIsBleedingOffset, &bleeding) || bleeding == 0) {
        return;
    }

    const float per_slot = slots[0] - before[0];
    if (!(per_slot > 1e-6f)) {
        return;
    }

    // Vanilla guard: leave 5.0 s and -1.0 s Bleed modifiers untouched.
    const float dur = latest_bleed_duration(store);
    if (!std::isfinite(dur) || dur <= 0.0f || dur >= 4.99f) {
        return;
    }

    const int n = std::clamp(static_cast<int>(std::lround(dur / kTickInterval)), 1, kSlotCount);
    if (n >= kSlotCount) {
        return;
    }

    const float new_per_slot = (per_slot * static_cast<float>(kSlotCount)) / static_cast<float>(n);
    float sum = 0.0f;
    for (int i = 0; i < kSlotCount; ++i) {
        const float v = before[i] + (i < n ? new_per_slot : 0.0f);
        slots[i] = v;
        sum += v;
    }
    if (g_set_observable && mem_writable(base + kObservableOffset, kObservableSize)) {
        g_set_observable(base + kObservableOffset, sum);
    }
    {
        std::lock_guard<std::mutex> lock(g_redistributed_mu);
        if (g_redistributed_targets.size() > 4096) {
            g_redistributed_targets.clear();
        }
        g_redistributed_targets.insert(target);
    }
}

void detour_on_modifier_removed(void* target, void* store, std::uint32_t stat_key) {
    if (g_orig_remove) {
        g_orig_remove(target, store, stat_key);
    }
    if (stat_key != kBleedStatKey || !target) {
        return;
    }

    hook_note_fire(g_rem_va);
    auto* base = static_cast<char*>(target);
    std::uint8_t bleeding = 1;
    if (!mem_load(base + kIsBleedingOffset, &bleeding) || bleeding != 0) {
        return;
    }
    {
        std::lock_guard<std::mutex> lock(g_redistributed_mu);
        if (g_redistributed_targets.erase(target) == 0) {
            return;
        }
    }

    auto* slots = reinterpret_cast<float*>(base + kSlotsOffset);
    if (!mem_writable(slots, sizeof(float) * kSlotCount)) {
        return;
    }

    bool any_nonzero = false;
    for (int i = 0; i < kSlotCount; ++i) {
        if (slots[i] != 0.0f) {
            any_nonzero = true;
            break;
        }
    }
    if (!any_nonzero) {
        return;
    }

    for (int i = 0; i < kSlotCount; ++i) {
        slots[i] = 0.0f;
    }
    if (g_set_observable && mem_writable(base + kObservableOffset, kObservableSize)) {
        g_set_observable(base + kObservableOffset, 0.0f);
    }
}

} // namespace

void install_bleed_hooks() {
    if (!resolve_checked("bleed", "EntityValueStore::GetModifierHolder",
                         Sym::EntityValueStore_GetModifierHolder_Pattern, &g_get_holder)
        || !resolve_checked("bleed", "FloatObservable::Set",
                            Sym::FloatObservable_Set_Pattern, &g_set_observable)) {
        Loader::get().log_warn("[bleed] helper symbols unresolved; duration hooks disabled");
        g_get_holder = nullptr;
        g_set_observable = nullptr;
        return;
    }

    if (!hook_install("bleed", "RemoteDamageTarget::OnModifierAdded",
                      Sym::RemoteDamageTarget_OnModifierAdded_Pattern,
                      reinterpret_cast<void*>(&detour_on_modifier_added),
                      reinterpret_cast<void**>(&g_orig_add), &g_add_va)) {
        g_get_holder = nullptr;
        g_set_observable = nullptr;
        return;
    }

    if (!hook_install("bleed", "RemoteDamageTarget::OnModifierRemoved",
                      Sym::RemoteDamageTarget_OnModifierRemoved_Pattern,
                      reinterpret_cast<void*>(&detour_on_modifier_removed),
                      reinterpret_cast<void**>(&g_orig_remove), &g_rem_va)) {
        hook_remove(g_add_va);
        g_add_va = 0;
        g_orig_add = nullptr;
        g_get_holder = nullptr;
        g_set_observable = nullptr;
        return;
    }

    Loader::get().log("[bleed] duration hooks armed");
}

void remove_bleed_hooks() {
    if (g_rem_va) {
        hook_remove(g_rem_va);
        g_rem_va = 0;
    }
    if (g_add_va) {
        hook_remove(g_add_va);
        g_add_va = 0;
    }
    g_orig_add = nullptr;
    g_orig_remove = nullptr;
    g_get_holder = nullptr;
    g_set_observable = nullptr;
    {
        std::lock_guard<std::mutex> lock(g_redistributed_mu);
        g_redistributed_targets.clear();
    }
}

} // namespace rsmm
