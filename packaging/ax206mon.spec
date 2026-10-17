# Built by packaging/build-in-container.sh, which vendors the Rust crates
# into the source tarball so the build itself runs offline.

# Release profile already strips the binary; no debuginfo subpackage.
%global debug_package %{nil}

Name:           ax206mon
Version:        0.1.0
Release:        1%{?dist}
Summary:        Status dashboard for AX206-protocol USB mini displays
License:        MIT
URL:            https://github.com/
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  pkgconfig(libusb-1.0)
BuildRequires:  systemd-rpm-macros
Requires(pre):  shadow-utils
%{?systemd_requires}

%description
ax206mon drives cheap 3.5" USB "AIDA64" mini screens that use the AX206
digital photo frame protocol (USB ID 1908:0102, e.g. QDtech USB-Display)
and shows a live dashboard with clock, CPU, memory, disk, temperature,
load and network rates. Runs as a systemd service and reconnects
automatically when the display is unplugged.

%prep
%autosetup

%build
export CARGO_HOME=%{_builddir}/cargo-home
cargo build --release --frozen --offline

%check
export CARGO_HOME=%{_builddir}/cargo-home
cargo test --release --frozen --offline

%install
install -Dpm 0755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dpm 0644 packaging/%{name}.service %{buildroot}%{_unitdir}/%{name}.service
install -Dpm 0644 packaging/70-%{name}.rules %{buildroot}%{_udevrulesdir}/70-%{name}.rules
install -Dpm 0644 packaging/config.toml %{buildroot}%{_sysconfdir}/%{name}/config.toml

%pre
getent group %{name} >/dev/null || groupadd -r %{name}
getent passwd %{name} >/dev/null || \
    useradd -r -g %{name} -d / -s /sbin/nologin -c "AX206 display service" %{name}
exit 0

%post
%systemd_post %{name}.service
# Apply the udev rule to an already plugged-in display.
udevadm control --reload-rules >/dev/null 2>&1 || :
udevadm trigger --subsystem-match=usb --attr-match=idVendor=1908 >/dev/null 2>&1 || :

%preun
%systemd_preun %{name}.service

%postun
%systemd_postun_with_restart %{name}.service

%files
%license LICENSE
%doc README.md
%{_bindir}/%{name}
%{_unitdir}/%{name}.service
%{_udevrulesdir}/70-%{name}.rules
%dir %{_sysconfdir}/%{name}
%config(noreplace) %{_sysconfdir}/%{name}/config.toml

%changelog
* Sat Oct 17 2026 Zeljko Milinkov <zeljko.milinkov@gmail.com> - 0.1.0-1
- Initial package
