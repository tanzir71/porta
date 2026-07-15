# Changelog

All notable changes to Porta are documented in this file.

## [1.2.0] - 2026-07-15

### Added

- Configurable tunnel-provider profiles for managed Cloudflare tunnels, ngrok, and direct custom tunnel CLIs.
- A global default provider plus optional per-share overrides without changing existing share data or IPC naming.
- In-app provider setup, executable selection, secure credential entry, connection testing, readiness status, editing, and removal.
- Provider-aware visitor address headers and a version-2 store migration that preserves all 1.1 shares.

### Security

- Provider tokens stay out of `store.json` and process arguments; Porta stores them in Keychain or Credential Manager and passes them through environment variables.
- Custom commands are launched directly without a shell, with bounded arguments, validated executables, HTTPS URLs, headers, patterns, and environment-variable names.

### Changed

- Cloudflare Quick Tunnel remains the built-in, account-free default, while tunnel lifecycle, restart, cleanup, and status handling now operate through a provider-neutral adapter.
- The app, README, release notes, and GitHub Pages explain provider choice and third-party account/terms responsibilities.

## [1.1.0] - 2026-07-15

### Added

- Native Windows 10/11 x64 support with a per-user NSIS setup executable.
- Bundled and checksum-verified Windows `cloudflared` sidecar.
- Windows Credential Manager, startup-app, notification-area, taskbar, File Explorer, and native title-bar integration.
- Cross-platform GitHub Actions builds that publish macOS and Windows installers with SHA-256 files.

### Changed

- App copy, folder paths, tray icons, process supervision, and atomic persistence now adapt to macOS or Windows without changing the persisted IPC contract.
- The official landing page now offers platform-specific downloads and trust instructions for Gatekeeper and SmartScreen.

## [1.0.0] - 2026-07-15

### Added

- Native macOS app for publishing folders or local ports through account-free Cloudflare Quick Tunnels.
- Folder listings with nested breadcrumbs, filtering, MIME types, ETags, byte-range downloads, and optional collision-safe uploads.
- Optional HTTP Basic authentication backed by macOS Keychain rather than the on-disk share store.
- Persistent share settings, automatic starts, launch-at-login support, and live clipboard updates.
- Request, byte, and unique-visitor counters plus optional first-visitor notifications.
- Resident menu-bar controls, live tray state, configurable Dock visibility, native folder drag-and-drop, and single-instance behavior.
- Bundled Apple Silicon and Intel `cloudflared` sidecars and an ad-hoc-signed macOS DMG build.

### Reliability

- Supervised tunnel processes restart with bounded backoff and surface actionable offline or repeated-failure messages.
- Closing the window keeps shares active; stopping, deleting, or quitting tears down local servers and tunnel processes.
- Live edits reapply password, listing, and upload settings, while display-name-only edits preserve the current public URL.
- Missing shared folders transition to an honest error state instead of leaving a stale live card.

### Security

- Every requested path is canonicalized and confined to its shared root, including encoded traversal and escaping symlinks.
- Server-rendered share names and filenames are HTML-escaped.
- Uploads are limited to 2 GiB, never overwrite an existing file, and are disabled unless explicitly enabled.
- Porta includes no analytics or telemetry. Quick Tunnel URLs remain public to anyone who has the link; they are not end-to-end encrypted.
