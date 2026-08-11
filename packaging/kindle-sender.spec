%global forgeurl https://github.com/mottipython/kindle-sender

Name:           kindle-sender
Version:        1.0.1
Release:        1%{?dist}
Summary:        Fix EPUBs for Kindle and send them to your device by email

License:        MIT
URL:            %{forgeurl}
Source0:        %{name}-%{version}.tar.gz

BuildArch:      noarch
BuildRequires:  python3-devel

Requires:       python3-tkinter

# The EPUB fixes and email delivery are stdlib-only, so none of these are hard
# requirements — each one enables an optional part of the pipeline.
Recommends:     calibre
Recommends:     python3-keyring
Recommends:     python3-pillow

%description
Kindle Sender takes any ebook, converts it to EPUB if needed, repairs the
things that break on a Kindle, tidies its metadata and cover, and emails it to
your Send-to-Kindle address.

The repairs are a port of kindle-epub-fix: it adds the XML declaration Kindle
needs (without it every accented character becomes mojibake), repairs a missing
or invalid dc:language, strips link fragments that point at an id on <body>
(Kindle dead-ends on these, which is why some tables of contents do nothing),
and removes <img> tags pointing at files that are not in the archive.

Conversion, metadata and cover art shell out to calibre's command line tools;
install calibre to enable them. The EPUB repairs and email delivery work
without it.

%prep
%autosetup -n %{name}-%{version}

%generate_buildrequires
%pyproject_buildrequires

%build
%pyproject_wheel

%install
%pyproject_install
%pyproject_save_files -l kindle_sender

install -Dpm 0644 packaging/%{name}.desktop \
    %{buildroot}%{_datadir}/applications/%{name}.desktop
install -Dpm 0644 packaging/%{name}.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{name}.svg

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{name}.desktop
# Builds deliberately broken epubs and asserts every fix fires. Needs no
# network, no calibre and no display.
%{python3} %{_builddir}/%{name}-%{version}/tests/test_epub_fix.py

%files -f %{pyproject_files}
%license LICENSE
%doc README.md
%{_bindir}/%{name}
%{_datadir}/applications/%{name}.desktop
%{_datadir}/icons/hicolor/scalable/apps/%{name}.svg

%changelog
* Tue Aug 11 2026 Motti Schleider <motti.schleider@gmail.com> - 1.0.1-1
- Flag an unverified Kindle address copied from calibre

* Tue Aug 11 2026 Motti Schleider <motti.schleider@gmail.com> - 1.0.0-1
- Initial package
