"""`make doom` — run full DOOM (doomgeneric) as a sandboxed kernai payload.

Boots the kernel built with the `doom` feature, hands QEMU the freely-licensed
Freedoom IWAD as a read-only memory device the kernel maps into the payload's
address space, sends `D`, and then:

  * prints DOOM's own stdout (its startup banner, WAD/zone/rendering init) as it
    arrives — each line is a `payload_output` event, untrusted and quota-capped
    like any other payload's output (P7);
  * renders every `frame` event — DOOM's 320x200 screen, downscaled in the
    payload to a grayscale grid and blitted via SYS_BLIT — as live ASCII;
  * reassembles the full-color keyframes DOOM streams via SYS_FRAME (base64
    `fbchunk` events) into real PNG screenshots, written to --out.

DOOM here is a real C program (~80 translation units) compiled against picolibc
and run entirely in U-mode: no FPU (it's fixed-point), no syscalls but kernai's
own ecall ABI, memory-isolated, and — because time is virtual and there's no
input — deterministic under -icount. With `--verify` a second boot must produce
byte-identical frame checksums (P9).
"""

import base64
import json
import os
import pathlib
import struct
import sys
import zlib

from .qemu import QemuKernel

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent

# QEMU loads the IWAD at this physical address — just past the kernel's 128 MiB
# frame pool (POOL_END), inside the 256 MiB we give the guest — and the kernel
# maps that window read-only into the DOOM payload. Must match WAD_PA/WAD_VA in
# kernel/src/payload.rs.
WAD_ADDR = 0x8800_0000

# DOOM's native screen; the payload streams full keyframes at this size.
COLOR_W, COLOR_H = 320, 200

WAD_CANDIDATES = [
    os.environ.get("DOOM_WAD", ""),
    "/usr/share/games/doom/freedoom1.wad",
    "/usr/share/doom/freedoom1.wad",
    str(REPO_ROOT / "freedoom1.wad"),
]


def find_wad():
    for p in WAD_CANDIDATES:
        if p and pathlib.Path(p).exists():
            return p
    sys.exit(
        "no IWAD found. Install one (`apt-get install freedoom`) or set "
        "DOOM_WAD=/path/to/iwad.wad. Tried:\n  "
        + "\n  ".join(c for c in WAD_CANDIDATES if c)
    )


def write_png(path, w, h, rgb, scale=1):
    """Minimal RGB PNG writer (stdlib only — no Pillow). Nearest-neighbour
    upscaled by `scale` so 320x200 is comfortable to view."""
    def chunk(typ, data):
        return (struct.pack(">I", len(data)) + typ + data
                + struct.pack(">I", zlib.crc32(typ + data) & 0xffffffff))

    ow, oh = w * scale, h * scale
    raw = bytearray()
    for y in range(oh):
        raw.append(0)  # filter type 0 (None) for this scanline
        row = rgb[(y // scale) * w * 3: (y // scale + 1) * w * 3]
        if scale == 1:
            raw += row
        else:
            for x in range(w):
                raw += row[x * 3:x * 3 + 3] * scale
    png = (b"\x89PNG\r\n\x1a\n"
           + chunk(b"IHDR", struct.pack(">IIBBBBB", ow, oh, 8, 2, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(bytes(raw), 6))
           + chunk(b"IEND", b""))
    pathlib.Path(path).write_bytes(png)


def boot(wad):
    """A QemuKernel wired for DOOM: 256 MiB RAM + the IWAD loaded as a device."""
    return QemuKernel(
        mem="256M",
        extra=["-device", f"loader,file={wad},addr=0x{WAD_ADDR:x}"],
    )


def run(animate, max_frames, timeout, out_dir=None):
    """One boot: send `D`, stream events. Returns (frame_checksums, png_paths).

    Prints DOOM's stdout and (when `animate`) renders ASCII frames in place;
    reassembles SYS_FRAME `fbchunk` streams into PNGs under `out_dir`.
    """
    frames = []
    pngs = []
    pending = b""          # partial line of DOOM stdout not yet newline-terminated
    color = {}             # seq -> chunk bytes, for the keyframe currently arriving
    tty = animate and sys.stdout.isatty()

    def flush_line(final=False):
        nonlocal pending
        while b"\n" in pending:
            line, pending = pending.split(b"\n", 1)
            print("    doom │ " + line.decode("latin1"))
        if final and pending:
            print("    doom │ " + pending.decode("latin1"))
            pending = b""

    def save_color():
        if not color or out_dir is None:
            return
        rgb = b"".join(color[s] for s in sorted(color))
        need = COLOR_W * COLOR_H * 3
        if len(rgb) >= need:
            idx = len(pngs)
            path = os.path.join(out_dir, f"doom_frame_{idx:03d}.png")
            write_png(path, COLOR_W, COLOR_H, rgb[:need], scale=2)
            pngs.append(path)
            print(f"    ● saved color screenshot {path} (frame ~{len(frames)})")
        color.clear()

    with boot(wad=find_wad()) as q:
        hello = q.next_event(timeout=60)
        assert hello and hello["type"] == "hello", f"no hello frame: {hello!r}"
        q.send(b"D")
        while True:
            e = q.next_event(timeout=timeout)
            if e is None:
                flush_line(final=True)
                print("    (kernel exited)")
                break
            t = e["type"]
            if t == "payload_output":
                # e["data"] is already un-escaped by json.loads(event); latin1
                # restores the raw payload bytes 1:1.
                pending += e["data"].encode("latin1")
                flush_line()
            elif t == "fbchunk":
                color[e["seq"]] = base64.b64decode(e["data"])
            elif t == "frame":
                save_color()  # the ASCII frame delimits a completed color frame
                frames.append(e["checksum"])
                if animate:
                    if tty:
                        sys.stdout.write("\033[H")
                    print(f"  kernai · DOOM — full doomgeneric, sandboxed   "
                          f"[frame {len(frames):3d}  checksum {e['checksum']}]   ")
                    for row in e["rows"]:
                        print("  │" + row + "│")
                    sys.stdout.flush()
                if len(frames) >= max_frames:
                    print(f"\n  (stopping after {max_frames} frames)")
                    break
            elif t == "payload_exit":
                flush_line(final=True)
                print(f"    doom exited (code {e.get('code')})")
                break
            elif t == "payload_load_fault":
                flush_line(final=True)
                print(f"    LOAD FAULT: {e.get('reason')}")
                break
            elif t in ("fault", "payload_killed", "panic"):
                flush_line(final=True)
                print(f"    {t.upper()}: {json.dumps(e)}")
                break
            elif t == "suite_done":
                flush_line(final=True)
                break
    return frames, pngs


def main():
    verify = "--verify" in sys.argv
    max_frames = 400
    out_dir = str(REPO_ROOT / "harness" / "doom_frames")
    for a in sys.argv:
        if a.startswith("--frames="):
            max_frames = int(a.split("=", 1)[1])
        if a.startswith("--out="):
            out_dir = a.split("=", 1)[1]
    os.makedirs(out_dir, exist_ok=True)
    timeout = 300

    if sys.stdout.isatty():
        sys.stdout.write("\033[2J\033[H")
    print("Booting kernai, running full DOOM as a sandboxed payload...\n")

    frames, pngs = run(animate=True, max_frames=max_frames, timeout=timeout, out_dir=out_dir)
    if not frames:
        print("\n  no frames rendered — did DOOM reach the rendering loop? "
              "(build with `make doom`)")
        sys.exit(1)
    print(f"\n  Rendered {len(frames)} frames from a real C DOOM build, "
          f"sandboxed by kernai; saved {len(pngs)} color screenshots to {out_dir}.")

    if verify:
        print("  Verifying determinism (second boot)...")
        frames2, _ = run(animate=False, max_frames=len(frames), timeout=timeout)
        ok = frames == frames2 and len(frames) > 0
        print(f"  Determinism: {len(frames)} frame checksums, two boots "
              f"{'IDENTICAL' if ok else 'DIVERGED'} — DOOM replays exactly (P9).")
        sys.exit(0 if ok else 1)
    sys.exit(0)


if __name__ == "__main__":
    main()
