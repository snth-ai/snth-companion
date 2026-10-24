#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#include "Session.h"
#include <assert.h>
#include <stdio.h>

int main(void) {
    @autoreleasepool {
        NSMutableDictionary *session = [@{ (__bridge NSString *)kCGSessionUserIDKey: @501,
            (__bridge NSString *)kCGSessionOnConsoleKey: @YES,
            (__bridge NSString *)kCGSessionLoginDoneKey: @YES } mutableCopy];
        SNTHLockState (^read)(void) = ^{ return snth_session_decode((__bridge CFDictionaryRef)session, 501); };
        assert(read() == SNTH_UNLOCKED); // actual unlocked Quartz shape
        session[@"CGSSessionScreenIsLocked"] = @YES;
        assert(read() == SNTH_LOCKED);
        session[@"CGSSessionScreenIsLocked"] = @NO;
        assert(read() == SNTH_UNLOCKED);
        session[@"CGSSessionScreenIsLocked"] = @"false";
        assert(read() == SNTH_LOCK_UNKNOWN);
        [session removeObjectForKey:@"CGSSessionScreenIsLocked"];
        session[@"kCGSSessionSecureInputPID"] = @123;
        assert(read() == SNTH_LOCK_UNKNOWN);
        session[@"kCGSSessionSecureInputPID"] = @0;
        assert(read() == SNTH_UNLOCKED);
        session[(__bridge NSString *)kCGSessionOnConsoleKey] = @NO;
        assert(read() == SNTH_LOCK_UNKNOWN);
        session[(__bridge NSString *)kCGSessionOnConsoleKey] = @YES;
        session[(__bridge NSString *)kCGSessionLoginDoneKey] = @NO;
        assert(read() == SNTH_LOCK_UNKNOWN);
        session[(__bridge NSString *)kCGSessionLoginDoneKey] = @YES;
        assert(snth_session_decode((__bridge CFDictionaryRef)session, 502) == SNTH_LOCK_UNKNOWN);
        assert(snth_session_decode((__bridge CFDictionaryRef)session, 0) == SNTH_LOCK_UNKNOWN);
        assert(snth_session_decode(NULL, 501) == SNTH_LOCK_UNKNOWN);
        puts("session state: unlocked omission, explicit lock, wrong user and ambiguous state passed");
    }
}
