#include "Watchdog.h"
#include "Session.h"
#include "Lease.h"
#include <errno.h>
#include <fcntl.h>
#include <mach/mach_time.h>
#include <mach-o/dyld.h>
#include <poll.h>
#include <signal.h>
#include <spawn.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include <limits.h>

extern char **environ;
typedef struct { uint64_t sent; uint64_t kind; } Pulse;
static uint64_t now_ms(void) {
    mach_timebase_info_data_t tb;
    mach_timebase_info(&tb);
    return (uint64_t)((long double)mach_continuous_time() * tb.numer / tb.denom / 1000000.0L);
}

bool snth_watchdog_start(SNTHWatchdog *watchdog) {
    *watchdog = (SNTHWatchdog){ .pipe = -1, .pid = -1 };
    char executable[PATH_MAX];
    uint32_t length = sizeof(executable);
    if (_NSGetExecutablePath(executable, &length)) return false;
    int input[2], ready[2];
    if (pipe(input)) return false;
    if (pipe(ready)) { close(input[0]); close(input[1]); return false; }
    for (int i = 0; i < 2; ++i) { fcntl(input[i], F_SETFD, FD_CLOEXEC); fcntl(ready[i], F_SETFD, FD_CLOEXEC); }
    posix_spawn_file_actions_t actions;
    posix_spawnattr_t attributes;
    posix_spawn_file_actions_init(&actions);
    posix_spawnattr_init(&attributes);
    posix_spawnattr_setflags(&attributes, POSIX_SPAWN_CLOEXEC_DEFAULT);
    posix_spawn_file_actions_adddup2(&actions, input[0], STDIN_FILENO);
    posix_spawn_file_actions_adddup2(&actions, ready[1], STDOUT_FILENO);
    posix_spawn_file_actions_addinherit_np(&actions, STDERR_FILENO);
    char *argv[] = { executable, "--watchdog", NULL };
    pid_t pid = -1;
    int error = posix_spawn(&pid, executable, &actions, &attributes, argv, environ);
    posix_spawn_file_actions_destroy(&actions);
    posix_spawnattr_destroy(&attributes);
    close(input[0]); close(ready[1]);
    struct pollfd fd = { .fd = ready[0], .events = POLLIN };
    char result = 0;
    bool ok = !error && poll(&fd, 1, 1000) == 1 && read(ready[0], &result, 1) == 1 && result == 'R';
    close(ready[0]);
    if (!ok) {
        close(input[1]);
        if (pid > 0) { kill(pid, SIGTERM); waitpid(pid, NULL, 0); }
        return false; // no cover or grant was issued yet
    }
    fcntl(input[1], F_SETFL, O_NONBLOCK);
    // Pipe writes must return EPIPE, never terminate the guardian.
    signal(SIGPIPE, SIG_IGN);
    *watchdog = (SNTHWatchdog){ .pipe = input[1], .pid = pid };
    return snth_watchdog_pulse(watchdog);
}

bool snth_watchdog_pulse(SNTHWatchdog *watchdog) {
    Pulse pulse = { .sent = now_ms(), .kind = 'H' };
    if (watchdog->pipe < 0 || watchdog->pid <= 0 || waitpid(watchdog->pid, NULL, WNOHANG) != 0) return false;
    return write(watchdog->pipe, &pulse, sizeof(pulse)) == sizeof(pulse);
}

void snth_watchdog_finish(SNTHWatchdog *watchdog) {
    // Call only after the broker drained the grant AND Quartz confirmed lock.
    // Child independently checks the lock before accepting this stop request.
    if (watchdog->pipe >= 0) {
        Pulse pulse = { .sent = now_ms(), .kind = 'Q' };
        (void)write(watchdog->pipe, &pulse, sizeof(pulse));
        close(watchdog->pipe);
        watchdog->pipe = -1;
    }
}

int snth_watchdog_main(void) {
    struct stat input;
    if (getuid() < 501 || fstat(STDIN_FILENO, &input) || !S_ISFIFO(input.st_mode) ||
        !snth_session_can_request_lock() || snth_session_lock_state() != SNTH_LOCKED) return 2;
    uint64_t started = now_ms(), seen = started;
    const uint64_t deadline = started + SNTH_LEASE_MAX_MS;
    bool revoking = false;
    uint64_t last_lock = 0;
    if (write(STDOUT_FILENO, "R", 1) != 1) return 2;
    close(STDOUT_FILENO);
    while (true) {
        struct pollfd fd = { .fd = STDIN_FILENO, .events = POLLIN };
        int ready = poll(&fd, 1, 100);
        uint64_t now = now_ms();
        if (ready < 0 && errno != EINTR) revoking = true;
        if (ready > 0) {
            Pulse pulse = {0};
            ssize_t size = read(STDIN_FILENO, &pulse, sizeof(pulse));
            if (size != sizeof(pulse) || pulse.sent > now || now - pulse.sent >= 1000 || pulse.sent < seen) {
                revoking = true;
            } else if (pulse.kind == 'Q') {
                if (snth_session_lock_state() == SNTH_LOCKED) return 0;
                revoking = true;
            } else if (pulse.kind == 'H' && !revoking) seen = pulse.sent;
            else revoking = true;
        }
        if (now < seen || now - seen >= 1000 || now >= deadline) revoking = true;
        if (revoking) {
            SNTHLockState lock = snth_session_lock_state();
            if (lock != SNTH_LOCKED && now - last_lock >= 250) {
                last_lock = now;
                snth_session_request_lock();
            }
            // Keep observing beyond the broker's maximum grant lifetime, so a
            // late in-flight authorization cannot leave the desktop unlocked.
            if (now >= deadline + SNTH_LEASE_HEARTBEAT_MS && lock == SNTH_LOCKED) return 0;
            // EOF is permanently readable: avoid spinning after parent death.
            if (ready > 0 && (fd.revents & POLLHUP)) poll(NULL, 0, 100);
        }
    }
}
