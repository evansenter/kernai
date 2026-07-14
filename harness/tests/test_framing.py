"""M0 acceptance: the framing layer works before any kernel exists."""

import json
import unittest

from harness.framing import MAGIC, MAX_FRAME_LEN, FrameDecoder, encode_frame
from harness.transport import Loopback


class FrameDecoderTests(unittest.TestCase):
    def test_single_frame_round_trip(self):
        d = FrameDecoder()
        self.assertEqual(d.feed(encode_frame(b"hello")), [b"hello"])

    def test_empty_payload(self):
        d = FrameDecoder()
        self.assertEqual(d.feed(encode_frame(b"")), [b""])

    def test_byte_at_a_time(self):
        payloads = [b"a", b"bb" * 300, json.dumps({"id": 1}).encode()]
        wire = b"".join(encode_frame(p) for p in payloads)
        d = FrameDecoder()
        got = []
        for i in range(len(wire)):
            got += d.feed(wire[i:i + 1])
        self.assertEqual(got, payloads)

    def test_leading_noise_skipped_and_kept(self):
        banner = b"OpenSBI v1.3\nPlatform Name : riscv-virtio,qemu\n"
        d = FrameDecoder()
        got = d.feed(banner + encode_frame(b"x") + b"stray" + encode_frame(b"y"))
        self.assertEqual(got, [b"x", b"y"])
        self.assertEqual(bytes(d.noise), banner + b"stray")

    def test_magic_split_across_chunks(self):
        wire = encode_frame(b"payload")
        d = FrameDecoder()
        got = d.feed(b"noise" + wire[:1])  # ends with first magic byte
        got += d.feed(wire[1:])
        self.assertEqual(got, [b"payload"])
        self.assertEqual(bytes(d.noise), b"noise")

    def test_payload_may_contain_magic(self):
        p = b"aa" + MAGIC + b"\x00\x00\x00\x00" + MAGIC
        d = FrameDecoder()
        self.assertEqual(d.feed(encode_frame(p) + encode_frame(b"next")),
                         [p, b"next"])

    def test_desync_bogus_length_recovers(self):
        bogus = MAGIC + (MAX_FRAME_LEN + 1).to_bytes(4, "little")
        d = FrameDecoder()
        self.assertEqual(d.feed(bogus + encode_frame(b"real")), [b"real"])

    def test_oversize_payload_rejected(self):
        with self.assertRaises(ValueError):
            encode_frame(b"\x00" * (MAX_FRAME_LEN + 1))


class LoopbackTests(unittest.TestCase):
    """Frames through a real OS pipe (a cat subprocess), read with timeouts —
    the exact machinery the QEMU transport uses from M1 on."""

    def test_loopback_round_trip(self):
        lb = Loopback()
        try:
            lb.send_raw(b"boot banner noise\n")
            events = [{"id": i, "type": "loopback"} for i in range(3)]
            for e in events:
                lb.send(json.dumps(e).encode())
            got = [json.loads(lb.stream.next_frame(timeout=5)) for _ in events]
            self.assertEqual(got, events)
            self.assertIn(b"banner", bytes(lb.stream.decoder.noise))
        finally:
            lb.close()

    def test_eof_returns_none(self):
        lb = Loopback()
        lb.send(b"last")
        lb._proc.stdin.close()
        self.assertEqual(lb.stream.next_frame(timeout=5), b"last")
        self.assertIsNone(lb.stream.next_frame(timeout=5))
        lb._proc.wait(timeout=5)
        lb.stream.close()
        lb._proc.stdout.close()


if __name__ == "__main__":
    unittest.main()
