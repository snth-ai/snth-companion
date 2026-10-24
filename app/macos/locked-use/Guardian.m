// Native prototype. Not launched by Connect; production unlock stays disabled.
// All UI, XPC replies and state transitions share the main run loop.
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#include <mach/mach_time.h>
#include <poll.h>
#include <sys/stat.h>
#include <unistd.h>
#include <uuid/uuid.h>
#include <xpc/xpc.h>
#include "GuardianState.h"
#include "Lease.h"
#include "Session.h"
#include "Watchdog.h"

#ifndef SNTH_TEAM_ID
#error "Set SNTH_TEAM_ID to the signing team"
#endif
#define BROKER "ai.snth.connect.locked-use-broker"
#define REQUIREMENT "anchor apple generic and identifier \"" BROKER "\" and certificate leaf[subject.OU] = \"" SNTH_TEAM_ID "\""

static uint64_t now_ms(void) {
    static mach_timebase_info_data_t tb;
    if (!tb.denom) mach_timebase_info(&tb);
    return (uint64_t)((long double)mach_continuous_time() * tb.numer / tb.denom / 1000000.0L);
}

@interface SNTHCover : NSPanel
@end
@implementation SNTHCover
- (BOOL)canBecomeKeyWindow { return NO; }
- (BOOL)canBecomeMainWindow { return NO; }
@end

@interface SNTHGuardian : NSObject <NSApplicationDelegate> {
    SNTHGuardianState state;
    CFMachPortRef tap;
    CFRunLoopSourceRef tapSource;
    xpc_connection_t broker;
    dispatch_source_t timer;
    NSMutableArray<SNTHCover *> *covers;
    NSString *token;
    SNTHWatchdog watchdog;
    BOOL enrolled, enrollmentSettled, cancelled, localReported, requestPending, heartbeatPending, finished;
    uint64_t lastHeartbeat, lastLockRequest;
}
- (instancetype)initWithToken:(NSString *)value;
- (void)revoke:(BOOL)local;
- (void)tick;
- (void)displayChanged:(NSNotification *)notification;
@end

static CGEventRef input_event(CGEventTapProxy proxy, CGEventType type, CGEventRef event, void *context) {
    (void)proxy;
    SNTHGuardian *guardian = (__bridge SNTHGuardian *)context;
    BOOL disabled = type == kCGEventTapDisabledByTimeout || type == kCGEventTapDisabledByUserInput;
    [guardian revoke:!disabled];
    // No PID/tag exemption: CGEvent fields are not an authenticated CUA route.
    // This deliberately also stops synthetic input until driver integration.
    return disabled ? event : NULL;
}

@implementation SNTHGuardian
- (instancetype)initWithToken:(NSString *)value {
    if ((self = [super init])) { token = value; covers = [NSMutableArray new]; watchdog = (SNTHWatchdog){ .pipe = -1, .pid = -1 }; }
    return self;
}

- (void)send:(const char *)operation completion:(void (^)(BOOL, BOOL))completion {
    xpc_object_t request = xpc_dictionary_create(NULL, NULL, 0);
    xpc_dictionary_set_uint64(request, "version", 1);
    xpc_dictionary_set_string(request, "operation", operation);
    xpc_dictionary_set_string(request, "token", token.UTF8String);
    xpc_connection_send_message_with_reply(broker, request, dispatch_get_main_queue(), ^(xpc_object_t reply) {
        if (self->finished) return;
        BOOL valid = xpc_get_type(reply) == XPC_TYPE_DICTIONARY &&
            xpc_dictionary_get_uint64(reply, "version") == 1;
        completion(valid && xpc_dictionary_get_bool(reply, "ok"), valid);
    });
}

- (void)coverDisplays {
    // Add new covers before closing old ones. Display reconfiguration still
    // revokes: AppKit notifications cannot promise atomic hot-plug coverage.
    NSMutableArray<SNTHCover *> *next = [NSMutableArray new];
    for (NSScreen *screen in NSScreen.screens) {
        SNTHCover *window = [[SNTHCover alloc] initWithContentRect:screen.frame
            styleMask:NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel
            backing:NSBackingStoreBuffered defer:NO];
        window.level = CGShieldingWindowLevel() + 1;
        window.backgroundColor = NSColor.blackColor;
        window.opaque = YES;
        window.hasShadow = NO;
        window.hidesOnDeactivate = NO;
        window.ignoresMouseEvents = YES;
        window.releasedWhenClosed = NO;
        window.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces |
            NSWindowCollectionBehaviorStationary | NSWindowCollectionBehaviorFullScreenAuxiliary |
            NSWindowCollectionBehaviorIgnoresCycle;
        // Advisory only. ScreenCaptureKit needs explicit exclusion in CUA.
        window.sharingType = NSWindowSharingNone;
        NSTextField *label = [NSTextField labelWithString:@"SNTH Connect\nЗащищённая сессия"];
        label.textColor = NSColor.whiteColor;
        label.alignment = NSTextAlignmentCenter;
        label.font = [NSFont systemFontOfSize:24];
        label.frame = NSMakeRect(0, NSHeight(screen.frame) / 2 - 40, NSWidth(screen.frame), 80);
        [window.contentView addSubview:label];
        [window orderFrontRegardless];
        [next addObject:window];
    }
    for (SNTHCover *window in covers) [window close];
    covers = next;
}

- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    (void)notification;
    if (!snth_session_can_request_lock() || getuid() < 501 || !AXIsProcessTrusted() || !CGPreflightListenEventAccess() ||
        snth_session_lock_state() != SNTH_LOCKED) {
        fputs("Guardian prerequisites unavailable; no desktop protection started.\n", stderr);
        [NSApp terminate:nil];
        return;
    }
    broker = xpc_connection_create_mach_service(BROKER ".guardian", dispatch_get_main_queue(),
        XPC_CONNECTION_MACH_SERVICE_PRIVILEGED);
    if (!broker || xpc_connection_set_peer_code_signing_requirement(broker, REQUIREMENT) != 0) {
        state = (SNTHGuardianState){0};
        [NSApp terminate:nil];
        return;
    }
    xpc_connection_set_event_handler(broker, ^(xpc_object_t event) {
        if (xpc_get_type(event) == XPC_TYPE_ERROR) [self revoke:NO];
        else if (xpc_get_type(event) == XPC_TYPE_DICTIONARY) {
            const char *kind = xpc_dictionary_get_string(event, "event");
            const char *value = xpc_dictionary_get_string(event, "token");
            if (xpc_dictionary_get_uint64(event, "version") == 1 && kind && value &&
                strcmp(kind, "relock-required") == 0 && strcmp(value, self->token.UTF8String) == 0)
                [self revoke:NO];
        }
    });
    xpc_connection_resume(broker);
    // Authenticate an available broker before covering anything. A missing or
    // unsigned service must never strand the owner behind prototype windows.
    [self send:"status" completion:^(BOOL ok, BOOL valid) {
        (void)valid;
        if (!ok || !snth_guardian_start(&self->state, snth_session_lock_state(), now_ms())) {
            xpc_connection_cancel(self->broker);
            [NSApp terminate:nil];
            return;
        }
        [self arm];
    }];
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        if (!self->state.started) { xpc_connection_cancel(self->broker); [NSApp terminate:nil]; }
    });
}

- (void)arm {
    if (!snth_watchdog_start(&watchdog)) {
        state = (SNTHGuardianState){0};
        xpc_connection_cancel(broker);
        [NSApp terminate:nil];
        return;
    }
    tap = CGEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap, kCGEventTapOptionDefault,
        kCGEventMaskForAllEvents, input_event, (__bridge void *)self);
    if (!tap) {
        snth_watchdog_finish(&watchdog);
        state = (SNTHGuardianState){0};
        xpc_connection_cancel(broker);
        [NSApp terminate:nil];
        return;
    }
    tapSource = CFMachPortCreateRunLoopSource(NULL, tap, 0);
    if (!tapSource) {
        snth_watchdog_finish(&watchdog);
        CFRelease(tap); tap = NULL;
        state = (SNTHGuardianState){0};
        xpc_connection_cancel(broker);
        [NSApp terminate:nil];
        return;
    }
    CFRunLoopAddSource(CFRunLoopGetMain(), tapSource, kCFRunLoopCommonModes);
    [self coverDisplays];
    [NSNotificationCenter.defaultCenter addObserver:self selector:@selector(displayChanged:)
        name:NSApplicationDidChangeScreenParametersNotification object:nil];
    [NSWorkspace.sharedWorkspace.notificationCenter addObserver:self selector:@selector(displayChanged:)
        name:NSWorkspaceActiveSpaceDidChangeNotification object:nil];
    [self send:"desktop-protected" completion:^(BOOL ok, BOOL valid) {
        self->enrollmentSettled = YES;
        self->enrolled = ok;
        snth_guardian_reply(&self->state, ok, now_ms());
        if (!ok) {
            [self revoke:NO];
            // Only an explicit denial proves absence of enrollment.
            self->state.drained = valid;
        }
    }];
    timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
    dispatch_source_set_timer(timer, DISPATCH_TIME_NOW, 100 * NSEC_PER_MSEC, 10 * NSEC_PER_MSEC);
    dispatch_source_set_event_handler(timer, ^{ [self tick]; });
    dispatch_resume(timer);
}

- (void)displayChanged:(NSNotification *)notification {
    (void)notification;
    if (finished) return;
    [self revoke:NO];
    [self coverDisplays];
}

- (void)revoke:(BOOL)local {
    snth_guardian_revoke(&state, local);
}

- (void)tick {
    uint64_t now = now_ms();
    SNTHLockState lock = snth_session_lock_state();
    BOOL healthy = snth_watchdog_pulse(&watchdog) && covers.count > 0 && covers.count == NSScreen.screens.count && tap &&
        CGEventTapIsEnabled(tap) && AXIsProcessTrusted() && CGPreflightListenEventAccess();
    for (NSWindow *window in covers) healthy &= window.visible;
    snth_guardian_tick(&state, lock, healthy, now);
    if (state.revoking) {
        if (tap && !CGEventTapIsEnabled(tap)) CGEventTapEnable(tap, true);
        if (lock != SNTH_LOCKED && now - lastLockRequest >= 500) {
            lastLockRequest = now;
            (void)snth_session_request_lock(); // return value does not prove the session locked
        }
        if (enrollmentSettled && enrolled && !requestPending) {
            if (!cancelled || (state.local_input && !localReported)) {
                requestPending = YES;
                BOOL reportLocal = state.local_input;
                [self send:reportLocal ? "local-input" : "cancel-task" completion:^(BOOL ok, BOOL valid) {
                    (void)valid;
                    self->requestPending = NO;
                    if (ok) { self->cancelled = YES; self->localReported |= reportLocal; }
                }];
            } else if (lock == SNTH_LOCKED && !state.drained) {
                requestPending = YES;
                [self send:"screen-locked" completion:^(BOOL ok, BOOL valid) {
                    (void)valid;
                    self->requestPending = NO;
                    self->state.drained = ok;
                }];
            }
        }
        if (snth_guardian_can_uncover(&state, lock)) [self finish];
    } else if (enrolled && !heartbeatPending && now - lastHeartbeat >= 500) {
        lastHeartbeat = now;
        heartbeatPending = YES;
        [self send:"heartbeat" completion:^(BOOL ok, BOOL valid) {
            (void)valid;
            self->heartbeatPending = NO;
            snth_guardian_reply(&self->state, ok, now_ms());
        }];
    }
}

- (void)finish {
    finished = YES;
    snth_watchdog_finish(&watchdog);
    dispatch_source_cancel(timer);
    [NSNotificationCenter.defaultCenter removeObserver:self];
    [NSWorkspace.sharedWorkspace.notificationCenter removeObserver:self];
    if (tapSource) { CFRunLoopRemoveSource(CFRunLoopGetMain(), tapSource, kCFRunLoopCommonModes); CFRelease(tapSource); }
    if (tap) { CFMachPortInvalidate(tap); CFRelease(tap); }
    for (NSWindow *window in covers) [window close];
    xpc_connection_cancel(broker);
    [NSApp terminate:nil];
}

- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)sender {
    (void)sender;
    if (!state.started || finished) return NSTerminateNow;
    [self revoke:NO];
    return NSTerminateCancel;
}
@end

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc == 2 && strcmp(argv[1], "--watchdog") == 0) return snth_watchdog_main();
        if (argc == 2 && strcmp(argv[1], "--check") == 0) {
            SNTHLockState lock = snth_session_lock_state();
            NSDictionary *status = @{ @"unlock_ready": @NO,
                @"lock_state": lock == SNTH_LOCKED ? @"locked" : lock == SNTH_UNLOCKED ? @"unlocked" : @"unknown",
                @"accessibility": @(AXIsProcessTrusted()), @"input_monitoring": @(CGPreflightListenEventAccess()),
                @"relock_symbol_available": @(snth_session_can_request_lock()), @"relock_verified": @NO,
                @"capture_exclusion_integrated": @NO, @"independent_watchdog_integrated": @YES };
            NSData *data = [NSJSONSerialization dataWithJSONObject:status options:0 error:nil];
            fwrite(data.bytes, 1, data.length, stdout); fputc('\n', stdout);
            return 0;
        }
        // Token handoff is via an inherited pipe, never argv/env or a disk file.
        struct stat input;
        if (argc != 2 || strcmp(argv[1], "--protect") != 0 || fstat(STDIN_FILENO, &input) || !S_ISFIFO(input.st_mode)) {
            fputs("Usage: SNTHLockedUseGuardian --check (read-only)\nProtection requires an app-owned token pipe.\n", stderr);
            return 2;
        }
        char bytes[SNTH_LEASE_TOKEN_SIZE] = {0};
        size_t used = 0;
        uint64_t deadline = now_ms() + 1000;
        while (used < sizeof(bytes) - 1 && now_ms() < deadline) {
            struct pollfd fd = { .fd = STDIN_FILENO, .events = POLLIN };
            if (poll(&fd, 1, 50) <= 0) continue;
            ssize_t count = read(STDIN_FILENO, bytes + used, sizeof(bytes) - 1 - used);
            if (count <= 0) break;
            used += (size_t)count;
        }
        close(STDIN_FILENO);
        uuid_t parsed;
        if (used != sizeof(bytes) - 1 || uuid_parse(bytes, parsed)) return 2;
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
        SNTHGuardian *guardian = [[SNTHGuardian alloc] initWithToken:[NSString stringWithUTF8String:bytes]];
        NSApp.delegate = guardian;
        [NSApp run];
    }
    return 0;
}
