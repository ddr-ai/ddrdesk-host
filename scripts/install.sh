#!/usr/bin/env bash
# Build and install DDRDesk as a persistent systemd service on Fedora/KDE.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
USER_NAME="${SUDO_USER:-${USER:-ddr}}"
USER_HOME="$(getent passwd "$USER_NAME" | cut -d: -f6)"
USER_UID="$(id -u "$USER_NAME")"

if [[ $EUID -ne 0 ]]; then
  echo "Re-running with sudo…"
  exec sudo --preserve-env=HOME "$0" "$@"
fi

echo "==> Installing packages (gpu-screen-recorder, firewalld helpers)"
if command -v dnf >/dev/null; then
  dnf install -y gpu-screen-recorder 2>/dev/null || true
fi

echo "==> Building ddrdeskd (release)"
sudo -u "$USER_NAME" bash -lc "cd '$ROOT' && cargo build --release"
install -m 0755 "$ROOT/target/release/ddrdeskd" /usr/local/bin/ddrdeskd
install -m 0755 "$ROOT/gui/ddrdesk-gui" /usr/local/bin/ddrdesk-gui
install -m 0755 "$ROOT/scripts/ddrdesk-ctl" /usr/local/bin/ddrdesk-ctl
install -m 0644 "$ROOT/packaging/ddrdesk.desktop" /usr/share/applications/ddrdesk.desktop
install -m 0440 "$ROOT/packaging/ddrdesk.sudoers" /etc/sudoers.d/ddrdesk
visudo -c -f /etc/sudoers.d/ddrdesk >/dev/null
if command -v dnf >/dev/null; then
  dnf install -y python3-pyside6 2>/dev/null || true
fi
# Refresh KDE app launcher
sudo -u "$USER_NAME" XDG_RUNTIME_DIR="/run/user/${USER_UID}" update-desktop-database "$USER_HOME/.local/share/applications" 2>/dev/null || true
update-desktop-database /usr/share/applications 2>/dev/null || true

echo "==> Installing systemd unit, udev rule"
sed "s/User=ddr/User=${USER_NAME}/; s/Group=ddr/Group=${USER_NAME}/; s|/home/ddr|${USER_HOME}|g; s/user@1000.service/user@${USER_UID}.service/; s|/run/user/1000|/run/user/${USER_UID}|" \
  "$ROOT/systemd/ddrdesk.service" > /etc/systemd/system/ddrdesk.service
install -m 0644 "$ROOT/udev/99-ddrdesk.rules" /etc/udev/rules.d/99-ddrdesk.rules
udevadm control --reload-rules
udevadm trigger --subsystem-match=misc --action=add || true
# Load uinput if needed
modprobe uinput 2>/dev/null || true
if [[ ! -e /etc/modules-load.d/uinput.conf ]]; then
  echo uinput > /etc/modules-load.d/uinput.conf
fi

echo "==> Groups and linger (service starts at boot without a console login)"
usermod -aG input,video,render "$USER_NAME" || true
loginctl enable-linger "$USER_NAME"

echo "==> Firewall"
if command -v firewall-cmd >/dev/null && firewall-cmd --state >/dev/null 2>&1; then
  firewall-cmd --permanent --add-port=44789/tcp || true
  firewall-cmd --reload || true
fi

echo "==> Enable and start ddrdesk.service"
systemctl daemon-reload
systemctl enable ddrdesk.service
systemctl restart ddrdesk.service
sleep 1
systemctl --no-pager --full status ddrdesk.service || true

echo
echo "Connection ID:"
sudo -u "$USER_NAME" /usr/local/bin/ddrdeskd --print-id || true
echo
echo "Stop:    sudo systemctl stop ddrdesk"
echo "Disable: sudo systemctl disable --now ddrdesk"
echo "Logs:    journalctl -u ddrdesk -f"
echo "ID:      ddrdeskd --print-id"
echo "GUI:     ddrdesk-gui   (also in the application launcher: DDRDesk)"
