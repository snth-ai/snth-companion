"""Exercise real child-process EOF and stalled-parent paths with fake OS calls."""
import ctypes
import os
import selectors
import struct
import subprocess
import sys
import time

binary = sys.argv[1]
lib = ctypes.CDLL('/usr/lib/libSystem.B.dylib')
lib.mach_continuous_time.restype = ctypes.c_uint64


class Timebase(ctypes.Structure):
    _fields_ = [('numer', ctypes.c_uint32), ('denom', ctypes.c_uint32)]


tb = Timebase()
lib.mach_timebase_info(ctypes.byref(tb))


def pulse(kind, age=0):
    now = lib.mach_continuous_time() * tb.numer // tb.denom // 1_000_000
    return struct.pack('=QQ', now - age, ord(kind))


def run_case(name, payload=None, eof=False, already_locked=False):
    env = os.environ.copy()
    env.pop('SNTH_TEST_ALREADY_LOCKED', None)
    if already_locked:
        env['SNTH_TEST_ALREADY_LOCKED'] = '1'
    process = subprocess.Popen([binary, '--watchdog'], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            assert selector.select(2), f'{name}: watchdog startup timed out'
            assert process.stdout.read(1) == b'R', f'{name}: startup failed'
        started = time.monotonic()
        if callable(payload):
            payload = payload()
        if payload:
            process.stdin.write(payload)
            process.stdin.flush()
        if eof:
            process.stdin.close()
        if already_locked:
            assert process.wait(timeout=2) == 0, name
            assert process.stderr.read() == b'', name
        else:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stderr, selectors.EVENT_READ)
                assert selector.select(2), f'{name}: no re-lock requested'
                assert process.stderr.readline() == b'LOCK\n', name
            assert time.monotonic() - started < 1.8, name
        print(f'watchdog: {name} passed')
    finally:
        if process.poll() is None:
            process.terminate()
        process.wait(timeout=2)
        for stream in (process.stdin, process.stdout, process.stderr):
            stream.close()


subprocess.run([binary], check=True, timeout=3)
run_case('parent crash / EOF', eof=True)
run_case('stalled parent')
run_case('stale heartbeat', lambda: pulse('H', 1500))
run_case('malformed input', b'x')
run_case('stop before OS lock', lambda: pulse('Q'))
run_case('verified graceful stop', lambda: pulse('Q'), already_locked=True)
