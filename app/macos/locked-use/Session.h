#pragma once
#include "GuardianState.h"
#include <CoreFoundation/CoreFoundation.h>
#include <sys/types.h>

SNTHLockState snth_session_decode(CFDictionaryRef session, uid_t expected_uid);
SNTHLockState snth_session_lock_state(void);
bool snth_session_can_request_lock(void);
// A request is not evidence of a completed lock/unlock. Always observe state.
bool snth_session_request_lock(void);
typedef enum {
    SNTH_UNLOCK_SUBMITTED, SNTH_UNLOCK_NOT_LOCKED, SNTH_UNLOCK_NO_ACCESS,
    SNTH_UNLOCK_NO_LOGINWINDOW, SNTH_UNLOCK_NO_CONTROL, SNTH_UNLOCK_ACTION_FAILED
} SNTHUnlockRequest;
SNTHUnlockRequest snth_session_request_unlock(void);
