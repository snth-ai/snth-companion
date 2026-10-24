// Privileged XPC transport for the memory-only task lease state machine.
// Not installed by the app. Unlock admission remains off until the protected
// desktop and independently verified re-lock watchdog are implemented.
#include "Lease.h"
#include <SystemConfiguration/SystemConfiguration.h>
#include <dispatch/dispatch.h>
#include <mach/mach_time.h>
#include <os/log.h>
#include <uuid/uuid.h>
#include <xpc/xpc.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#ifndef SNTH_TEAM_ID
#error "Set SNTH_TEAM_ID to the signing team"
#endif

#define BASE "ai.snth.connect.locked-use-broker"
#define SIGNED(ID) "anchor apple generic and identifier \"" ID "\" and certificate leaf[subject.OU] = \"" SNTH_TEAM_ID "\""
#define AUTH_HOST "anchor apple and (identifier \"com.apple.authorizationhost\" or identifier \"com.apple.authorizationhosthelper.arm64\" or identifier \"com.apple.authorizationhosthelper.x86_64\" or identifier \"com.apple.SecurityAgent\" or identifier \"com.apple.SecurityAgentHelper.arm64\" or identifier \"com.apple.SecurityAgentHelper.x86_64\")"

typedef enum { APP, GUARDIAN, AUTHORIZATION, VALIDATION } Role;
typedef struct Peer {
    uint64_t id;
    Role role;
    xpc_connection_t connection;
    char validation_token[SNTH_LEASE_TOKEN_SIZE];
} Peer;

static dispatch_queue_t queue;
static SNTHLease lease;
static uint64_t next_peer = 1;
static xpc_connection_t guardian_connection;
static uint64_t relock_notified;
// Only the separately signed, locally authenticated validation app can mint
// this five-second round-trip grant. Ordinary Connect tasks remain disabled.
static bool validation_active;

static uint64_t now_ms(void) {
    static mach_timebase_info_data_t timebase;
    if (!timebase.denom) mach_timebase_info(&timebase);
    return (uint64_t)((long double)mach_continuous_time() * timebase.numer / timebase.denom / 1000000.0L);
}

static uint32_t console_uid(void) {
    uid_t uid = 0;
    gid_t gid = 0;
    CFStringRef name = SCDynamicStoreCopyConsoleUser(NULL, &uid, &gid);
    if (!name) return 0;
    CFRelease(name);
    return uid;
}

static void request_relock(void) {
    if (lease.phase != SNTH_REVOKING) return;
    uint64_t now = now_ms();
    if (now >= relock_notified && now - relock_notified < 500) return;
    relock_notified = now;
    // Retain the revoking state until a trusted guardian verifies the lock.
    // No replacement grant is possible merely because a timer expired.
    if (guardian_connection) {
        xpc_object_t event = xpc_dictionary_create(NULL, NULL, 0);
        xpc_dictionary_set_uint64(event, "version", 1);
        xpc_dictionary_set_string(event, "event", "relock-required");
        xpc_dictionary_set_string(event, "token", lease.token);
        xpc_connection_send_message(guardian_connection, event);
        xpc_release(event);
    }
}

static void forget_guardian(void) {
    if (guardian_connection) xpc_release(guardian_connection);
    guardian_connection = NULL;
}

static void handle(Peer *peer, xpc_object_t request) {
    xpc_object_t reply = xpc_dictionary_create_reply(request);
    if (!reply) return; // no notification may mutate authorization state
    xpc_dictionary_set_uint64(reply, "version", 1);
    bool ok = false;
    const char *reason = "invalid_request";
    const char *op = xpc_dictionary_get_string(request, "operation");
    const char *token = xpc_dictionary_get_string(request, "token");
    uint32_t uid = xpc_connection_get_euid(peer->connection);
    uint32_t asid = xpc_connection_get_asid(peer->connection);
    uint32_t console = console_uid();
    uint64_t now = now_ms();
    if (lease.phase != SNTH_IDLE && console != lease.uid) {
        snth_lease_disconnect(&lease, lease.owner_peer);
    }
    snth_lease_tick(&lease, now);
    if (xpc_dictionary_get_uint64(request, "version") != 1 || !op) goto done;
    reason = "denied";

    if (strcmp(op, "status") == 0) {
        ok = true;
        xpc_dictionary_set_bool(reply, "unlock_ready", false);
        xpc_dictionary_set_uint64(reply, "maximum_grant_ms", SNTH_LEASE_MAX_MS);
        // Do not expose task/synth names or another connection's token.
        xpc_dictionary_set_string(reply, "runtime", "protected-desktop-not-integrated");
    } else if ((peer->role == APP && strcmp(op, "prepare-task") == 0) ||
               (peer->role == VALIDATION && strcmp(op, "prepare-validation") == 0)) {
        uuid_t bytes;
        char issued[SNTH_LEASE_TOKEN_SIZE];
        uuid_generate_random(bytes);
        uuid_unparse_lower(bytes, issued);
        uint64_t ttl = peer->role == VALIDATION ? 5000 : xpc_dictionary_get_uint64(request, "ttl_ms");
        if (uid == console) {
            ok = snth_lease_begin(&lease, peer->id, uid, asid, issued,
                peer->role == VALIDATION ? "local-roundtrip-validation" : xpc_dictionary_get_string(request, "task"),
                peer->role == VALIDATION ? "local-owner" : xpc_dictionary_get_string(request, "synth"), ttl, now);
        }
        if (ok) {
            validation_active = peer->role == VALIDATION;
            if (validation_active) memcpy(peer->validation_token, issued, sizeof(issued));
            xpc_dictionary_set_string(reply, "token", issued);
            xpc_dictionary_set_uint64(reply, "heartbeat_interval_ms", 500);
            xpc_dictionary_set_uint64(reply, "ttl_ms", lease.deadline - now);
        }
    } else if (peer->role == VALIDATION && strcmp(op, "validation-state") == 0) {
        if (token && peer->validation_token[0] && strcmp(token, peer->validation_token) == 0) {
            ok = true;
            bool mine = validation_active && lease.owner_peer == peer->id && strcmp(lease.token, token) == 0;
            xpc_dictionary_set_bool(reply, "protected", mine && lease.phase == SNTH_PROTECTED);
            xpc_dictionary_set_bool(reply, "consumed", mine && lease.phase == SNTH_CONSUMED);
            xpc_dictionary_set_bool(reply, "finished", !mine || lease.phase == SNTH_IDLE);
            xpc_dictionary_set_bool(reply, "revoking", mine && lease.phase == SNTH_REVOKING);
        }
    } else if (peer->role == GUARDIAN && strcmp(op, "desktop-protected") == 0) {
        if (uid == console) ok = snth_lease_protect(&lease, peer->id, uid, asid, token, now);
        if (ok) {
            forget_guardian();
            guardian_connection = peer->connection;
            xpc_retain(guardian_connection);
        }
    } else if (peer->role != AUTHORIZATION && strcmp(op, "heartbeat") == 0) {
        ok = snth_lease_heartbeat(&lease, peer->id, token, now);
    } else if (peer->role != AUTHORIZATION && strcmp(op, "cancel-task") == 0) {
        ok = snth_lease_cancel(&lease, peer->id, token, false);
    } else if (peer->role == GUARDIAN && strcmp(op, "local-input") == 0) {
        ok = snth_lease_cancel(&lease, peer->id, token, true);
    } else if (peer->role == GUARDIAN && strcmp(op, "screen-locked") == 0) {
        ok = snth_lease_locked(&lease, peer->id, token);
        if (ok) forget_guardian();
    } else if (peer->role == GUARDIAN && strcmp(op, "manual-unlock") == 0) {
        if (uid == console) ok = snth_lease_manual_unlock(&lease, uid, asid);
    } else if (peer->role == AUTHORIZATION && strcmp(op, "consume-pending-unlock") == 0) {
        uint64_t requested_uid = xpc_dictionary_get_uint64(request, "uid");
        xpc_dictionary_set_uint64(reply, "uid", requested_uid);
        ok = validation_active && requested_uid == console && snth_lease_consume(&lease, console, now);
        xpc_dictionary_set_bool(reply, "granted", ok);
        // Production grants remain disabled. Validation is a distinct signed
        // client which performs LocalAuthentication for each single round trip.
        reason = requested_uid == console ? "no_protected_validation_grant" : "wrong_console_user";
    }
done:
    request_relock();
    xpc_dictionary_set_bool(reply, "ok", ok);
    if (!ok) xpc_dictionary_set_string(reply, "reason", reason);
    xpc_connection_send_message(peer->connection, reply);
    xpc_release(reply);
}

static void accept_peer(xpc_connection_t connection, Role role) {
    const char *requirement = role == APP ? SIGNED("ai.snth.connect") :
        role == GUARDIAN ? SIGNED("ai.snth.connect.locked-use-guardian") :
        role == VALIDATION ? SIGNED("ai.snth.connect.locked-use-validation") : AUTH_HOST;
    if (xpc_connection_set_peer_code_signing_requirement(connection, requirement) != 0 || !next_peer) {
        xpc_connection_cancel(connection);
        return;
    }
    Peer *peer = calloc(1, sizeof(*peer));
    if (!peer) { xpc_connection_cancel(connection); return; }
    *peer = (Peer){ .id = next_peer++, .role = role, .connection = connection };
    xpc_retain(connection);
    xpc_connection_set_target_queue(connection, queue);
    xpc_connection_set_event_handler(connection, ^(xpc_object_t event) {
        if (xpc_get_type(event) == XPC_TYPE_DICTIONARY) {
            handle(peer, event);
        } else if (xpc_get_type(event) == XPC_TYPE_ERROR) {
            if (snth_lease_disconnect(&lease, peer->id)) {
                request_relock();
                if (peer->id == lease.guardian_peer) {
                    forget_guardian();
                    os_log_error(OS_LOG_DEFAULT, "SNTH Locked use: guardian disconnected; unlock remains unavailable");
                }
            }
            if (event == XPC_ERROR_CONNECTION_INVALID) {
                xpc_release(peer->connection);
                free(peer);
            } else {
                // The final INVALID event releases our connection reference.
                xpc_connection_cancel(peer->connection);
            }
        }
    });
    xpc_connection_resume(connection);
}

static xpc_connection_t listen_service(const char *name, Role role) {
    xpc_connection_t listener = xpc_connection_create_mach_service(name, queue, XPC_CONNECTION_MACH_SERVICE_LISTENER);
    if (!listener) return NULL;
    xpc_connection_set_event_handler(listener, ^(xpc_object_t event) {
        if (xpc_get_type(event) == XPC_TYPE_CONNECTION) accept_peer((xpc_connection_t)event, role);
    });
    xpc_connection_resume(listener);
    return listener;
}

int main(void) {
    if (geteuid() != 0) {
        static const char message[] = "Run via the root launchd service; this tool does not install itself.\n";
        write(STDERR_FILENO, message, sizeof(message) - 1);
        return 1;
    }
    queue = dispatch_queue_create(BASE, DISPATCH_QUEUE_SERIAL);
    if (!listen_service(BASE, AUTHORIZATION) || !listen_service(BASE ".app", APP) ||
        !listen_service(BASE ".guardian", GUARDIAN) || !listen_service(BASE ".validation", VALIDATION)) return 1;
    dispatch_source_t timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, queue);
    dispatch_source_set_timer(timer, DISPATCH_TIME_NOW, 100 * NSEC_PER_MSEC, 10 * NSEC_PER_MSEC);
    dispatch_source_set_event_handler(timer, ^{
        if (lease.phase != SNTH_IDLE && console_uid() != lease.uid) snth_lease_disconnect(&lease, lease.owner_peer);
        if (snth_lease_tick(&lease, now_ms())) request_relock();
    });
    dispatch_resume(timer);
    dispatch_main();
}
