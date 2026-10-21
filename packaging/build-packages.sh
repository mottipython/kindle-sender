#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="${REPO_DIR}/dist"
VERSION="1.0.1"

echo "=== Building Kindle Sender Release Binary ==="
cargo build --release --manifest-path "${REPO_DIR}/Cargo.toml"

BIN="${REPO_DIR}/target/release/kindle-sender"
DESKTOP="${REPO_DIR}/packaging/kindle-sender.desktop"
ICON_SVG="${REPO_DIR}/packaging/kindle-sender.svg"
ICONS_DIR="${REPO_DIR}/packaging/icons"

mkdir -p "${DIST_DIR}"

# -------------------------------------------------------------
# 1. Build RPM
# -------------------------------------------------------------
echo "=== Packaging RPM ==="
RPM_TMP=$(mktemp -d /tmp/ks-rpm-build-XXXXXX)
trap 'rm -rf "${RPM_TMP}"' EXIT

mkdir -p "${RPM_TMP}"/{BUILD,RPMS,SOURCES,SPECS,SRPMS,tmp}
cp "${BIN}" "${RPM_TMP}/SOURCES/kindle-sender"
cp "${DESKTOP}" "${RPM_TMP}/SOURCES/kindle-sender.desktop"
cp "${ICON_SVG}" "${RPM_TMP}/SOURCES/kindle-sender.svg"
cp -r "${ICONS_DIR}" "${RPM_TMP}/SOURCES/icons"

cat << SPEC > "${RPM_TMP}/SPECS/kindle-sender.spec"
Name:           kindle-sender
Version:        ${VERSION}
Release:        1%{?dist}
Summary:        Fix EPUBs for Kindle and send them to your device by email
License:        MIT
URL:            https://github.com/mottipython/kindle-sender
BuildArch:      x86_64
Recommends:     calibre

%description
Kindle Sender takes any ebook, converts it to EPUB if needed, repairs the
things that break on a Kindle, tidies its metadata and cover, and emails it to
your Send-to-Kindle address.

%install
mkdir -p %{buildroot}%{_bindir}
mkdir -p %{buildroot}%{_datadir}/applications
mkdir -p %{buildroot}%{_datadir}/icons/hicolor/scalable/apps

install -Dpm 0755 %{_sourcedir}/kindle-sender %{buildroot}%{_bindir}/kindle-sender
install -Dpm 0644 %{_sourcedir}/kindle-sender.desktop %{buildroot}%{_datadir}/applications/kindle-sender.desktop
install -Dpm 0644 %{_sourcedir}/kindle-sender.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/kindle-sender.svg

for sz in 16 24 32 48 64 128 256; do
    mkdir -p %{buildroot}%{_datadir}/icons/hicolor/\${sz}x\${sz}/apps
    install -Dpm 0644 %{_sourcedir}/icons/\${sz}x\${sz}/apps/kindle-sender.png %{buildroot}%{_datadir}/icons/hicolor/\${sz}x\${sz}/apps/kindle-sender.png
done

%files
%{_bindir}/kindle-sender
%{_datadir}/applications/kindle-sender.desktop
%{_datadir}/icons/hicolor/*/apps/kindle-sender.*

%changelog
* Wed Oct 21 2026 Motti Schleider <motti.schleider@gmail.com> - 1.0.1-1
- Port to Rust with Slint native GUI
SPEC

rpmbuild \
    --define "_topdir ${RPM_TMP}" \
    --define "_tmppath ${RPM_TMP}/tmp" \
    --define "%_builddir ${RPM_TMP}/BUILD" \
    -bb "${RPM_TMP}/SPECS/kindle-sender.spec"

cp "${RPM_TMP}"/RPMS/x86_64/*.rpm "${DIST_DIR}/"
echo "RPM created in dist/"

# -------------------------------------------------------------
# 2. Build DEB
# -------------------------------------------------------------
echo "=== Packaging DEB ==="
DEB_TMP=$(mktemp -d /tmp/ks-deb-build-XXXXXX)

# Data tree
mkdir -p "${DEB_TMP}/data/usr/bin"
mkdir -p "${DEB_TMP}/data/usr/share/applications"
mkdir -p "${DEB_TMP}/data/usr/share/icons/hicolor/scalable/apps"

install -m 0755 "${BIN}" "${DEB_TMP}/data/usr/bin/kindle-sender"
install -m 0644 "${DESKTOP}" "${DEB_TMP}/data/usr/share/applications/kindle-sender.desktop"
install -m 0644 "${ICON_SVG}" "${DEB_TMP}/data/usr/share/icons/hicolor/scalable/apps/kindle-sender.svg"

for sz in 16 24 32 48 64 128 256; do
    mkdir -p "${DEB_TMP}/data/usr/share/icons/hicolor/${sz}x${sz}/apps"
    install -m 0644 "${ICONS_DIR}/${sz}x${sz}/apps/kindle-sender.png" "${DEB_TMP}/data/usr/share/icons/hicolor/${sz}x${sz}/apps/kindle-sender.png"
done

# Control tree
mkdir -p "${DEB_TMP}/control"
INSTALLED_SIZE=$(du -sk "${DEB_TMP}/data" | cut -f1)

cat << CTRL > "${DEB_TMP}/control/control"
Package: kindle-sender
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: amd64
Installed-Size: ${INSTALLED_SIZE}
Maintainer: Motti Schleider <motti.schleider@gmail.com>
Homepage: https://github.com/mottipython/kindle-sender
Description: Fix EPUBs for Kindle and send them to your device by email
 Kindle Sender takes any ebook, converts it to EPUB if needed, repairs the
 things that break on a Kindle, tidies its metadata and cover, and emails it to
 your Send-to-Kindle address.
Recommends: calibre
CTRL

echo "2.0" > "${DEB_TMP}/debian-binary"

(cd "${DEB_TMP}/control" && tar czf "${DEB_TMP}/control.tar.gz" .)
(cd "${DEB_TMP}/data" && tar czf "${DEB_TMP}/data.tar.gz" .)

DEB_NAME="kindle-sender_${VERSION}_amd64.deb"
ar -rc "${DIST_DIR}/${DEB_NAME}" "${DEB_TMP}/debian-binary" "${DEB_TMP}/control.tar.gz" "${DEB_TMP}/data.tar.gz"
rm -rf "${DEB_TMP}"

echo "DEB created in dist/"

echo "=== Generated Packages in ${DIST_DIR} ==="
ls -lh "${DIST_DIR}"/*.rpm "${DIST_DIR}"/*.deb
