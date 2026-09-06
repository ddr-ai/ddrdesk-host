#!/usr/bin/env bash
set -euo pipefail
if [[ $EUID -ne 0 ]]; then
  exec sudo "$0" "$@"
fi
systemctl disable --now ddrdesk.service 2>/dev/null || true
rm -f /etc/systemd/system/ddrdesk.service /usr/local/bin/ddrdeskd /etc/udev/rules.d/99-ddrdesk.rules
systemctl daemon-reload
udevadm control --reload-rules
if command -v firewall-cmd >/dev/null && firewall-cmd --state >/dev/null 2>&1; then
  firewall-cmd --permanent --remove-port=44789/tcp || true
  firewall-cmd --reload || true
fi
echo "DDRDesk host uninstalled. State remains in ~/.local/share/ddrdesk"
