#!/bin/sh
# Builds the release archive for this host into dist/. Linux builds static musl.
set -eu

die() { echo "dist: $*" >&2; exit 1; }

cd "$(git rev-parse --show-toplevel)"

[ -z "$(git status --porcelain --untracked-files=no)" ] || die "tree has uncommitted changes; commit or drop them"

id=$(cargo pkgid)
version=${id##*[#@]}
tag="v$version"
[ "$(git rev-parse -q --verify "refs/tags/$tag^{commit}")" = "$(git rev-parse HEAD)" ] ||
  die "HEAD is not tag $tag from Cargo.toml version; tag HEAD or check out the tag"

host=$(rustc -vV | sed -n 's/^host: //p')
case "$host" in
  *-linux-gnu) target=${host%-gnu}-musl ;;
  *) target=$host ;;
esac

cargo build --release --locked --target "$target"

name="scswarm-$tag-$target.tar.gz"
rm -rf dist
mkdir dist
COPYFILE_DISABLE=1 tar -czf "dist/$name" -C "target/$target/release" scswarm
cd dist
if command -v sha256sum >/dev/null; then sha256sum "$name"; else shasum -a 256 "$name"; fi >"$name.sha256"
echo "dist/$name"
