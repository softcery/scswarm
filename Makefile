.PHONY: lint fmt test e2e e2e-namespace release

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
	cargo test --test e2e slot_set_follows_host_file_live -- --ignored --nocapture

# runner `namespace` as root in a throwaway Linux container, under rootless podman or a podman machine; no tokens spent.
e2e-namespace:
	podman run --rm --volume $(CURDIR):/src:z --workdir /src \
		--env CARGO_TARGET_DIR=/src/target/e2e-namespace --env HOME=/tmp/home \
		docker.io/library/rust:1-bookworm \
		sh -c 'mkdir -m 755 -p "$$HOME" && cargo test --test e2e namespace -- --ignored --nocapture'

# tag HEAD v<Cargo.toml version>, build macOS arm64 and linux x86_64 musl, publish GitHub release. macOS only.
release:
	scripts/release.sh
