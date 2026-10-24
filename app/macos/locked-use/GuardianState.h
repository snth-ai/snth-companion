#pragma once
#include <stdbool.h>
#include <stdint.h>

// The UI adapter runs all transitions on its main queue. UNKNOWN never proves
// that it is safe to uncover the desktop. No transition can re-arm a session.
typedef enum { SNTH_LOCK_UNKNOWN, SNTH_LOCKED, SNTH_UNLOCKED } SNTHLockState;
typedef struct {
    bool started, revoking, local_input, drained;
    uint64_t deadline, broker_seen, last_tick;
} SNTHGuardianState;

bool snth_guardian_start(SNTHGuardianState *state, SNTHLockState lock, uint64_t now);
void snth_guardian_revoke(SNTHGuardianState *state, bool local_input);
void snth_guardian_reply(SNTHGuardianState *state, bool ok, uint64_t now);
void snth_guardian_tick(SNTHGuardianState *state, SNTHLockState lock,
    bool protection_healthy, uint64_t now);
bool snth_guardian_can_uncover(const SNTHGuardianState *state, SNTHLockState lock);
