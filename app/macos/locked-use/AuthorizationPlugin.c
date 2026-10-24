// The native authorization boundary. This bundle is not installed by the
// application yet: a signed broker and protected-desktop lifecycle are required.
// It never reads a password and never accepts a grant directly from a synth.
#include <Security/AuthorizationPlugin.h>
#include <SystemConfiguration/SystemConfiguration.h>
#include <dispatch/dispatch.h>
#include <xpc/xpc.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#ifndef SNTH_TEAM_ID
#error "Set SNTH_TEAM_ID to the Developer ID team used to sign the broker"
#endif

#define BROKER "ai.snth.connect.locked-use-broker"
#define BROKER_REQUIREMENT "anchor apple generic and identifier \"" BROKER "\" and certificate leaf[subject.OU] = \"" SNTH_TEAM_ID "\""

typedef struct { const AuthorizationCallbacks *callbacks; } Plugin;
typedef struct {
    Plugin *plugin;
    AuthorizationEngineRef engine;
} Mechanism;

// Only an authenticated root broker may consume a pending, single-use grant.
// A missing service, invalid signature, malformed reply, or timeout declines
// this branch. The system's original unlock rule is a separate alternative.
static bool consume_pending(uid_t uid) {
    xpc_connection_t connection = xpc_connection_create_mach_service(
        BROKER, dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0),
        XPC_CONNECTION_MACH_SERVICE_PRIVILEGED);
    if (!connection) return false;
    if (xpc_connection_set_peer_code_signing_requirement(connection, BROKER_REQUIREMENT) != 0) {
        xpc_release(connection);
        return false;
    }
    xpc_connection_set_event_handler(connection, ^(xpc_object_t event) { (void)event; });
    xpc_connection_resume(connection);
    xpc_object_t request = xpc_dictionary_create(NULL, NULL, 0);
    xpc_dictionary_set_uint64(request, "version", 1);
    xpc_dictionary_set_string(request, "operation", "consume-pending-unlock");
    xpc_dictionary_set_uint64(request, "uid", uid);
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    // The callback owns its semaphore reference even when this wait times out.
    dispatch_retain(done);
    __block bool allowed = false;
    xpc_connection_send_message_with_reply(connection, request,
        dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^(xpc_object_t reply) {
            if (xpc_get_type(reply) == XPC_TYPE_DICTIONARY &&
                xpc_dictionary_get_uint64(reply, "version") == 1 &&
                xpc_dictionary_get_uint64(reply, "uid") == uid) {
                allowed = xpc_dictionary_get_bool(reply, "granted");
            }
            dispatch_semaphore_signal(done);
            dispatch_release(done);
        });
    bool completed = dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 250 * NSEC_PER_MSEC)) == 0;
    // Never read the callback's result after a timeout.
    bool result = completed && allowed;
    xpc_connection_cancel(connection);
    xpc_release(request);
    xpc_release(connection);
    dispatch_release(done);
    return result;
}

static OSStatus plugin_destroy(AuthorizationPluginRef ref) {
    free(ref);
    return errAuthorizationSuccess;
}

static OSStatus mechanism_create(AuthorizationPluginRef plugin, AuthorizationEngineRef engine,
                                 AuthorizationMechanismId name, AuthorizationMechanismRef *out) {
    if (!plugin || !engine || !out || !name) return errAuthorizationInternal;
    *out = NULL;
    if (strcmp(name, "allow") != 0) return errAuthorizationInternal;
    Mechanism *mechanism = calloc(1, sizeof(*mechanism));
    if (!mechanism) return errAuthorizationInternal;
    mechanism->plugin = plugin;
    mechanism->engine = engine;
    *out = mechanism;
    return errAuthorizationSuccess;
}

static OSStatus mechanism_invoke(AuthorizationMechanismRef ref) {
    if (!ref) return errAuthorizationInternal;
    Mechanism *mechanism = ref;
    const AuthorizationCallbacks *callbacks = mechanism->plugin->callbacks;
    const AuthorizationValue *value = NULL;
    AuthorizationContextFlags flags = 0;
    uid_t target = 0;
    bool allowed = false;
    // Use engine context, never an advisory hint supplied by a requester.
    if (callbacks->GetContextValue(mechanism->engine, "uid", &flags, &value) == errAuthorizationSuccess &&
        value && value->data && value->length == sizeof(target)) {
        memcpy(&target, value->data, sizeof(target));
        uid_t console_uid = 0;
        gid_t console_gid = 0;
        CFStringRef user = SCDynamicStoreCopyConsoleUser(NULL, &console_uid, &console_gid);
        if (user) {
            // Exclude loginwindow/root and non-console sessions. This plug-in
            // is only a screensaver-unlock branch, never boot-time login.
            if (target >= 501 && console_uid == target) allowed = consume_pending(target);
            CFRelease(user);
        }
    }
    return callbacks->SetResult(mechanism->engine,
        allowed ? kAuthorizationResultAllow : kAuthorizationResultDeny);
}

static OSStatus mechanism_deactivate(AuthorizationMechanismRef ref) {
    if (!ref) return errAuthorizationInternal;
    Mechanism *mechanism = ref;
    return mechanism->plugin->callbacks->DidDeactivate(mechanism->engine);
}

static OSStatus mechanism_destroy(AuthorizationMechanismRef ref) {
    free(ref);
    return errAuthorizationSuccess;
}

static const AuthorizationPluginInterface interface = {
    kAuthorizationPluginInterfaceVersion, plugin_destroy, mechanism_create,
    mechanism_invoke, mechanism_deactivate, mechanism_destroy
};

__attribute__((visibility("default")))
OSStatus AuthorizationPluginCreate(const AuthorizationCallbacks *callbacks,
                                  AuthorizationPluginRef *out,
                                  const AuthorizationPluginInterface **out_interface) {
    if (!callbacks || !out || !out_interface || !callbacks->SetResult ||
        !callbacks->GetContextValue || !callbacks->DidDeactivate) return errAuthorizationInternal;
    *out = NULL;
    *out_interface = NULL;
    Plugin *plugin = calloc(1, sizeof(*plugin));
    if (!plugin) return errAuthorizationInternal;
    plugin->callbacks = callbacks;
    *out = plugin;
    *out_interface = &interface;
    return errAuthorizationSuccess;
}
