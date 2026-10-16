# PXR build helpers.
#
#   make            # debug build
#   make release    # optimized compiler at target/release/pxr
#   make test       # run the full test suite
#   make check      # type-check every example with the debug compiler
#   make examples   # run every example
#   make bench      # build release + run benchmarks

CARGO ?= cargo
PXR   := ./target/release/pxr
EXAMPLES := $(wildcard examples/*.php) examples/migration/main.php examples/async/main.php examples/async/server.php
CHECK_EXAMPLES := $(EXAMPLES) examples/async/tcp.php

.PHONY: all build release test check examples bench fmt clean

all: build

build:
	$(CARGO) build

release:
	$(CARGO) build --release

test:
	$(CARGO) test

check: build
	@for f in $(CHECK_EXAMPLES); do \
		echo "checking $$f"; \
		./target/debug/pxr check $$f || exit 1; \
	done

examples: release
	@for f in $(EXAMPLES); do \
		echo "== $$f =="; \
		$(PXR) run $$f || exit 1; \
	done

bench: release
	python3 benchmarks/bench.py

fmt:
	$(CARGO) fmt

clean:
	$(CARGO) clean
