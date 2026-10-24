// Process-level watchdog fixture. Never links Session.m or calls an OS lock API.
#include "Session.h"
#include "Watchdog.h"
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static unsigned reads;
static bool requested;
SNTHLockState snth_session_lock_state(void) {
    if (reads++ == 0 || requested || getenv("SNTH_TEST_ALREADY_LOCKED")) return SNTH_LOCKED;
    return SNTH_UNLOCKED;
}
bool snth_session_can_request_lock(void) { return true; }
bool snth_session_request_lock(void) {
    requested = true;
    return write(STDERR_FILENO, "LOCK\n", 5) == 5;
}
int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--watchdog") == 0) return snth_watchdog_main();
    // Exercise the real posix_spawn/pipe handoff and graceful completion too.
    setenv("SNTH_TEST_ALREADY_LOCKED", "1", 1);
    SNTHWatchdog watchdog;
    if (!snth_watchdog_start(&watchdog)) return 1;
    if (!snth_watchdog_pulse(&watchdog)) return 1;
    snth_watchdog_finish(&watchdog);
    return 0;
}
