#!/usr/bin/env python3
"""Serialize Console dependency replacement, builds and artifact consumers."""
import errno
import os
from pathlib import Path
import subprocess
import sys
import time


LOCK_PATH = Path(__file__).resolve().parents[1] / "target" / "console-build.lock"
LOCK_ENV = "RUA_CONSOLE_BUILD_LOCK"


def main():
    command = sys.argv[1:]
    if not command:
        raise SystemExit("Usage: with-console-build-lock.py COMMAND [ARGS...]")
    # Nested packaging steps run while the outer wrapper still owns the lock.
    if os.environ.get(LOCK_ENV) == str(LOCK_PATH):
        return subprocess.call(command)
    LOCK_PATH.parent.mkdir(parents=True, exist_ok=True)
    with LOCK_PATH.open("a+b") as lock:
        if os.name == "nt" and lock.tell() == 0:
            lock.write(b"0")
            lock.flush()
        waiting = False
        while True:
            try:
                if os.name == "nt":
                    import msvcrt
                    lock.seek(0)
                    msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except OSError as error:
                if error.errno not in (errno.EACCES, errno.EAGAIN):
                    raise
                if not waiting:
                    print("[console-build] waiting for another build to release Console dependencies and artifacts ...", flush=True)
                    waiting = True
                time.sleep(0.1)
        env = {**os.environ, LOCK_ENV: str(LOCK_PATH)}
        # On Unix the child retains the lock even if this wrapper is terminated.
        options = {} if os.name == "nt" else {"pass_fds": (lock.fileno(),)}
        return subprocess.call(command, env=env, **options)


if __name__ == "__main__":
    raise SystemExit(main())
