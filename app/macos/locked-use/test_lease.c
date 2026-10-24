#include "Lease.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

static const char *token = "01234567-89ab-cdef-0123-456789abcdef";
static SNTHLease pending(uint64_t ttl) {
    SNTHLease lease = {0};
    assert(snth_lease_begin(&lease, 1, 501, 123, token, "task-a", "synth-a", ttl, 100));
    return lease;
}
static SNTHLease protected(void) {
    SNTHLease lease = pending(SNTH_LEASE_MAX_MS);
    assert(!snth_lease_heartbeat(&lease, 0, token, 101));
    assert(!snth_lease_cancel(&lease, 0, token, false));
    assert(snth_lease_protect(&lease, 2, 501, 123, token, 110));
    return lease;
}

int main(void) {
    SNTHLease lease = pending(SNTH_LEASE_MAX_MS);
    assert(!snth_lease_consume(&lease, 501, 110)); // no protected desktop
    assert(!snth_lease_protect(&lease, 2, 502, 123, token, 110));
    assert(!snth_lease_protect(&lease, 2, 501, 124, token, 110));
    assert(!snth_lease_protect(&lease, 1, 501, 123, token, 110));
    assert(!snth_lease_protect(&lease, 2, 501, 123, "wrong", 110));
    assert(snth_lease_protect(&lease, 2, 501, 123, token, 110));
    assert(!snth_lease_consume(&lease, 502, 120));
    assert(snth_lease_consume(&lease, 501, 120));
    assert(!snth_lease_consume(&lease, 501, 121)); // replay
    assert(!snth_lease_begin(&lease, 3, 501, 123, token, "task-b", "synth-b", 100, 130));
    assert(!snth_lease_cancel(&lease, 3, token, false)); // other connection
    assert(!snth_lease_cancel(&lease, 1, token, true)); // owner cannot fake guardian input
    assert(snth_lease_cancel(&lease, 2, token, true));
    assert(lease.phase == SNTH_REVOKING);
    assert(!snth_lease_heartbeat(&lease, 1, token, 150));
    assert(!snth_lease_locked(&lease, 1, token));
    assert(snth_lease_locked(&lease, 2, token));
    assert(lease.phase == SNTH_IDLE && lease.manual_unlock_required);
    assert(!snth_lease_begin(&lease, 1, 501, 123, token, "task-a", "synth-a", 100, 200));
    assert(!snth_lease_manual_unlock(&lease, 502, 123));
    assert(!snth_lease_manual_unlock(&lease, 501, 124));
    assert(snth_lease_manual_unlock(&lease, 501, 123));

    for (uint64_t peer = 1; peer <= 2; peer++) {
        lease = protected();
        assert(snth_lease_consume(&lease, 501, 120));
        assert(snth_lease_disconnect(&lease, peer));
        assert(lease.phase == SNTH_REVOKING);
        assert(!snth_lease_consume(&lease, 501, 121));
    }
    lease = pending(100);
    assert(!snth_lease_protect(&lease, 2, 501, 123, token, 200));
    assert(lease.phase == SNTH_IDLE);

    lease = protected();
    assert(!snth_lease_heartbeat(&lease, 1, token, 2100)); // expired app heartbeat
    assert(lease.phase == SNTH_REVOKING);
    lease = protected();
    assert(snth_lease_heartbeat(&lease, 1, token, 2000));
    assert(!snth_lease_heartbeat(&lease, 2, token, 2110)); // expired guardian heartbeat
    assert(lease.phase == SNTH_REVOKING);

    // Heartbeats cannot extend the hard deadline, even if both peers stay alive.
    lease = protected();
    for (uint64_t now = 1000; now < 30000; now += 1000) {
        assert(snth_lease_heartbeat(&lease, 1, token, now));
        assert(snth_lease_heartbeat(&lease, 2, token, now));
    }
    assert(!snth_lease_consume(&lease, 501, 30100));
    assert(lease.phase == SNTH_REVOKING);

    lease = protected();
    assert(snth_lease_tick(&lease, 200000)); // sleep/wake cannot revive a grant
    assert(!snth_lease_consume(&lease, 501, 200001));
    lease = protected();
    assert(!snth_lease_heartbeat(&lease, 1, token, 99)); // clock regression
    assert(lease.phase == SNTH_REVOKING);

    memset(&lease, 0, sizeof(lease));
    assert(!snth_lease_begin(&lease, 1, 501, 123, token, "task", "synth", 0, 0));
    assert(!snth_lease_begin(&lease, 1, 501, 123, token, "task", "synth", SNTH_LEASE_MAX_MS + 1, 0));
    assert(!snth_lease_begin(&lease, 1, 501, 123, token, "task", "synth", 100, UINT64_MAX - 5));
    assert(!snth_lease_begin(&lease, 1, 0, 123, token, "task", "synth", 100, 0));
    assert(!snth_lease_begin(&lease, 1, 501, 0, token, "task", "synth", 100, 0));
    puts("lease lifecycle: binding, one-shot consumption, deadlines, heartbeat loss, disconnect and manual-unlock latch passed");
}
