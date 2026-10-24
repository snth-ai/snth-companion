#pragma once
#include <stdbool.h>
#include <stdint.h>

// Memory-only state. The broker calls this exclusively on its serial queue.
// Times are continuous monotonic milliseconds (including machine sleep).
#define SNTH_LEASE_MAX_MS 30000
#define SNTH_LEASE_HEARTBEAT_MS 2000
#define SNTH_LEASE_TOKEN_SIZE 37
#define SNTH_LEASE_LABEL_SIZE 129

typedef enum { SNTH_IDLE, SNTH_PREPARING, SNTH_PROTECTED, SNTH_CONSUMED, SNTH_REVOKING } SNTHPhase;
typedef struct {
    SNTHPhase phase;
    uint64_t owner_peer, guardian_peer;
    uint32_t uid, audit_session;
    uint64_t deadline, owner_seen, guardian_seen;
    bool manual_unlock_required;
    char token[SNTH_LEASE_TOKEN_SIZE];
    char task[SNTH_LEASE_LABEL_SIZE];
    char synth[SNTH_LEASE_LABEL_SIZE];
} SNTHLease;

bool snth_lease_begin(SNTHLease *lease, uint64_t peer, uint32_t uid, uint32_t audit_session,
    const char *token, const char *task, const char *synth, uint64_t ttl_ms, uint64_t now);
bool snth_lease_protect(SNTHLease *lease, uint64_t peer, uint32_t uid, uint32_t audit_session,
    const char *token, uint64_t now);
bool snth_lease_heartbeat(SNTHLease *lease, uint64_t peer, const char *token, uint64_t now);
bool snth_lease_consume(SNTHLease *lease, uint32_t console_uid, uint64_t now);
bool snth_lease_cancel(SNTHLease *lease, uint64_t peer, const char *token, bool local_input);
bool snth_lease_disconnect(SNTHLease *lease, uint64_t peer);
bool snth_lease_tick(SNTHLease *lease, uint64_t now);
bool snth_lease_locked(SNTHLease *lease, uint64_t guardian, const char *token);
bool snth_lease_manual_unlock(SNTHLease *lease, uint32_t uid, uint32_t audit_session);
