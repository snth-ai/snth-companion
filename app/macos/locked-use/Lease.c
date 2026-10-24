#include "Lease.h"
#include <string.h>

static bool matches(const SNTHLease *lease, const char *token) {
    return lease->phase != SNTH_IDLE && token &&
        strnlen(token, SNTH_LEASE_TOKEN_SIZE) == SNTH_LEASE_TOKEN_SIZE - 1 &&
        memcmp(token, lease->token, SNTH_LEASE_TOKEN_SIZE) == 0;
}

static bool recent(uint64_t now, uint64_t then) {
    return now >= then && now - then < SNTH_LEASE_HEARTBEAT_MS;
}

static bool live(const SNTHLease *lease, uint64_t now) {
    return now < lease->deadline && recent(now, lease->owner_seen) &&
        (!lease->guardian_peer || recent(now, lease->guardian_seen));
}

static void revoke(SNTHLease *lease) {
    if (lease->phase == SNTH_IDLE || lease->phase == SNTH_REVOKING) return;
    // Without a guardian nothing was allowed to unlock or draw a cover.
    // Otherwise retain identity until that guardian confirms re-lock.
    if (!lease->guardian_peer && lease->phase == SNTH_PREPARING) {
        memset(lease, 0, sizeof(*lease));
    } else {
        lease->phase = SNTH_REVOKING;
    }
}

bool snth_lease_begin(SNTHLease *lease, uint64_t peer, uint32_t uid, uint32_t audit_session,
    const char *token, const char *task, const char *synth, uint64_t ttl_ms, uint64_t now) {
    if (lease->phase != SNTH_IDLE || lease->manual_unlock_required || !peer || uid < 501 ||
        !audit_session || !token || strnlen(token, SNTH_LEASE_TOKEN_SIZE) != SNTH_LEASE_TOKEN_SIZE - 1 ||
        !task || !*task || strnlen(task, SNTH_LEASE_LABEL_SIZE) >= SNTH_LEASE_LABEL_SIZE ||
        !synth || !*synth || strnlen(synth, SNTH_LEASE_LABEL_SIZE) >= SNTH_LEASE_LABEL_SIZE ||
        ttl_ms == 0 || ttl_ms > SNTH_LEASE_MAX_MS || now > UINT64_MAX - ttl_ms) return false;
    *lease = (SNTHLease){ .phase = SNTH_PREPARING, .owner_peer = peer, .uid = uid,
        .audit_session = audit_session, .deadline = now + ttl_ms, .owner_seen = now };
    memcpy(lease->token, token, SNTH_LEASE_TOKEN_SIZE);
    strcpy(lease->task, task);
    strcpy(lease->synth, synth);
    return true;
}

bool snth_lease_protect(SNTHLease *lease, uint64_t peer, uint32_t uid, uint32_t audit_session,
    const char *token, uint64_t now) {
    if (lease->phase != SNTH_PREPARING || !matches(lease, token) || !peer || peer == lease->owner_peer ||
        uid != lease->uid || audit_session != lease->audit_session) return false;
    if (!live(lease, now)) { revoke(lease); return false; }
    lease->guardian_peer = peer;
    lease->guardian_seen = now;
    lease->phase = SNTH_PROTECTED;
    return true;
}

bool snth_lease_heartbeat(SNTHLease *lease, uint64_t peer, const char *token, uint64_t now) {
    if (!peer || lease->phase == SNTH_REVOKING || !matches(lease, token) ||
        (peer != lease->owner_peer && peer != lease->guardian_peer)) return false;
    if (!live(lease, now)) { revoke(lease); return false; }
    if (peer == lease->owner_peer) lease->owner_seen = now;
    else lease->guardian_seen = now;
    return true;
}

bool snth_lease_consume(SNTHLease *lease, uint32_t console_uid, uint64_t now) {
    if (lease->phase != SNTH_PROTECTED || console_uid != lease->uid) return false;
    if (!live(lease, now)) { revoke(lease); return false; }
    lease->phase = SNTH_CONSUMED;
    return true;
}

bool snth_lease_cancel(SNTHLease *lease, uint64_t peer, const char *token, bool local_input) {
    if (!peer || !matches(lease, token) || (peer != lease->owner_peer && peer != lease->guardian_peer)) return false;
    // Only the independently authenticated guardian may report physical input.
    if (local_input && peer != lease->guardian_peer) return false;
    if (local_input) lease->manual_unlock_required = true;
    revoke(lease);
    return true;
}

bool snth_lease_disconnect(SNTHLease *lease, uint64_t peer) {
    if (lease->phase == SNTH_IDLE || !peer || (peer != lease->owner_peer && peer != lease->guardian_peer)) return false;
    revoke(lease);
    return true;
}

bool snth_lease_tick(SNTHLease *lease, uint64_t now) {
    if (lease->phase == SNTH_IDLE) return false;
    if (lease->phase != SNTH_REVOKING && !live(lease, now)) revoke(lease);
    return lease->phase == SNTH_REVOKING;
}

bool snth_lease_locked(SNTHLease *lease, uint64_t guardian, const char *token) {
    if (lease->phase != SNTH_REVOKING || !matches(lease, token) || !guardian || guardian != lease->guardian_peer) return false;
    // Keep the manual-unlock latch scoped to the original console session.
    bool manual = lease->manual_unlock_required;
    uint32_t uid = lease->uid, audit_session = lease->audit_session;
    memset(lease, 0, sizeof(*lease));
    if (manual) {
        lease->manual_unlock_required = true;
        lease->uid = uid;
        lease->audit_session = audit_session;
    }
    return true;
}

bool snth_lease_manual_unlock(SNTHLease *lease, uint32_t uid, uint32_t audit_session) {
    if (lease->phase != SNTH_IDLE || !lease->manual_unlock_required ||
        uid != lease->uid || audit_session != lease->audit_session) return false;
    memset(lease, 0, sizeof(*lease));
    return true;
}
