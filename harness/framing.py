"""Length-prefixed framing over a byte stream. Deliberately dumb (CLAUDE.md).

Wire format, little-endian:

    +------+------+----------------+---------...---+
    | 0xAA | 0x99 |  len: u32 LE   | payload (len) |
    +------+------+----------------+---------...---+

The two magic bytes exist for exactly one reason: the kernel shares its UART
with OpenSBI's ASCII boot banner, so a reader must be able to find the first
frame in a stream that starts with noise. 0xAA 0x99 cannot occur in ASCII
text. There is no checksum: QEMU's virtual serial is lossless, and any
cleverness here contaminates downstream measurements (CLAUDE.md).

Payloads are UTF-8 JSON event objects, but this module treats them as opaque
bytes; JSON is the layer above.
"""

MAGIC = b"\xaa\x99"
LEN_BYTES = 4
MAX_FRAME_LEN = 1 << 20  # 1 MiB sanity bound; a bigger "length" means desync


def encode_frame(payload: bytes) -> bytes:
    """Encode one payload as a wire frame."""
    if len(payload) > MAX_FRAME_LEN:
        raise ValueError(f"payload too large: {len(payload)}")
    return MAGIC + len(payload).to_bytes(LEN_BYTES, "little") + payload


class FrameDecoder:
    """Incremental decoder: feed() arbitrary chunks, get complete payloads out.

    Bytes before/between frames that aren't MAGIC are accumulated as `noise`
    (the OpenSBI banner, stray prints) rather than being an error.
    """

    def __init__(self):
        self._buf = bytearray()
        self.noise = bytearray()  # non-frame bytes, kept for diagnostics

    def feed(self, data: bytes) -> list[bytes]:
        """Consume a chunk; return every complete frame payload found."""
        self._buf += data
        frames = []
        while True:
            start = self._buf.find(MAGIC)
            if start == -1:
                # No magic; all but the last byte is definitely noise (the
                # last byte could be the first half of a split MAGIC).
                keep = 1 if self._buf[-1:] == MAGIC[:1] else 0
                if len(self._buf) > keep:
                    self.noise += self._buf[: len(self._buf) - keep]
                    del self._buf[: len(self._buf) - keep]
                return frames
            if start > 0:
                self.noise += self._buf[:start]
                del self._buf[:start]
            header_end = len(MAGIC) + LEN_BYTES
            if len(self._buf) < header_end:
                return frames  # wait for the rest of the header
            length = int.from_bytes(self._buf[len(MAGIC):header_end], "little")
            if length > MAX_FRAME_LEN:
                # Desync: this wasn't a real header. Skip one byte and rescan.
                self.noise += self._buf[:1]
                del self._buf[:1]
                continue
            if len(self._buf) < header_end + length:
                return frames  # wait for the rest of the payload
            frames.append(bytes(self._buf[header_end:header_end + length]))
            del self._buf[:header_end + length]
