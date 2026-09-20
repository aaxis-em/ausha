# Installs an already-built tree rather than compiling: scripts/package.sh
# stages the binaries, man pages and licence, tars them, and hands that to
# rpmbuild as Source0.

# There is nothing to extract debuginfo from: the binaries arrive already built
# and stripped, and leaving this on makes the build fail outright.
%global debug_package %{nil}

Name:           ausha
Version:        %{version}
Release:        1%{?dist}
Summary:        Stream desktop audio to receivers on the local network

License:        GPL-3.0-or-later
URL:            https://aaxis-em.github.io/ausha/
Source0:        %{name}-%{version}.tar.gz

ExclusiveArch:  x86_64

Requires:       (ffmpeg or ffmpeg-free)
Requires:       pulseaudio-utils
Requires:       iproute

Suggests:       alsa-utils

%description
Ausha captures whatever this computer is already playing and streams it as Opus
audio in RTP to receivers on the same network: the Ausha Android app, or
ausha-recv on another computer.

Call mode runs the traffic the other way as well, publishing a receiver's
microphone here as a capture device named ausha, so a call taken on this machine
can be spoken into from a phone.

This package contains both the sender (ausha) and the desktop receiver
(ausha-recv).

%prep
%setup -q

%install
mkdir -p %{buildroot}
cp -a usr %{buildroot}/

%files
%license %{_datadir}/licenses/%{name}/LICENSE
%doc %{_datadir}/doc/%{name}/Readme.md
%{_bindir}/ausha
%{_bindir}/ausha-recv
%{_mandir}/man1/ausha.1.gz
%{_mandir}/man1/ausha-recv.1.gz

%changelog
* Sun Sep 20 2026 Aaxis-em <aashishadhikari693@gmail.com> - 0.1.0-1
- First release.
