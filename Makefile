.PHONY: prepare check test build update clean

prepare:
	cargo +nightly fmt
	uv run scripts/blank-lines.py --fix
	cargo clippy --all-targets --fix --allow-dirty --allow-staged

check:
	cargo +nightly fmt --check
	uv run scripts/blank-lines.py
	cargo clippy --all-targets -- -D warnings

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
