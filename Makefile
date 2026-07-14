# kernai — build/run/test/debug. `make test` is the acceptance gate; it must
# pass from a fresh clone with the bootstrap documented in README.md.
#
# All QEMU invocations route through harness/qemu.py so the determinism
# flags (-icount shift=1,sleep=off — P9) live in exactly one place.

TARGET     := riscv64gc-unknown-none-elf
KERNEL_ELF := kernel/target/$(TARGET)/release/kernai

.PHONY: build run debug gdb test fmt clippy unsafe-budget clean

build:
	cd kernel && cargo build --release

run: build
	python3 -m harness.qemu

# Boot halted with a gdb stub on :1234; attach from another terminal.
debug: build
	python3 -m harness.qemu --gdb

gdb:
	gdb-multiarch $(KERNEL_ELF) -ex "target remote localhost:1234"

test: unsafe-budget fmt clippy build
	python3 -m harness.runner all

fmt:
	cd kernel && cargo fmt --check

clippy:
	cd kernel && cargo clippy --release -- -D warnings

unsafe-budget:
	ci/unsafe_budget.sh

clean:
	cd kernel && cargo clean
	rm -rf harness/__pycache__ harness/tests/__pycache__
