.PHONY: prepare check test build update clean

LINT := uv run --script https://raw.githubusercontent.com/vladkens/lint/main/rust.py

prepare:
	cargo +nightly fmt
	cargo +nightly fmt --manifest-path tests/hello/Cargo.toml
	cargo +nightly fmt --manifest-path tests/cxx/Cargo.toml
	uvx ruff format --line-length 100 scripts
	$(LINT) fix
	cargo clippy --all-targets --fix --allow-dirty --allow-staged

check:
	cargo +nightly fmt --check
	cargo +nightly fmt --check --manifest-path tests/hello/Cargo.toml
	cargo +nightly fmt --check --manifest-path tests/cxx/Cargo.toml
	uvx ruff format --check --line-length 100 scripts
	cargo clippy --all-targets -- -D warnings
	$(LINT)

test:
	cargo test

build:
	cargo build --release
	ls -lh target/release/cargo-solo

update:
	cargo upgrade --incompatible
	cargo update

clean:
	cargo clean

.PHONY: e2e pins

e2e: # tests/hello built for every target and run in its OS: Linux in Docker, Windows on Windows
	uv run scripts/e2e.py all target/e2e

pins: # the packages re-pinned to what Debian and rustup have now, every file fetched from every mirror
	uv run scripts/pins.py
	cargo test -- --ignored every_package_is_on_every_mirror
