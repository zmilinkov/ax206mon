#!/usr/bin/env bash
# Runs inside the AlmaLinux build container (see ../build-rpm.sh).
# /src = project (read-only), /out = where the RPMs go.
set -euo pipefail

name=ax206mon
src=/src
out=/out

version=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$src/Cargo.toml" | head -1)
spec_version=$(sed -n 's/^Version: *//p' "$src/packaging/$name.spec" | tr -d '[:space:]')
if [[ "$version" != "$spec_version" ]]; then
    echo "error: Cargo.toml version ($version) != spec Version ($spec_version)" >&2
    exit 1
fi

echo "==> $(rustc --version) on $(. /etc/os-release; echo "$PRETTY_NAME")"

work=$(mktemp -d)
tree="$work/$name-$version"
mkdir -p "$tree"
cp -a "$src"/{Cargo.toml,Cargo.lock,LICENSE,README.md,src,packaging,.cargo} "$tree/"

echo "==> vendoring crates"
(cd "$tree" && cargo vendor --locked --versioned-dirs vendor >> .cargo/config.toml)

mkdir -p ~/rpmbuild/SOURCES
tar -C "$work" -czf ~/rpmbuild/SOURCES/"$name-$version".tar.gz "$name-$version"

echo "==> rpmbuild"
rpmbuild -ba "$tree/packaging/$name.spec"

mkdir -p "$out"
cp -v ~/rpmbuild/RPMS/*/"$name"-*.rpm ~/rpmbuild/SRPMS/"$name"-*.src.rpm "$out/"
if [[ -n "${HOST_UID:-}" ]]; then
    chown "${HOST_UID}:${HOST_GID:-$HOST_UID}" "$out"/*.rpm
fi
