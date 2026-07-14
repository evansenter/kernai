"""Canonical QEMU invocation. Every boot — interactive `make run`, `make
debug`, and the acceptance checks — goes through this module, so the
determinism flags (P9: -icount, virtual time from instruction count) cannot
drift between invocations.

As a module: `python3 -m harness.qemu [--gdb] [kernel_elf]` execs QEMU with
serial on the caller's stdio.
"""

import json
import os
import pathlib
import subprocess
import sys
import tempfile

from .transport import FrameStream

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
KERNEL_ELF = REPO_ROOT / "kernel/target/riscv64gc-unknown-none-elf/release/kernai"


def qemu_args(kernel_elf=KERNEL_ELF, gdb=False):
    args = [
        "qemu-system-riscv64",
        "-machine", "virt",
        "-cpu", "rv64",
        "-m", "128M",
        "-bios", "default",              # QEMU's bundled OpenSBI, never vendored
        "-display", "none",
        "-monitor", "none",
        "-serial", "stdio",
        "-icount", "shift=1,sleep=off",  # P9: deterministic virtual time
        "-rtc", "clock=vm",              # RTC follows virtual time too
        "-no-reboot",
        "-kernel", str(kernel_elf),
    ]
    if gdb:
        args += ["-s", "-S"]             # gdb stub on :1234, start halted
    return args


class QemuKernel:
    """Boot the kernel for an acceptance check: read events, send bytes."""

    def __init__(self, kernel_elf=KERNEL_ELF):
        self._stderr = tempfile.TemporaryFile()
        self._proc = subprocess.Popen(
            qemu_args(kernel_elf),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self._stderr,
        )
        self.stream = FrameStream(self._proc.stdout)

    def next_event(self, timeout):
        """Next JSON event from the kernel; None on EOF (QEMU exited)."""
        frame = self.stream.next_frame(timeout)
        if frame is None:
            return None
        try:
            return json.loads(frame)
        except ValueError as e:
            raise AssertionError(f"unparseable frame {frame!r}: {e}") from e

    def send(self, data: bytes):
        """Write raw bytes to the guest's serial input."""
        self._proc.stdin.write(data)
        self._proc.stdin.flush()

    def wait_exit(self, timeout):
        """Wait for QEMU to exit on its own (guest SBI shutdown)."""
        return self._proc.wait(timeout)

    def stderr_tail(self):
        self._stderr.seek(0)
        return self._stderr.read()[-2000:].decode(errors="replace")

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        if self._proc.poll() is None:
            self._proc.kill()
        self._proc.wait(timeout=10)
        self.stream.close()
        self._proc.stdout.close()
        self._proc.stdin.close()
        self._stderr.close()


def main(argv):
    gdb = "--gdb" in argv
    positional = [a for a in argv[1:] if not a.startswith("--")]
    elf = pathlib.Path(positional[0]) if positional else KERNEL_ELF
    if not elf.exists():
        print(f"kernel ELF not found: {elf} — run `make build` first", file=sys.stderr)
        return 1
    if gdb:
        print(f"QEMU halted with gdb stub on :1234 — attach with `make gdb`",
              file=sys.stderr)
    os.execvp("qemu-system-riscv64", qemu_args(elf, gdb=gdb))


if __name__ == "__main__":
    sys.exit(main(sys.argv))
