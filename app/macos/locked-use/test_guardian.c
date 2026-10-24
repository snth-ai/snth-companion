#include "GuardianState.h"
#include <assert.h>
#include <stdio.h>

static SNTHGuardianState start(void) {
    SNTHGuardianState s = {0};
    assert(!snth_guardian_start(&s, SNTH_LOCK_UNKNOWN, 100));
    assert(!snth_guardian_start(&s, SNTH_UNLOCKED, 100));
    assert(snth_guardian_start(&s, SNTH_LOCKED, 100));
    return s;
}

int main(void) {
    SNTHGuardianState s = start();
    assert(!snth_guardian_start(&s, SNTH_LOCKED, 101));
    snth_guardian_tick(&s, SNTH_UNLOCKED, true, 200);
    assert(!s.revoking); // existing session may unlock while protected
    snth_guardian_revoke(&s, true);
    assert(s.local_input && s.revoking);
    assert(!snth_guardian_can_uncover(&s, SNTH_LOCKED)); // pending grant not drained
    s.drained = true;
    assert(!snth_guardian_can_uncover(&s, SNTH_UNLOCKED));
    assert(!snth_guardian_can_uncover(&s, SNTH_LOCK_UNKNOWN));
    assert(snth_guardian_can_uncover(&s, SNTH_LOCKED));
    snth_guardian_reply(&s, true, 201);
    assert(s.revoking && s.local_input); // delayed success never re-arms

    s = start();
    snth_guardian_reply(&s, true, 2100);
    assert(s.revoking); // late reply cannot conceal missing watchdog heartbeat
    s = start();
    snth_guardian_tick(&s, SNTH_LOCKED, true, 2100);
    assert(s.revoking);
    s = start();
    snth_guardian_tick(&s, SNTH_LOCK_UNKNOWN, true, 101);
    assert(s.revoking);
    s = start();
    snth_guardian_tick(&s, SNTH_LOCKED, false, 101);
    assert(s.revoking); // tap, display, or permissions lost
    s = start();
    snth_guardian_tick(&s, SNTH_LOCKED, true, 99);
    assert(s.revoking); // clock regression
    s = start();
    for (uint64_t now = 600; now < 30100; now += 500) {
        snth_guardian_reply(&s, true, now);
        snth_guardian_tick(&s, SNTH_UNLOCKED, true, now);
        assert(!s.revoking);
    }
    snth_guardian_tick(&s, SNTH_UNLOCKED, true, 30100);
    assert(s.revoking); // healthy heartbeats do not extend hard limit
    s = start();
    snth_guardian_tick(&s, SNTH_UNLOCKED, true, 100000);
    assert(s.revoking); // sleep/wake
    s = start();
    snth_guardian_reply(&s, false, 101);
    assert(s.revoking);
    s = (SNTHGuardianState){0};
    assert(!snth_guardian_start(&s, SNTH_LOCKED, UINT64_MAX));
    puts("guardian lifecycle tests passed");
}
