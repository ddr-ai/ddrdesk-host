# DDRDesk host

Persistent remote-desktop **server** for Fedora 44 + KDE Plasma (Wayland). Pair with the [ddrdesk-ios](https://github.com/ddr-ai/ddrdesk-ios) client.

The daemon starts at boot, survives logout/lock/crash, streams whatever is on the laptop’s real display (Plasma Login Manager greeter, lock screen, or Plasma session), and is stopped only when you `systemctl stop` / `disable` it.

## Technology (why these choices)

| Piece | Choice | Why |
|---|---|---|
| Language | **Rust** (tokio) | Long-running network service next to input/DRM; memory-safe; already installed on this host (`rustc 1.98`). |
| Capture | **gpu-screen-recorder** + `gsr-kms-server` | This Lunar Lake panel uses **AR30 + CCS modifiers**. `ffmpeg -f kmsgrab` rejects that format. GSR imports the DRM framebuffer via EGL/modifiers and encodes with **VAAPI H.264** (`iHD`). `gsr-kms-server` has `cap_sys_admin`, so capture works **before login** and after logout — not only inside a PipeWire session. |
| Encode | **H.264 High, CBR, keyint 15, no B-frames** (GSR) | Hardware encode on Intel Arc 130V/140V; iPhone/iPad **VideoToolbox** decodes it with almost no copy. Short GOP so a reconnect resyncs in ≤ 0.5 s. |
| Transport | **TLS 1.3 over TCP**, `TCP_NODELAY`, ALPN `ddrdesk/1` | Direct host-to-phone. No rendezvous, no relay. Network.framework on iOS talks TLS 1.3 reliably; QUIC interop between Quinn and `NWProtocolQUIC` is still brittle. |
| Input | **uinput** (`/dev/uinput`) | Kernel evdev events reach the **Plasma Login Manager greeter**, KWin lock screen, and the Plasma session. libei / `ydotool` / portal RemoteDesktop only work inside a user session. |
| Display fit | **kscreen-doctor** mode + scale + rotation | Official KDE API. Logical size is matched to the client’s point size so text/icons stay readable without pinch-zoom. Encoder `-s WxH` also scales the bitstream to the device. |
| Discovery | **mDNS** `_ddrdesk._tcp` + **UPnP** + **STUN** (address hint only) | 9-digit ID is the *secret*, not a directory key. LAN: browse mDNS and match `id=`. WAN: after one LAN pair (or UPnP public mapping) the client dials cached endpoints. STUN is not a relay. |
| Service | **systemd** `Restart=always`, enabled at `multi-user.target`, linger | Starts without a console login; restarts on crash; `systemctl stop/disable` is the explicit off switch. |

See [PROTOCOL.md](PROTOCOL.md) for the on-the-wire format.

## Connection ID (no third-party server)

On first start the daemon writes a random **9-digit** ID to `~/.local/share/ddrdesk/connection-id`.

```
ddrdeskd --print-id
# e.g. 582 914 337
```

Enter that ID in the iOS app.

- **Same network:** mDNS finds the host; TLS + ID authenticate.
- **Different network:** the host opens TCP/44789 (firewalld + UPnP). The client reuses endpoints it saved at pairing time (LAN IPs + STUN/UPnP public address). There is **no** central ID server and **no** video relay.

A 9-digit code cannot encode a full IPv4+port, so the first connection is easiest on the same Wi-Fi. After that, WAN reconnects use the cached public mapping.

## Requirements

- Fedora 44 (KDE Plasma), Wayland
- `gpu-screen-recorder` (Fedora package; uses `/usr/bin/gsr-kms-server`)
- User in groups `input`, `video`, `render` (the install script does this)
- Port **44789/tcp** open on the host firewall

## Install (this laptop)

```bash
git clone https://github.com/ddr-ai/ddrdesk-host
cd ddrdesk-host
./scripts/install.sh
ddrdeskd --print-id
```

The script builds a release binary, installs `/usr/local/bin/ddrdeskd`, a systemd unit, a uinput udev rule, linger, and the firewall port, then `enable --now ddrdesk`.

## Usage

| Action | Command |
|---|---|
| Connection ID | `ddrdeskd --print-id` |
| New ID | `ddrdeskd --regen-id` then `sudo systemctl restart ddrdesk` |
| Status | `systemctl status ddrdesk` |
| Logs | `journalctl -u ddrdesk -f` |
| Stop (stays installed) | `sudo systemctl stop ddrdesk` |
| Disable across reboots | `sudo systemctl disable --now ddrdesk` |
| Enable again | `sudo systemctl enable --now ddrdesk` |
| Uninstall | `./scripts/uninstall.sh` |

If nobody is logged in, the client shows the **Plasma Login Manager** greeter. Type with the iPhone/iPad system keyboard; after a successful login Plasma starts and the stream follows. Logout/lock returns to the greeter or lock screen automatically (the daemon always captures the active DRM plane).

## Adaptive display

The client sends pixel size, point size, scale, and orientation. The host:

1. Picks a `kscreen-doctor` mode + scale (and rotation for portrait) so Plasma’s *logical* size is close to the device’s point size.
2. Tells GSR to encode at a matching even resolution (`-s WxH`).
3. Restores the previous laptop mode when the client disconnects.

This applies to iPhone and iPad, portrait and landscape.

## Security notes

- TLS 1.3, self-signed cert, TOFU pin on the client.
- 9-digit ID (~30 bits): compared in constant time, **8 tries / 30 s / IP**. Pairing on LAN, then cert pin, is the real protection.
- No inbound video path; the host only accepts control+auth then pushes H.264.
- The daemon runs as your user (not root). KMS uses `gsr-kms-server`’s file capability.
