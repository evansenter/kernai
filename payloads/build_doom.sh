#!/bin/sh
# Build the DOOM payload (the kernel's `doom` cargo feature): full doomgeneric
# DOOM, cross-compiled for rv64 with picolibc (a real libc: malloc, string,
# stdio, soft-float via libgcc), linked with our own script into a two-segment
# W^X ELF the kernel's loader maps. Output lands where the kernel's build.rs
# embeds payloads from (PAYLOAD_DOOM), same as the Rust/craycast payloads.
#
# Integer-only on purpose: the kernel runs payloads with sstatus.FS=0 (no FPU),
# so -mabi=lp64 (soft-float) guarantees no F/D instruction is emitted — which is
# exactly why classic DOOM, being fixed-point, ports at all. -mcmodel=medany so
# code+data can sit at a low VA yet reach the 26 MiB data segment via PC-relative
# addressing without HI20 relocation overflow.
#
# doomgeneric is portable DOOM: a core plus six DG_* hooks + main(); we supply
# those in doomgeneric_kernai.c and the libc bottom-edge in sys_kernai.c, and
# drop every SDL/Allegro/X11/Windows backend from the vendor tree.
set -eu

DIR="$(cd "$(dirname "$0")" && pwd)"
DOOMDIR="$DIR/doom"
SRCDIR="$DOOMDIR/vendor/doomgeneric/doomgeneric"
BUILD="$DOOMDIR/build"
OUT="$DIR/target/riscv64gc-unknown-none-elf/release"
mkdir -p "$BUILD" "$OUT"

# doomgeneric is not vendored in-repo (it's third-party GPL, ~5 MiB); clone it
# at a pinned SHA on first build. Only the two kernai shims + doom.ld are ours.
DG_REPO="${DOOMGENERIC_REPO:-https://github.com/ozkl/doomgeneric}"
DG_PIN="dcb7a8dbc7a16ce3dda29382ac9aae9d77d21284"
if [ ! -d "$SRCDIR" ]; then
    echo "cloning doomgeneric @ $DG_PIN ..."
    git clone --quiet "$DG_REPO" "$DOOMDIR/vendor/doomgeneric"
    git -C "$DOOMDIR/vendor/doomgeneric" checkout --quiet "$DG_PIN"
fi

CC="${RISCV_GCC:-riscv64-unknown-elf-gcc}"
STRIP="${RISCV_STRIP:-riscv64-unknown-elf-strip}"
SPECS="${PICOLIBC_SPECS:-/usr/lib/picolibc/riscv64-unknown-elf/picolibc.specs}"

# ~26 s of game time by default (title screen → attract-mode demo playback).
# Overridable so a quick boot smoke test can render fewer frames, or an
# interactive (agent-driven) session can run longer. COLOR_EVERY is the
# full-color keyframe cadence (every Nth frame ships as PNG-able RGB).
FRAMES="${DOOM_FRAMES:-900}"
COLOR_EVERY="${DOOM_COLOR_EVERY:-24}"

CFLAGS="--specs=$SPECS -march=rv64imac -mabi=lp64 -mcmodel=medany \
  -O2 -ffreestanding -fno-stack-protector \
  -DNORMALUNIX -DLINUX -DDOOMGENERIC_RESX=320 -DDOOMGENERIC_RESY=200 \
  -DDOOM_FRAMES=$FRAMES -DDOOM_COLOR_EVERY=$COLOR_EVERY \
  -Wno-implicit-function-declaration -Wno-int-conversion \
  -I$SRCDIR"

# Vendor backends we replace (platform mains) or that need libraries we don't
# have (SDL/Allegro sound + music). Everything else in the tree is portable core.
skip() {
    case "$1" in
        doomgeneric_allegro|doomgeneric_emscripten|doomgeneric_linuxvt|\
        doomgeneric_sdl|doomgeneric_soso|doomgeneric_sosox|doomgeneric_win|\
        doomgeneric_xlib|i_allegromusic|i_allegrosound|i_sdlmusic|i_sdlsound)
            return 0 ;;
        *) return 1 ;;
    esac
}

objs=""
for src in "$SRCDIR"/*.c; do
    base="$(basename "$src" .c)"
    skip "$base" && continue
    o="$BUILD/$base.o"
    $CC $CFLAGS -c "$src" -o "$o"
    objs="$objs $o"
done

# Our two kernai shims: the six DG_* hooks + main(), and the libc syscall/stdio
# backend wired to kernai's ecall ABI.
for base in doomgeneric_kernai sys_kernai; do
    o="$BUILD/$base.o"
    $CC $CFLAGS -c "$DOOMDIR/$base.c" -o "$o"
    objs="$objs $o"
done

# Link with our script (two page-aligned W^X PT_LOAD segments, entry _start),
# then strip to shrink the copy embedded in the kernel image.
$CC $CFLAGS -T "$DOOMDIR/doom.ld" $objs -o "$BUILD/doom.elf"
$STRIP --strip-all -o "$OUT/doom" "$BUILD/doom.elf"
echo "built DOOM payload: $OUT/doom ($(wc -c < "$OUT/doom") bytes, $FRAMES frames, keyframe every $COLOR_EVERY)"
