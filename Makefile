# Release helpers. `make deb` needs cargo-deb (`cargo install cargo-deb`).

CARGO ?= cargo
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)

.PHONY: all build release check fmt clippy test deb clean

all: build

build:
	$(CARGO) build -p mdedit-app

release:
	$(CARGO) build --release -p mdedit-app

fmt:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

test:
	$(CARGO) test --workspace

# The merge gate.
check: fmt clippy test

# Builds the release binary and target/debian/mdedit_<version>-1_<arch>.deb.
deb:
	@command -v cargo-deb >/dev/null || { echo "cargo-deb missing: cargo install cargo-deb" >&2; exit 1; }
	$(CARGO) deb -p mdedit-app
	@ls -1 target/debian/mdedit_$(VERSION)-*.deb

clean:
	$(CARGO) clean
