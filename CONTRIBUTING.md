# Contributing

## Requirements

- Rust toolchain. `rust-toolchain.toml` pins channel `stable` with `rustfmt` and `clippy`.
- Python 3 for `make lint`. The script uses the standard library alone.
- For `make e2e`: rootless podman on Linux, or a podman machine on macOS.

## Build

```
cargo build --release
cargo run -- <args>
```

## Check before each commit

The repo has no CI. Run these checks yourself before you open a pull request:

1. Run `make lint`. It runs the comment budget script, `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`.
2. Run `cargo test`.
3. If you change slots, the pool or podman calls, run `make e2e`. It spends no agent tokens. It runs about 320 s on macOS. It writes `~/.config/scswarm/host.yaml` and `E2E_*` files in `~/.config/scswarm/credentials/`, and removes them. It refuses to run for a user whose host file it did not write: slots and container names belong to the user.

To run `make lint` on each commit, install a hook:

```
printf '#!/bin/sh\nexec make lint\n' > .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit
```

## Release

Releases build on a Mac with `gh` logged in. The Mac builds `aarch64-apple-darwin` natively and `x86_64-unknown-linux-musl` with the `rust-lld` linker that Rust ships, so it needs no Linux host.

1. Set `version` in `Cargo.toml`, commit it as `build(release): vX.Y.Z`, and push.
2. Run `make release`.

`make release` builds committed `HEAD` in a temporary worktree, so uncommitted changes never ship. After the build it tags `HEAD` `vX.Y.Z`, pushes the tag and publishes `scswarm-vX.Y.Z-<target>.tar.gz` for each target plus `SHA256SUMS`. It refuses a logged-out `gh`, a `HEAD` not on `origin/main`, an existing release, or a tag on another commit. A Mac cannot run the Linux binary: a Linux user smoke-tests it after the release. If the release changes `work` argv or stdin, write a release note.

## Comment budget

`scripts/comment-budget.py` caps comment prose in `.rs`, `.py`, `.ts`, `.tsx`, `.js`, `.jsx` and `.md` files.

| Check | Warn | Fail |
|---|---|---|
| comment chars per file | over 15% | over 20% |
| comment chars over the tree | over 13% | over 15% |
| lines per comment block | over 2 | over 4 |
| articles per comment word, 8 words or more | over 18% | none |

A phrase on the `TELLS` list in the script fails the check. Pass `--strict` to fail on warnings too.

## Propose a change

- Bug or small fix: open a GitHub issue.
- New behavior or a design change: write an RFC. Flow and template: [docs/rfcs/README.md](docs/rfcs/README.md).

## Design rules

- Read [docs/refs/architecture.md](docs/refs/architecture.md) and [docs/spec/contract.md](docs/spec/contract.md) first.
- Tool state lives in git: branches, land refs, fail marks. Add no database and no state file.
- The tool holds no project layout. Task dirs, id patterns and prompts come from the project's `SWARM.yaml` and `scswarm.yaml`.
- Dependencies are `clap`, `serde`, `serde_json`, `serde-saphyr`. Justify each new one in the pull request.
- Error messages name the field, the expected format and the shape of the value received. They never echo the raw value or a secret.
- Write tests for behavior only. Write no tests for data or config.

## Commit messages, pull request and issue titles

Follow [Conventional Commits 1.0.0 (conventionalcommits.org)](https://www.conventionalcommits.org/en/v1.0.0/): `type(scope): description`, lowercase, no period.

| Type | Use for |
|---|---|
| `feat` | new behavior |
| `fix` | bug fix |
| `perf` | speed or resource use, same behavior |
| `refactor` | code change with no behavior change |
| `test` | tests alone |
| `docs` | docs alone |
| `build` | `Cargo.toml`, `Makefile`, toolchain, Containerfile |
| `chore` | anything else, no `src/` change |

- Scope is the module, for example `host` for `src/host.rs`, or an area: `slot`, `steps`, `status`.
- A change to the CLI, `SWARM.yaml`, `scswarm.yaml` or the host file that breaks existing setups takes `!` after the scope and a `BREAKING CHANGE:` footer.
- Example: `fix(host): resolve the device nodes once at load`.
