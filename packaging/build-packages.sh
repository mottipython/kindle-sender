#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="${REPO_DIR}/dist"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "${REPO_DIR}/Cargo.toml" | head -1)"

# KS_BIN lets a binary built elsewhere (e.g. in an older-glibc container, so the
# .deb and tarball run on Debian/Ubuntu) be packaged instead of building here.
if [ -z "${KS_BIN:-}" ]; then
    echo "=== Building Kindle Sender Release Binary ==="
    cargo build --release --manifest-path "${REPO_DIR}/Cargo.toml"
    KS_BIN="${REPO_DIR}/target/release/kindle-sender"
fi
BIN="${KS_BIN}"
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
* Sun Oct 25 2026 Motti Schleider <motti.schleider@gmail.com> - 1.1.0-1
- Built-in MOBI, AZW, AZW3, PDF, FB2, TXT and HTML converters; calibre is now optional
- Metadata and cover editing without calibre

* Sun Oct 25 2026 Motti Schleider <motti.schleider@gmail.com> - 1.0.2-1
- Fix book names invisible in dark themes
- Always send from the GUI and watch folder (stale do_send=false made sends dry runs)

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
Depends: libc6, libssl3 | libssl3t64, libdbus-1-3, libfontconfig1
Recommends: calibre
CTRL

echo "2.0" > "${DEB_TMP}/debian-binary"

(cd "${DEB_TMP}/control" && tar czf "${DEB_TMP}/control.tar.gz" .)
(cd "${DEB_TMP}/data" && tar czf "${DEB_TMP}/data.tar.gz" .)

DEB_NAME="kindle-sender_${VERSION}_amd64.deb"
ar -rc "${DIST_DIR}/${DEB_NAME}" "${DEB_TMP}/debian-binary" "${DEB_TMP}/control.tar.gz" "${DEB_TMP}/data.tar.gz"
rm -rf "${DEB_TMP}"

echo "DEB created in dist/"

# -------------------------------------------------------------
# 3. Generic tarball
# -------------------------------------------------------------
echo "=== Packaging tarball ==="
TAR_TMP=$(mktemp -d /tmp/ks-tar-build-XXXXXX)
TAR_DIR="kindle-sender-${VERSION}-linux-x86_64"
mkdir -p "${TAR_TMP}/${TAR_DIR}"
install -m 0755 "${BIN}" "${TAR_TMP}/${TAR_DIR}/kindle-sender"
cp "${REPO_DIR}/README.md" "${REPO_DIR}/LICENSE" "${DESKTOP}" "${ICON_SVG}" "${TAR_TMP}/${TAR_DIR}/"
tar czf "${DIST_DIR}/${TAR_DIR}.tar.gz" -C "${TAR_TMP}" "${TAR_DIR}"
rm -rf "${TAR_TMP}"

echo "=== Generated Packages in ${DIST_DIR} ==="
ls -lh "${DIST_DIR}"/*"${VERSION}"*
