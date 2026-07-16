#!/bin/sh
# Build the C payloads (the `cpayloads` kernel feature) with clang → rv64 and
# link them with the shared payload linker script, into the same release dir
# the kernel's build.rs embeds Rust payloads from.
#
# Integer-only on purpose: the kernel runs payloads with sstatus.FS=0 (no FPU),
# so -march=rv64imac -mabi=lp64 (soft-float) guarantees no FP instruction is
# emitted. This is also why real Doom is fixed-point.
set -eu

DIR="$(cd "$(dirname "$0")" && pwd)"
OUT="$DIR/target/riscv64gc-unknown-none-elf/release"
mkdir -p "$OUT"

CC="${CLANG:-clang}"
LD="${LLD:-ld.lld}"
CFLAGS="--target=riscv64-unknown-elf -march=rv64imac -mabi=lp64 -ffreestanding -fno-builtin -nostdlib -O2 -Wall -Wextra"

for name in craycast; do
    src="$DIR/$name"
    "$CC" $CFLAGS -c "$src/entry.S"   -o "$src/entry.o"
    "$CC" $CFLAGS -c "$src/payload.c" -o "$src/payload.o"
    "$LD" -T "$DIR/link.ld" "$src/entry.o" "$src/payload.o" -o "$OUT/$name"
    echo "built C payload: $name -> $OUT/$name"
done
