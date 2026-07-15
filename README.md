# Porta

Porta is a free, macOS-first app for sharing a folder or local web server with
a public HTTPS link. Pick or drag in a folder, and Porta serves it through a
Cloudflare Quick Tunnel without an account, a terminal command, or a hosted
Porta service.

![Porta — Share a folder. Get a link.](site/assets/og.png)

## What it does

- Shares folders with browsable directory pages, downloads, and optional uploads.
- Forwards a public URL to an existing service on a local port.
- Adds optional password protection, request/visitor stats, and first-visitor notifications.
- Copies new links automatically and keeps active shares running from the menu bar.
- Can launch at login and restart selected shares automatically.
- Stores share settings locally and passwords in the macOS Keychain.

Porta bundles `cloudflared`; users do not need to install it separately or
create a Cloudflare account. Porta has no analytics or telemetry.

## Security and appropriate use

A Quick Tunnel URL is hard to guess, but it is **public**. Anyone who receives
the URL can reach the share while it is running. Enable Porta's password option
for sensitive shares, remove uploads unless they are needed, and stop a share
when you are finished.

Password protection uses HTTP Basic authentication at Porta's local server.
The browser connection is HTTPS, but Porta does not provide end-to-end
encryption from the browser through Cloudflare to the shared files, and it
should not be described as doing so.

Cloudflare documents Quick Tunnels as a testing and development feature, not a
production hosting service. They have no SLA, support at most 200 concurrent
in-flight requests, and do not support Server-Sent Events. Use of Porta and
Quick Tunnels remains subject to the
[Cloudflare terms and Quick Tunnel documentation](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/).

## Install

Porta 1.0 currently targets Apple silicon Macs. Download
[`Porta_1.0.0_aarch64.dmg`](../../releases/latest/download/Porta_1.0.0_aarch64.dmg),
open it, and drag Porta to Applications.

The official 1.0 release is ad-hoc signed and has not been notarized by Apple,
so macOS will likely block the first launch:

1. Try opening Porta once, then dismiss the warning.
2. Open **System Settings → Privacy & Security** and scroll to **Security**.
3. Click **Open Anyway**, enter your login password, then confirm **Open**.

Apple makes **Open Anyway** available for about an hour after the blocked
launch. Only override Gatekeeper when the DMG came from Porta's
[official GitHub Release](../../releases/latest) and this SHA-256 matches:

```text
d76ba419489c0b3b434b4a28eb6171cfcbb4ad1394f8d98bcb779eb2fcac2257
```

See [Apple's official instructions](https://support.apple.com/guide/mac-help/open-an-app-by-overriding-security-settings-mh40617/mac).

Then drag a folder into Porta or choose **Share a folder**, review the share
options, and use the copied `trycloudflare.com` link. Closing the window hides
it; choose **Quit Porta** from the menu bar to stop the resident app and its
active tunnels.

## Build from source

Prerequisites are the current stable Rust toolchain, Node.js with npm, and both
macOS `cloudflared` binaries described in
[`src-tauri/binaries/README.md`](src-tauri/binaries/README.md).

```sh
npm --prefix ui install
cd src-tauri
cargo tauri build
```

For development, run `cargo tauri dev` from `src-tauri`. To verify both halves
without opening the app:

```sh
npm --prefix ui run build
cd src-tauri
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Architecture

Porta is a Tauri 2 application with a React/TypeScript interface and a Rust
backend. Folder shares are served from a loopback-only Axum server, and the
bundled `cloudflared` sidecar connects that server—or a selected local port—to
Cloudflare's edge. Share state is stored atomically in the app-data directory;
passwords are kept out of that file and stored in Keychain.
