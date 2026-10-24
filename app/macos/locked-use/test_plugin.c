// Exercises the actual plug-in ABI in-process; no authdb changes or login.
#include <Security/AuthorizationPlugin.h>
#include <assert.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static AuthorizationResult result;
static int results;
static int deactivations;
static uid_t target;
static bool missing_context;

static OSStatus set_result(AuthorizationEngineRef engine, AuthorizationResult value) {
    (void)engine; result = value; results++; return errAuthorizationSuccess;
}
static OSStatus deactivate(AuthorizationEngineRef engine) {
    (void)engine; deactivations++; return errAuthorizationSuccess;
}
static OSStatus context(AuthorizationEngineRef engine, AuthorizationString key,
                        AuthorizationContextFlags *flags, const AuthorizationValue **out) {
    (void)engine;
    assert(strcmp(key, "uid") == 0);
    static AuthorizationValue value;
    *flags = 0;
    value.length = sizeof(target); value.data = &target;
    *out = missing_context ? NULL : &value;
    return errAuthorizationSuccess;
}

int main(void) {
    AuthorizationCallbacks callbacks = { .version = kAuthorizationCallbacksVersion,
        .SetResult = set_result, .DidDeactivate = deactivate, .GetContextValue = context };
    AuthorizationPluginRef plugin = NULL;
    const AuthorizationPluginInterface *api = NULL;
    AuthorizationCallbacks invalid = {0};
    assert(AuthorizationPluginCreate(&invalid, &plugin, &api) != errAuthorizationSuccess);
    assert(AuthorizationPluginCreate(&callbacks, &plugin, &api) == errAuthorizationSuccess);
    AuthorizationEngineRef engine = (AuthorizationEngineRef)&callbacks;
    AuthorizationMechanismRef mechanism = NULL;
    assert(api->MechanismCreate(plugin, engine, "other", &mechanism) != errAuthorizationSuccess);
    assert(mechanism == NULL);
    assert(api->MechanismCreate(plugin, engine, "allow", &mechanism) == errAuthorizationSuccess);
    for (int scenario = 0; scenario < 3; scenario++) {
        target = scenario == 0 ? 0 : getuid();
        missing_context = scenario == 2;
        assert(api->MechanismInvoke(mechanism) == errAuthorizationSuccess);
        assert(result == kAuthorizationResultDeny);
    }
    assert(results == 3);
    assert(api->MechanismDeactivate(mechanism) == errAuthorizationSuccess);
    assert(deactivations == 1);
    assert(api->MechanismDestroy(mechanism) == errAuthorizationSuccess);
    assert(api->PluginDestroy(plugin) == errAuthorizationSuccess);
    puts("native authorization ABI: root, missing context, and unavailable broker all denied");
}
