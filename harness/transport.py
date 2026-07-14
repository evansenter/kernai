"""Byte transports the framing layer runs over.

`FrameStream` turns any pipe-like file object into a frame source with a
wall-clock watchdog. The watchdog is host-side hygiene only — it stops
`make test` from hanging on a wedged guest. All guest-visible time is
icount-derived and deterministic (P9); nothing in the guest ever depends on
these timeouts.

`Loopback` is the M0 stub: a `cat` subprocess, so frames traverse a real OS
pipe with kernel-determined chunking. It proves the framing + read machinery
end to end before any kernel exists.
"""

from __future__ import annotations  # keeps `bytes | None` legal on py3.9

import os
import selectors
import subprocess
import time
from collections import deque

from .framing import FrameDecoder, encode_frame


class FrameStream:
    """Incrementally read frames from a file object backed by a real fd."""

    def __init__(self, fileobj):
        self._fd = fileobj.fileno()
        os.set_blocking(self._fd, False)
        self._sel = selectors.DefaultSelector()
        self._sel.register(self._fd, selectors.EVENT_READ)
        self.decoder = FrameDecoder()
        self._pending = deque()
        self.eof = False

    def next_frame(self, timeout: float) -> bytes | None:
        """Return the next payload, None on EOF, or raise TimeoutError."""
        deadline = time.monotonic() + timeout
        while True:
            if self._pending:
                return self._pending.popleft()
            if self.eof:
                return None
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                tail = bytes(self.decoder.noise[-200:])
                raise TimeoutError(
                    f"no frame within {timeout}s; trailing noise: {tail!r}"
                )
            if not self._sel.select(remaining):
                continue
            data = os.read(self._fd, 65536)
            if not data:
                self.eof = True
                continue
            self._pending.extend(self.decoder.feed(data))

    def drain(self, timeout: float) -> list[bytes]:
        """Read frames until EOF (returning them) or until the deadline."""
        frames = []
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return frames
            try:
                frame = self.next_frame(remaining)
            except TimeoutError:
                return frames
            if frame is None:
                return frames
            frames.append(frame)

    def close(self):
        self._sel.close()


class Loopback:
    """M0 loopback stub: whatever is sent comes back through a `cat` pipe."""

    def __init__(self):
        self._proc = subprocess.Popen(
            ["cat"], stdin=subprocess.PIPE, stdout=subprocess.PIPE
        )
        self.stream = FrameStream(self._proc.stdout)

    def send(self, payload: bytes):
        self._proc.stdin.write(encode_frame(payload))
        self._proc.stdin.flush()

    def send_raw(self, data: bytes):
        """Inject arbitrary bytes (noise) into the stream."""
        self._proc.stdin.write(data)
        self._proc.stdin.flush()

    def close(self):
        self._proc.stdin.close()
        self._proc.wait(timeout=5)
        self.stream.close()
        self._proc.stdout.close()
