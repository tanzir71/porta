# Bundled cloudflared

Porta uses the official Cloudflare `cloudflared` 2026.7.1 release for macOS
and Windows. The executable files are intentionally ignored by Git, but the
binary for the current target must exist beside this file before a build:

- `cloudflared-aarch64-apple-darwin`
  - archive SHA-256: `6d4b59383cdad387834d7ae5704fc512882b2d078074bf5770e02b186a0981ed`
- `cloudflared-x86_64-apple-darwin`
  - archive SHA-256: `05871d772745b0f8398c7be89113a0b178474936ff20638b3b07c0e7262f717e`
- `cloudflared-x86_64-pc-windows-msvc.exe`
  - executable SHA-256: `ccb0756de288d3c2c076d19764ca53e0849a10f2dd9c23f8656ac42bdeb45001`
  - source asset: `cloudflared-windows-amd64.exe`

Source archives:
`https://github.com/cloudflare/cloudflared/releases/tag/2026.7.1`
