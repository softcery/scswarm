#!/bin/sh
# Builds every target from HEAD in a clean worktree, tags it v<Cargo.toml version>, publishes a GitHub release.
set -eu

targets="aarch64-apple-darwin x86_64-unknown-linux-musl"

die() { echo "release: $*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || die "aarch64-apple-darwin links on macOS alone; run on a Mac"
gh auth status >/dev/null 2>&1 || die "gh is not logged in; run gh auth login"

root=$(git rev-parse --show-toplevel)
repo=$(git -C "$root" remote get-url origin)
git -C "$root" fetch -q origin
git -C "$root" merge-base --is-ancestor HEAD origin/main || die "HEAD is not on origin/main; push it there first"

tree=$(mktemp -d)
trap 'git -C "$root" worktree remove --force "$tree"' EXIT
git -C "$root" worktree add -q --detach "$tree" HEAD
cd "$tree"

id=$(cargo pkgid)
tag="v${id##*[#@]}"
! gh release view "$tag" -R "$repo" >/dev/null 2>&1 || die "release $tag exists; bump version in Cargo.toml"
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  [ "$(git rev-parse "$tag^{commit}")" = "$(git rev-parse HEAD)" ] || die "tag $tag points to another commit than HEAD"
fi

rustup -q target add $targets
mkdir dist
for target in $targets; do
  CARGO_TARGET_DIR="$root/target" cargo build --release --locked --target "$target"
  COPYFILE_DISABLE=1 tar -czf "dist/scswarm-$tag-$target.tar.gz" -C "$root/target/$target/release" scswarm
done
(cd dist && shasum -a 256 scswarm-* >SHA256SUMS)

git rev-parse -q --verify "refs/tags/$tag" >/dev/null || git tag -a "$tag" -m "$tag"
git push -q origin "$tag"

gh release create "$tag" -R "$repo" --verify-tag --generate-notes dist/*
