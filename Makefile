.PHONY: lint fmt test e2e

# comment budget + slop scan, then rust fmt check + clippy. pre-commit hook runs same.
lint:
	python3 scripts/comment-budget.py
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

test:
	cargo test

# live slot set, two tracks and two projects against rootless podman, Linux or macOS podman machine; no tokens spent.
e2e:
	cargo test --test e2e -- --ignored --nocapture
