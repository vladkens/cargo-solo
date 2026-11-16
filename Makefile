.PHONY: prepare check test e2e pins build update clean

prepare:
	cargo +nightly fmt
	cargo +nightly fmt --manifest-path tests/hello/Cargo.toml
	uv run scripts/blank-lines.py --fix
	cargo clippy --all-targets --fix --allow-dirty --allow-staged

check:
	cargo +nightly fmt --check
	cargo +nightly fmt --check --manifest-path tests/hello/Cargo.toml
	uv run scripts/blank-lines.py
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

e2e:
	uv run scripts/e2e.py all target/e2e

pins:
	uv run scripts/pins.py
	cargo test -- --ignored every_package_is_on_every_mirror

build:
	cargo build --release
	ls -lh target/release/cargo-solo

update:
	cargo upgrade --incompatible
	cargo update

clean:
	cargo clean
