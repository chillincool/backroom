#!/usr/bin/env bash
# Builds Backroom's Flatpak and installs it for the current user.
# Needs flatpak and flatpak-builder; the GNOME runtime and SDK come from
# Flathub (added for this user if missing).
#
#   scripts/flatpak.sh            build and install
#   scripts/flatpak.sh --bundle   also write backroom.flatpak to share
set -euo pipefail

cd "$(dirname "$0")/.."
manifest=build-aux/flatpak/net.chillincool.backroom.yml

flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak-builder --user --install-deps-from=flathub --force-clean --install \
	--repo=build-flatpak/repo build-flatpak/build "$manifest"

if [[ ${1:-} == --bundle ]]; then
	flatpak build-bundle build-flatpak/repo backroom.flatpak net.chillincool.backroom
	echo "Wrote backroom.flatpak (install with: flatpak install --user backroom.flatpak)"
fi
echo "Installed. Run it from the app menu or: flatpak run net.chillincool.backroom"
