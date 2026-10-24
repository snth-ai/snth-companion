#include "GuardianState.h"
#include "Lease.h"

bool snth_guardian_start(SNTHGuardianState *s, SNTHLockState lock, uint64_t now) {
    if (s->started || lock != SNTH_LOCKED || now > UINT64_MAX - SNTH_LEASE_MAX_MS) return false;
    *s = (SNTHGuardianState){ .started = true, .deadline = now + SNTH_LEASE_MAX_MS,
        .broker_seen = now, .last_tick = now };
    return true;
}

void snth_guardian_revoke(SNTHGuardianState *s, bool local_input) {
    if (!s->started) return;
    s->revoking = true;
    s->local_input |= local_input;
}

void snth_guardian_reply(SNTHGuardianState *s, bool ok, uint64_t now) {
    if (!s->started || s->revoking) return;
    // A late reply cannot resurrect a failed or expired lease.
    if (!ok || now < s->broker_seen || now - s->broker_seen >= SNTH_LEASE_HEARTBEAT_MS ||
        now >= s->deadline) snth_guardian_revoke(s, false);
    else s->broker_seen = now;
}

void snth_guardian_tick(SNTHGuardianState *s, SNTHLockState lock, bool healthy, uint64_t now) {
    if (!s->started) return;
    if (!healthy || lock == SNTH_LOCK_UNKNOWN || now < s->last_tick || now < s->broker_seen ||
        now - s->broker_seen >= SNTH_LEASE_HEARTBEAT_MS || now >= s->deadline)
        snth_guardian_revoke(s, false);
    s->last_tick = now;
}

bool snth_guardian_can_uncover(const SNTHGuardianState *s, SNTHLockState lock) {
    return s->started && s->revoking && s->drained && lock == SNTH_LOCKED;
}
