#!/usr/bin/env bash
set -euo pipefail

: "${CPR_CARGO_CACHE:?Cargo cache directory is required}"
: "${CPR_BACKEND_IMAGE:?Pinned Dockerfile toolchain image is required}"
: "${CPR_VERSION:?Release version is required}"
: "${CPR_GIT_SHA:?Source revision is required}"
: "${CPR_BUILD_TIME:?Build time is required}"
if [[ ${#CPR_GIT_SHA} != 40 || "$CPR_GIT_SHA" == *[!a-f0-9]* ]]; then
  printf '%s\n' 'An exact source revision is required' >&2
  exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
staging="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/cpr-ci-backend.XXXXXXXX")"
trap 'rm -rf "$staging"' EXIT
mkdir -p "$CPR_CARGO_CACHE" "$staging/backend" "$staging/release" "$root/dist/docker/linux-amd64"
cache="$(cd "$CPR_CARGO_CACHE" && pwd)"

# Match backend-builder's inputs, including its pinned image toolchain. In
# particular, do not accidentally activate a host checkout's rustup override.
cp "$root/backend/Cargo.toml" "$root/backend/Cargo.lock" "$staging/backend/"
cp -R "$root/backend/apps" "$root/backend/crates" "$root/backend/migrations" "$staging/backend/"
cp "$root/release/version.yaml" "$staging/release/"

docker run --rm --platform linux/amd64 --user "$(id -u):$(id -g)" \
  --env CARGO_HOME=/cargo --env CARGO_TARGET_DIR=/cargo/target --env HOME=/tmp \
  --env RUST_MIN_STACK=16777216 \
  --env CPR_VERSION --env CPR_GIT_SHA --env CPR_BUILD_TIME --env CPR_BUILD_TYPE=release \
  --volume "$cache:/cargo" --volume "$staging:/app:ro" --workdir /app/backend \
  "$CPR_BACKEND_IMAGE" cargo build --release --locked --bin codex-proxy-rs

cp "$cache/target/release/codex-proxy-rs" "$root/dist/docker/linux-amd64/codex-proxy-rs"
