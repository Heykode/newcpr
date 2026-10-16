#!/usr/bin/env bash
set -euo pipefail

# Optional source-build path. CI's prebuilt binary path keeps its direct cache mount.
if [ "$#" -ne 4 ]; then
  echo "usage: $0 BUILDER CACHE_DIRECTORY amd64|arm64 IMAGE_TAG" >&2
  exit 2
fi
builder="$1"
cache_directory="$2"
architecture="$3"
image_tag="$4"
case "$architecture" in amd64|arm64) ;; *) exit 2 ;; esac
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bash "$root/release/cargo-cache/cargo-cache.sh" restore "$builder" "$cache_directory" "$architecture"
docker buildx build --builder "$builder" --platform "linux/$architecture" \
  --file "$root/deploy/Dockerfile" --target runtime --load --tag "$image_tag" \
  --build-arg "CPR_VERSION=${CPR_VERSION:-}" \
  --build-arg "CPR_GIT_SHA=$(git -C "$root" rev-parse HEAD)" \
  --build-arg "CPR_BUILD_TIME=$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$root"
bash "$root/release/cargo-cache/cargo-cache.sh" snapshot "$builder" "$cache_directory" "$architecture"
