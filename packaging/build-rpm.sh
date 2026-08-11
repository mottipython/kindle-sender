#!/usr/bin/env bash
# Build an installable RPM from this checkout.
#
#   sudo dnf install rpm-build rpmdevtools python3-devel desktop-file-utils
#   ./packaging/build-rpm.sh
#
# The result lands in ~/rpmbuild/RPMS/noarch/ and installs with `dnf install`.
set -euo pipefail

cd "$(dirname "$0")/.."
name=kindle-sender
version=$(sed -n 's/^version = "\(.*\)"/\1/p' pyproject.toml | head -1)

for tool in rpmbuild rpmdev-setuptree; do
    command -v "$tool" >/dev/null || {
        echo "missing $tool — sudo dnf install rpm-build rpmdevtools" >&2
        exit 1
    }
done

rpmdev-setuptree
tarball="$HOME/rpmbuild/SOURCES/$name-$version.tar.gz"

if git rev-parse --git-dir >/dev/null 2>&1; then
    # Tracked files only, so venv/ and build junk can never leak into a release.
    git archive --format=tar.gz --prefix="$name-$version/" -o "$tarball" HEAD
else
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    mkdir -p "$tmp/$name-$version"
    tar -c --exclude=venv --exclude=.git --exclude='__pycache__' \
        --exclude='*.egg-info' --exclude=dist --exclude=build . |
        tar -x -C "$tmp/$name-$version"
    tar -czf "$tarball" -C "$tmp" "$name-$version"
fi

echo "source: $tarball"

if ! rpmbuild -ba "packaging/$name.spec"; then
    cat >&2 <<'MSG'

--------------------------------------------------------------------
If that failed on "Failed build dependencies", install them and retry:

    sudo dnf builddep ~/rpmbuild/SRPMS/kindle-sender-*.nosrc.rpm

Recent Fedora no longer ships setuptools with the system Python, so on
a clean machine this usually means:

    sudo dnf install python3-setuptools python3-pip python3-wheel
--------------------------------------------------------------------
MSG
    exit 1
fi

echo
echo "Built:"
find "$HOME/rpmbuild/RPMS" -name "$name-$version*.rpm" -newermt '-5 minutes'
echo
echo "Install with:  sudo dnf install ~/rpmbuild/RPMS/noarch/$name-$version-*.noarch.rpm"
