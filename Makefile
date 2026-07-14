# kernai — build/run/test/debug. `make test` is the acceptance gate; it must
# pass from a fresh clone with the bootstrap documented in README.md.

.PHONY: test unsafe-budget clean

test: unsafe-budget
	python3 -m harness.runner all

unsafe-budget:
	ci/unsafe_budget.sh

clean:
	rm -rf harness/__pycache__ harness/tests/__pycache__
