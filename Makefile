# Release helpers. `make deb` needs cargo-deb (`cargo install cargo-deb`);
# `make app` and `make dmg` run on macOS only.

CARGO ?= cargo
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)

.PHONY: all build release check fmt clippy test deb app dmg clean

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

# macOS: target/macos/mdedit.app, ad-hoc signed (scripts/macos-bundle.sh).
app:
	CARGO=$(CARGO) sh scripts/macos-bundle.sh

# macOS: the app above plus target/macos/mdedit-<version>.dmg.
dmg:
	CARGO=$(CARGO) sh scripts/macos-bundle.sh --dmg

clean:
	$(CARGO) clean
