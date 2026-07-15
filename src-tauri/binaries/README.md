# Bundled cloudflared

Porta uses the official Cloudflare `cloudflared` 2026.7.1 release for both
macOS architectures. The executable files are intentionally ignored by Git,
but must exist beside this file before a local build:

- `cloudflared-aarch64-apple-darwin`
  - archive SHA-256: `6d4b59383cdad387834d7ae5704fc512882b2d078074bf5770e02b186a0981ed`
- `cloudflared-x86_64-apple-darwin`
  - archive SHA-256: `05871d772745b0f8398c7be89113a0b178474936ff20638b3b07c0e7262f717e`

Source archives:
`https://github.com/cloudflare/cloudflared/releases/tag/2026.7.1`
