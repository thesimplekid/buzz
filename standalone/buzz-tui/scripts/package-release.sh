#!/usr/bin/env bash
set -euo pipefail

BUZZ_REV="753bc22f42bbed8b7f3ea0c1d8e572bd8f5e4dfd"
BUZZ_REPOSITORY="https://github.com/block/buzz"

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(sed -n 's/^version = "\\([^"]*\\)"/\\1/p' "$project_root/Cargo.toml" | head -n 1)"
target_name="$(rustc -vV | sed -n 's/^host: //p')"
bundle_name="buzz-tui-${version}-${target_name}"
bundle_dir="$project_root/dist/$bundle_name"
checkout="$(mktemp -d)"
trap 'rm -rf "$checkout"' EXIT

cargo build --manifest-path "$project_root/Cargo.toml" --release --locked

git -C "$checkout" init --quiet
git -C "$checkout" remote add origin "$BUZZ_REPOSITORY"
git -C "$checkout" fetch --quiet --depth 1 origin "$BUZZ_REV"
git -C "$checkout" checkout --quiet --detach FETCH_HEAD
cargo build \
  --manifest-path "$checkout/Cargo.toml" \
  --release \
  --locked \
  -p buzz-acp \
  -p buzz-dev-mcp

mkdir -p "$bundle_dir"
suffix=""
if [[ "$target_name" == *windows* ]]; then
  suffix=".exe"
fi
cp "$project_root/target/release/buzz-tui$suffix" "$bundle_dir/"
cp "$checkout/target/release/buzz-acp$suffix" "$bundle_dir/"
cp "$checkout/target/release/buzz-dev-mcp$suffix" "$bundle_dir/"
cp "$project_root/README.md" "$bundle_dir/"
cp "$project_root/LICENSE" "$bundle_dir/"
chmod 0755 \
  "$bundle_dir/buzz-tui$suffix" \
  "$bundle_dir/buzz-acp$suffix" \
  "$bundle_dir/buzz-dev-mcp$suffix"

tar -C "$project_root/dist" -czf "$project_root/dist/$bundle_name.tar.gz" "$bundle_name"
printf '%s\n' "$project_root/dist/$bundle_name.tar.gz"
