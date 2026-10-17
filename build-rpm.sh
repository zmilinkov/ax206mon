#!/usr/bin/env bash
# Build the RPM inside an AlmaLinux container. Output goes to ./dist/.
#
#   ./build-rpm.sh            # AlmaLinux 9
#   EL=10 ./build-rpm.sh      # AlmaLinux 10
#   ENGINE=podman ./build-rpm.sh
set -euo pipefail
cd "$(dirname "$0")"

EL=${EL:-9}
ENGINE=${ENGINE:-$(command -v docker || command -v podman || true)}
if [[ -z "$ENGINE" ]]; then
    echo "error: neither docker nor podman found" >&2
    exit 1
fi
IMAGE="ax206mon-build:el$EL"

"$ENGINE" build --build-arg EL="$EL" -t "$IMAGE" -f packaging/Dockerfile packaging

mkdir -p dist
# :Z relabels the mounts for SELinux (openSUSE Leap 16 enforces it).
# The named volume caches downloaded crates between builds.
"$ENGINE" run --rm \
    -v "$PWD:/src:ro,Z" \
    -v "$PWD/dist:/out:Z" \
    -v "ax206mon-cargo-el$EL:/root/.cargo/registry" \
    -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
    "$IMAGE" bash /src/packaging/build-in-container.sh

echo
ls -l dist/
