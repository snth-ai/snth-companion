// A manually launched native round-trip validator, not a synth-facing API.
// Each attempt requires fresh LocalAuthentication BEFORE any grant or lock.
// The loginwindow submission uses a non-secret marker, never the owner's password.
#import <AppKit/AppKit.h>
#import <LocalAuthentication/LocalAuthentication.h>
#include <mach/mach_time.h>
#include <xpc/xpc.h>
#include "Session.h"

#define BROKER "ai.snth.connect.locked-use-broker"
#define REQUIREMENT "anchor apple generic and identifier \"" BROKER "\" and certificate leaf[subject.OU] = \"" SNTH_TEAM_ID "\""
#define GUARDIAN @"/Library/Application Support/SNTH Connect/Locked Use/SNTHLockedUseGuardian.app/Contents/MacOS/SNTHLockedUseGuardian"

static uint64_t now_ms(void) {
    mach_timebase_info_data_t tb;
    mach_timebase_info(&tb);
    return (uint64_t)((long double)mach_continuous_time() * tb.numer / tb.denom / 1000000.0L);
}

@interface Validation : NSObject <NSApplicationDelegate> {
    xpc_connection_t broker;
    dispatch_source_t timer;
    LAContext *authentication;
    NSTask *guardian;
    NSString *token;
    uint64_t deadline, lastHeartbeat, nextSubmit, submitDeadline;
    BOOL locking, prepared, preparing, submitted, unlocked, authorized, cancelling, statePending, done;
}
@end

@implementation Validation
- (void)report:(NSString *)step {
    NSData *data = [NSJSONSerialization dataWithJSONObject:@{@"step": step} options:0 error:nil];
    fwrite(data.bytes, 1, data.length, stdout); fputc('\n', stdout); fflush(stdout);
}
- (void)finish:(NSString *)reason success:(BOOL)success {
    if (done) return;
    done = YES;
    [self report:reason];
    // Disconnect revokes any pending lease; guardian and its independent child
    // retain responsibility for re-lock. Never kill a protecting guardian.
    if (broker) xpc_connection_cancel(broker);
    if (timer) dispatch_source_cancel(timer);
    [authentication invalidate];
    exit(success ? 0 : 1);
}
- (void)send:(const char *)operation reply:(void (^)(xpc_object_t))callback {
    xpc_object_t request = xpc_dictionary_create(NULL, NULL, 0);
    xpc_dictionary_set_uint64(request, "version", 1);
    xpc_dictionary_set_string(request, "operation", operation);
    if (token) xpc_dictionary_set_string(request, "token", token.UTF8String);
    xpc_connection_send_message_with_reply(broker, request, dispatch_get_main_queue(), ^(xpc_object_t reply) {
        if (self->done) return;
        if (xpc_get_type(reply) != XPC_TYPE_DICTIONARY || xpc_dictionary_get_uint64(reply, "version") != 1 ||
            !xpc_dictionary_get_bool(reply, "ok")) {
            [self finish:@"broker_refused_or_unavailable; unlock manually if needed" success:NO];
            return;
        }
        callback(reply);
    });
}
- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    (void)notification;
    if (!AXIsProcessTrusted() || !snth_session_can_request_lock() || snth_session_lock_state() != SNTH_UNLOCKED ||
        ![NSFileManager.defaultManager isExecutableFileAtPath:GUARDIAN]) {
        [self finish:@"prerequisites_missing; start from your unlocked console session with Accessibility access" success:NO];
        return;
    }
    broker = xpc_connection_create_mach_service(BROKER ".validation", dispatch_get_main_queue(),
        XPC_CONNECTION_MACH_SERVICE_PRIVILEGED);
    if (!broker || xpc_connection_set_peer_code_signing_requirement(broker, REQUIREMENT)) {
        [self finish:@"broker_identity_unavailable" success:NO]; return;
    }
    xpc_connection_set_event_handler(broker, ^(xpc_object_t event) {
        if (xpc_get_type(event) == XPC_TYPE_ERROR) [self finish:@"broker_disconnected; unlock manually if needed" success:NO];
    });
    xpc_connection_resume(broker);
    deadline = now_ms() + 3000;
    [self send:"status" reply:^(xpc_object_t reply) {
        (void)reply;
        self->deadline = now_ms() + 120000;
        self->authentication = [LAContext new];
        self->authentication.touchIDAuthenticationAllowableReuseDuration = 0;
        [NSApp activateIgnoringOtherApps:YES];
        [self report:@"owner_authentication_required; this test locks, unlocks once, then locks again"];
        [self->authentication evaluatePolicy:LAPolicyDeviceOwnerAuthentication
            localizedReason:@"Проверить Locked use: Mac заблокируется, разблокируется на один тест и снова заблокируется."
            reply:^(BOOL allowed, NSError *error) {
                (void)error;
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (self->done) return;
                    if (!allowed || snth_session_lock_state() != SNTH_UNLOCKED) {
                        [self finish:@"owner_authentication_cancelled_or_session_changed" success:NO]; return;
                    }
                    self->locking = YES;
                    self->deadline = now_ms() + 20000;
                    [self report:@"requesting_system_lock"];
                    snth_session_request_lock();
                });
            }];
    }];
    timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, dispatch_get_main_queue());
    dispatch_source_set_timer(timer, DISPATCH_TIME_NOW, 100 * NSEC_PER_MSEC, 10 * NSEC_PER_MSEC);
    dispatch_source_set_event_handler(timer, ^{ [self tick]; });
    dispatch_resume(timer);
}
- (void)tick {
    uint64_t now = now_ms();
    if (now >= deadline) {
        [self finish:locking ? @"roundtrip_timed_out; unlock manually" : @"owner_authentication_timed_out; no_lock_requested" success:NO];
        return;
    }
    SNTHLockState lock = snth_session_lock_state();
    if (locking && lock == SNTH_LOCKED && !preparing) {
        preparing = YES;
        [self report:@"system_lock_observed"];
        [self send:"prepare-validation" reply:^(xpc_object_t reply) {
            const char *value = xpc_dictionary_get_string(reply, "token");
            if (!value || strlen(value) != 36) { [self finish:@"invalid_grant" success:NO]; return; }
            self->token = [NSString stringWithUTF8String:value];
            self->prepared = YES;
            self->guardian = [NSTask new];
            self->guardian.executableURL = [NSURL fileURLWithPath:GUARDIAN];
            self->guardian.arguments = @[@"--protect"];
            NSPipe *pipe = [NSPipe pipe];
            self->guardian.standardInput = pipe;
            self->guardian.standardOutput = [NSFileHandle fileHandleWithNullDevice];
            NSError *error = nil;
            if (![self->guardian launchAndReturnError:&error]) {
                [self finish:@"guardian_launch_failed" success:NO]; return;
            }
            [pipe.fileHandleForWriting writeData:[self->token dataUsingEncoding:NSUTF8StringEncoding]];
            [pipe.fileHandleForWriting closeFile];
        }];
    }
    if (!prepared) return;
    if (!cancelling && now - lastHeartbeat >= 500) {
        lastHeartbeat = now;
        [self send:"heartbeat" reply:^(xpc_object_t reply) { (void)reply; }];
    }
    if (submitted && lock == SNTH_UNLOCKED) unlocked = YES;
    if (unlocked && authorized && !cancelling) {
        cancelling = YES;
        [self report:@"system_unlock_observed; requesting_relock"];
        [self send:"cancel-task" reply:^(xpc_object_t reply) { (void)reply; }];
    }
    if (!statePending) {
        statePending = YES;
        [self send:"validation-state" reply:^(xpc_object_t reply) {
            self->statePending = NO;
            self->authorized |= xpc_dictionary_get_bool(reply, "consumed");
            if (xpc_dictionary_get_bool(reply, "protected") && !self->submitted && !self->cancelling) {
                uint64_t now = now_ms();
                if (!self->submitDeadline) {
                    self->submitDeadline = now + 2500;
                    self->nextSubmit = now + 300; // loginwindow UI settles after the lock flag
                }
                if (now < self->nextSubmit) return;
                [self report:@"protected_desktop_ready; submitting_system_authorization"];
                SNTHUnlockRequest result = snth_session_request_unlock();
                if (result == SNTH_UNLOCK_SUBMITTED) {
                    self->submitted = YES;
                } else if (result == SNTH_UNLOCK_NO_CONTROL && now_ms() < self->submitDeadline) {
                    self->nextSubmit = now_ms() + 200;
                } else {
                    [self report:[NSString stringWithFormat:@"system_authorization_request_failed_%d", result]];
                    self->cancelling = YES;
                    [self send:"cancel-task" reply:^(xpc_object_t r) { (void)r; }];
                }
            }
            if (xpc_dictionary_get_bool(reply, "revoking")) self->cancelling = YES;
            if (xpc_dictionary_get_bool(reply, "finished") && snth_session_lock_state() == SNTH_LOCKED) {
                BOOL success = self->unlocked && self->authorized;
                [self finish:success ? @"roundtrip_passed; system_relock_observed" : @"roundtrip_failed_closed; system_is_locked"
                    success:success];
            }
        }];
    }
}
@end

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc != 2 || strcmp(argv[1], "--roundtrip") != 0) {
            puts("Usage: SNTHLockedUseValidation --roundtrip\nRequires fresh owner authentication; finishes with the Mac locked.");
            return 2;
        }
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        Validation *delegate = [Validation new];
        NSApp.delegate = delegate;
        [NSApp run];
    }
    return 1;
}
