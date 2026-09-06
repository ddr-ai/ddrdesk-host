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
systemctl enable --now ddrdesk.service
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
