#!/usr/bin/env bash
# Installs (or removes) Backroom's Linux app for the current user:
# ~/.local/bin/backroom-gtk, its desktop entry (which handles the
# backroom:// login callback), icons and AppStream metadata.
#
#   scripts/linux-install.sh            build (release) and install
#   scripts/linux-install.sh --debug    install the debug build instead
#   scripts/linux-install.sh --remove   uninstall
set -euo pipefail

cd "$(dirname "$0")/.."
app=net.chillincool.backroom
data=crates/backroom-gtk/data
bin_dir=${XDG_BIN_HOME:-$HOME/.local/bin}
share=${XDG_DATA_HOME:-$HOME/.local/share}

refresh() {
	update-desktop-database -q "$share/applications" 2>/dev/null || true
	gtk-update-icon-cache -q -t "$share/icons/hicolor" 2>/dev/null || true
}

if [[ ${1:-} == --remove ]]; then
	rm -f "$bin_dir/backroom-gtk" \
		"$share/applications/$app.desktop" \
		"$share/metainfo/$app.metainfo.xml" \
		"$share/icons/hicolor/scalable/apps/$app.svg" \
		"$share/icons/hicolor/symbolic/apps/$app-symbolic.svg"
	refresh
	echo "Removed. Settings and history stay in ~/.config/backroom and ~/.local/share/backroom."
	exit 0
fi

profile=release
[[ ${1:-} == --debug ]] && profile=debug
if [[ $profile == release ]]; then
	cargo build --locked --release -p backroom-gtk
else
	cargo build --locked -p backroom-gtk
fi

install -Dm755 "target/$profile/backroom-gtk" "$bin_dir/backroom-gtk"
# Exec= with the full path: ~/.local/bin isn't on every desktop's PATH.
sed "s|^Exec=backroom-gtk|Exec=$bin_dir/backroom-gtk|" "$data/$app.desktop" |
	install -Dm644 /dev/stdin "$share/applications/$app.desktop"
install -Dm644 "$data/$app.metainfo.xml" "$share/metainfo/$app.metainfo.xml"
install -Dm644 "$data/icons/hicolor/scalable/apps/$app.svg" "$share/icons/hicolor/scalable/apps/$app.svg"
install -Dm644 "$data/icons/hicolor/symbolic/apps/$app-symbolic.svg" \
	"$share/icons/hicolor/symbolic/apps/$app-symbolic.svg"
refresh
xdg-mime default "$app.desktop" x-scheme-handler/backroom 2>/dev/null || true
echo "Installed $profile build: $bin_dir/backroom-gtk (in your app menu as Backroom)."
