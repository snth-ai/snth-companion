#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <Security/Security.h>
#include <dlfcn.h>
#include <unistd.h>
#include "Session.h"

SNTHLockState snth_session_decode(CFDictionaryRef session, uid_t expected_uid) {
    if (!session || expected_uid < 501 || CFGetTypeID(session) != CFDictionaryGetTypeID()) return SNTH_LOCK_UNKNOWN;
    CFTypeRef uid = CFDictionaryGetValue(session, kCGSessionUserIDKey);
    int64_t value = -1;
    if (!uid || CFGetTypeID(uid) != CFNumberGetTypeID() ||
        !CFNumberGetValue(uid, kCFNumberSInt64Type, &value) || value != expected_uid) return SNTH_LOCK_UNKNOWN;
    CFTypeRef console = CFDictionaryGetValue(session, kCGSessionOnConsoleKey);
    CFTypeRef login = CFDictionaryGetValue(session, kCGSessionLoginDoneKey);
    if (!console || CFGetTypeID(console) != CFBooleanGetTypeID() || !CFBooleanGetValue(console) ||
        !login || CFGetTypeID(login) != CFBooleanGetTypeID() || !CFBooleanGetValue(login))
        return SNTH_LOCK_UNKNOWN;
    CFTypeRef locked = CFDictionaryGetValue(session, CFSTR("CGSSessionScreenIsLocked"));
    if (locked) {
        if (CFGetTypeID(locked) != CFBooleanGetTypeID()) return SNTH_LOCK_UNKNOWN;
        return CFBooleanGetValue(locked) ? SNTH_LOCKED : SNTH_UNLOCKED;
    }
    // Quartz omits the lock flag on an unlocked console session. Do not apply
    // this convention to missing dictionaries, other users or login sessions.
    // Secure input with no explicit lock flag is ambiguous, so decline it.
    CFTypeRef secure = CFDictionaryGetValue(session, CFSTR("kCGSSessionSecureInputPID"));
    if (secure) {
        int64_t pid = -1;
        if (CFGetTypeID(secure) != CFNumberGetTypeID() ||
            !CFNumberGetValue(secure, kCFNumberSInt64Type, &pid) || pid != 0) return SNTH_LOCK_UNKNOWN;
    }
    return SNTH_UNLOCKED;
}

SNTHLockState snth_session_lock_state(void) {
    CFDictionaryRef session = CGSessionCopyCurrentDictionary();
    SNTHLockState state = snth_session_decode(session, getuid());
    if (session) CFRelease(session);
    return state;
}

typedef int (*LockFn)(void);
static LockFn lock_function(void) {
    static LockFn fn;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        void *lib = dlopen("/System/Library/PrivateFrameworks/login.framework/login", RTLD_NOW | RTLD_LOCAL);
        if (lib) fn = (LockFn)dlsym(lib, "SACLockScreenImmediate");
    });
    return fn;
}
bool snth_session_can_request_lock(void) { return lock_function() != NULL; }
bool snth_session_request_lock(void) {
    LockFn fn = lock_function();
    if (!fn) return false;
    (void)fn();
    return true;
}

static bool apple_loginwindow(pid_t pid) {
    SecCodeRef code = NULL;
    SecRequirementRef requirement = NULL;
    NSDictionary *attributes = @{(__bridge NSString *)kSecGuestAttributePid: @(pid)};
    bool ok = SecCodeCopyGuestWithAttributes(NULL, (__bridge CFDictionaryRef)attributes,
        kSecCSDefaultFlags, &code) == errSecSuccess &&
        SecRequirementCreateWithString(CFSTR("anchor apple and identifier \"com.apple.loginwindow\""),
            kSecCSDefaultFlags, &requirement) == errSecSuccess &&
        SecCodeCheckValidity(code, kSecCSDefaultFlags, requirement) == errSecSuccess;
    if (requirement) CFRelease(requirement);
    if (code) CFRelease(code);
    return ok;
}

static id attribute(AXUIElementRef element, CFStringRef name) {
    CFTypeRef value = NULL;
    if (AXUIElementCopyAttributeValue(element, name, &value) != kAXErrorSuccess) return nil;
    return CFBridgingRelease(value);
}

SNTHUnlockRequest snth_session_request_unlock(void) {
    // Internal call for the coordinator AFTER a broker-protected grant exists.
    // Never expose it as a synth tool. It never reads the credential value.
    if (snth_session_lock_state() != SNTH_LOCKED) return SNTH_UNLOCK_NOT_LOCKED;
    if (!AXIsProcessTrusted()) return SNTH_UNLOCK_NO_ACCESS;
    pid_t pid = 0;
    for (NSRunningApplication *app in [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.apple.loginwindow"]) {
        if (apple_loginwindow(app.processIdentifier)) { pid = app.processIdentifier; break; }
    }
    if (!pid) return SNTH_UNLOCK_NO_LOGINWINDOW;
    AXUIElementRef app = AXUIElementCreateApplication(pid);
    AXUIElementSetMessagingTimeout(app, 0.1f);
    id windows = attribute(app, kAXWindowsAttribute);
    NSMutableArray *pending = [NSMutableArray arrayWithObject:CFBridgingRelease(app)];
    if ([windows isKindOfClass:NSArray.class]) [pending addObjectsFromArray:windows];
    CFAbsoluteTime deadline = CFAbsoluteTimeGetCurrent() + 1.0;
    NSUInteger count = 0;
    while (pending.count && count++ < 256 && CFAbsoluteTimeGetCurrent() < deadline) {
        id object = pending.lastObject;
        [pending removeLastObject];
        AXUIElementRef element = (__bridge AXUIElementRef)object;
        if (CFGetTypeID(element) != AXUIElementGetTypeID()) continue;
        NSString *identifier = attribute(element, kAXIdentifierAttribute);
        NSString *subrole = attribute(element, kAXSubroleAttribute);
        if ([identifier isEqual:@"UserPasswordTextField"] && [subrole isEqual:(__bridge NSString *)kAXSecureTextFieldSubrole]) {
            CFArrayRef actions = NULL;
            AXError error = AXUIElementCopyActionNames(element, &actions);
            bool confirm = error == kAXErrorSuccess && actions &&
                CFArrayContainsValue(actions, CFRangeMake(0, CFArrayGetCount(actions)), kAXConfirmAction);
            if (actions) CFRelease(actions);
            if (!confirm) {
                fputs("SNTH validation: system password control has no AXConfirm action\n", stderr);
                return SNTH_UNLOCK_NO_CONTROL;
            }
            // loginwindow ignores confirmation of an empty field. A unique,
            // non-secret marker starts its authorization machinery; it is not
            // the owner's password. If our plug-in declines, the normal system
            // branch may reject this marker as an incorrect credential.
            // Our single-use plug-in grant must authorize this attempt. Submit
            // exactly once: a rejected attempt must never become password retries.
            if (snth_session_lock_state() != SNTH_LOCKED || !apple_loginwindow(pid)) return SNTH_UNLOCK_NOT_LOCKED;
            NSString *marker = [@"SNTH-validation-" stringByAppendingString:NSUUID.UUID.UUIDString];
            if (AXUIElementSetAttributeValue(element, kAXValueAttribute, (__bridge CFStringRef)marker) != kAXErrorSuccess)
                return SNTH_UNLOCK_ACTION_FAILED;
            return AXUIElementPerformAction(element, kAXConfirmAction) == kAXErrorSuccess
                ? SNTH_UNLOCK_SUBMITTED : SNTH_UNLOCK_ACTION_FAILED;
        }
        id children = attribute(element, kAXChildrenAttribute);
        if ([children isKindOfClass:NSArray.class]) {
            NSUInteger remaining = 256 - MIN((NSUInteger)256, count + pending.count);
            [pending addObjectsFromArray:[children subarrayWithRange:NSMakeRange(0, MIN([children count], remaining))]];
        }
    }
    fprintf(stderr, "SNTH validation: no ready system password control in %lu AX nodes\n", (unsigned long)count);
    return SNTH_UNLOCK_NO_CONTROL;
}
