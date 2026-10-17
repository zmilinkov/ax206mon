# Build environment for the RPM. EL selects the AlmaLinux major version:
#   docker build --build-arg EL=9  ...   (default)
#   docker build --build-arg EL=10 ...
ARG EL=9
FROM docker.io/library/almalinux:${EL}

# CRB carries some -devel packages on EL9/EL10; pkgconfig() lets dnf pick
# the right libusb devel package name on either release.
RUN dnf -y install dnf-plugins-core \
 && (dnf config-manager --set-enabled crb || true) \
 && dnf -y install rpm-build rust cargo gcc tar gzip findutils \
        'pkgconfig(libusb-1.0)' systemd-rpm-macros \
 && dnf clean all \
 && rustc --version && cargo --version
