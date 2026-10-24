#pragma once
#include <stdbool.h>
#include <sys/types.h>

typedef struct { int pipe; pid_t pid; } SNTHWatchdog;
// Separate process in the same GUI/audit session; the guardian UI thread cannot
// keep it alive accidentally while hung. Hard deadlines never renew.
bool snth_watchdog_start(SNTHWatchdog *watchdog);
bool snth_watchdog_pulse(SNTHWatchdog *watchdog);
void snth_watchdog_finish(SNTHWatchdog *watchdog);
int snth_watchdog_main(void);
