# kernai — build/run/test/debug. `make test` is the acceptance gate; it must
# pass from a fresh clone with the bootstrap documented in README.md.
#
# All QEMU invocations route through harness/qemu.py so the determinism
# flags (-icount shift=1,sleep=off — P9) live in exactly one place.

TARGET     := riscv64gc-unknown-none-elf
KERNEL_ELF := kernel/target/$(TARGET)/release/kernai

.PHONY: build payloads run debug gdb demo eval raycast doom doom-play test fmt clippy unsafe-budget clean

# Payloads build first: the kernel embeds their ELFs via include_bytes!
# (kernel/build.rs fails loudly if they're missing).
payloads:
	cd payloads && cargo build --release

build: payloads
	cd kernel && cargo build --release

run: build
	python3 -m harness.qemu

# Boot halted with a gdb stub on :1234; attach from another terminal.
debug: build
	python3 -m harness.qemu --gdb

gdb:
	gdb-multiarch $(KERNEL_ELF) -ex "target remote localhost:1234"

# Narrated tour (M0-M12) for humans (docs/WALKTHROUGH.md is the readable twin).
demo: build
	python3 -m harness.demo

# E1 (M12): score the two diagnostic surfaces against the seeded-fault set and
# print the scorecard. The `e1` acceptance check (in `make test`) asserts the
# gap; this target shows it.
eval: build
	python3 -m harness.eval

# Optional demo: a C-language fixed-point raycaster (Wolfenstein-lite, Doom's
# ancestor) running as a sandboxed kernai payload, rendered as ASCII frames.
# Needs clang + lld (rv64). Builds the C payload, then the kernel with the
# cpayloads feature, then plays it.
raycast: payloads
	payloads/build_c.sh
	cd kernel && cargo build --release --features cpayloads
	python3 -m harness.raycast

# Optional demo: full DOOM (doomgeneric) as a sandboxed kernai payload. Needs
# riscv64-unknown-elf-gcc + picolibc and a Doom IWAD (`apt-get install freedoom`,
# or set DOOM_WAD). Builds the C payload, then the kernel with the doom feature,
# then boots it — printing DOOM's stdout, ASCII frames, and saving color PNG
# screenshots to harness/doom_frames/. Add --verify to check determinism.
doom: payloads
	payloads/build_doom.sh
	cd kernel && cargo build --release --features doom
	python3 -m harness.doom

# The agentic loop closed on DOOM: start the suite over MCP, observe frame
# events, act via key symbols (SYS_GETKEY), end with the operator kill. Builds
# with a longer frame budget + denser color keyframes for the interactive run.
doom-play: payloads
	DOOM_FRAMES=2000 DOOM_COLOR_EVERY=12 payloads/build_doom.sh
	cd kernel && cargo build --release --features doom
	python3 -m harness.doom_play

test: unsafe-budget fmt clippy build
	python3 -m harness.runner all

fmt:
	cd kernel && cargo fmt --check
	cd payloads && cargo fmt --all --check

clippy: payloads
	cd kernel && cargo clippy --release -- -D warnings
	cd payloads && cargo clippy --release --workspace -- -D warnings

unsafe-budget:
	ci/unsafe_budget.sh

clean:
	cd kernel && cargo clean
	cd payloads && cargo clean
	rm -rf harness/__pycache__ harness/tests/__pycache__
