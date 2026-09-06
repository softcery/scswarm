.PHONY: lint fmt test

# comment budget + slop scan, then rust fmt check + clippy. pre-commit hook runs same.
lint:
	python3 scripts/comment-budget.py
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

test:
	cargo test
