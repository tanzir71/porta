# Porta — Codex Handoff & Build Loop

**Porta** is a free, UI-first tunneling app for macOS and Windows. Users visually pick folders (or local ports) and get a public `https://` link instantly. Cloudflare Quick Tunnel remains zero-config and account-free; managed Cloudflare, ngrok, and custom tunnel CLIs can be configured in the UI.

This document is a **loopable work order**. Run it iteration after iteration until every checkbox in [§7 Milestones](#7-milestones--the-loop-state) is checked. §8 defines the loop protocol. Preserve the established React design while extending the Rust/Tauri backend and IPC contract additively.

---

## 1. Product principles (read every loop)

1. **Free forever.** Porta has no account, paid API, or server we run. The built-in Cloudflare Quick option remains account-free; user-configured providers may require their own account or plan.
2. **Grandma-simple.** Drag folder in → link on clipboard. Every error message must say what to do next, in plain words.
3. **The UI is the spec.** Everything visible in `ui/src` must work exactly as the components imply. Never simplify a feature because the backend is hard.
4. **Quiet resident app.** System-tray first: closing the window hides it; the app keeps serving. Quit only from the tray menu.
5. **Honest security.** Tunnel URLs are public to anyone who has them. Password protection is our auth layer (basic-auth at the local server). Never claim end-to-end encryption, and explain that each selected provider's terms apply.

**Non-goals (do not build):** Porta-managed domains or DNS, TCP tunnels, Porta accounts/sync, Linux, Windows ARM64/32-bit, analytics/telemetry of any kind.

---

## 2. Competitor analysis — why Porta wins

| Product | Price | GUI? | Folder sharing | Key limits / friction |
|---|---|---|---|---|
| **ngrok** | Free tier; paid from ~$10/mo | Web dashboard only, CLI-driven | No first-class folder UI | Free: ~1 GB/mo bandwidth, 20k req/mo, interstitial warning page, random URLs; sessions historically time-limited |
| **Cloudflare Tunnel (cloudflared)** | Free (Quick Tunnels: no account) | **None — CLI only** | No | 200 in-flight request cap; no SSE; random URL per run; no SLA |
| **LocalCan** | **$89–119 one-time** | Excellent native macOS app | Not folder-first (ports/domains) | Paid; SSH-tunnel based |
| **Tailscale Funnel** | Free personal tier | App exists, Funnel is CLI | No | Requires account + tailnet; undisclosed bandwidth limit; `.ts.net` domains |
| **localtunnel** | Free, OSS | No | No | Unreliable public server, npm-only, no auth |
| **zrok** | Free tier (5 GB/day) | Minimal | Has "drives" concept | Requires account; developer-oriented |
| **Pinggy / localhost.run** | Freemium | No (SSH one-liners) | No | Session limits on free tier; random URLs |
| **bore / frp / chisel** | Free, OSS | No | No | Need your own VPS; deeply technical |

**The gap Porta fills:** tunnel vendors remain CLI-first, while folder sharing is rarely the primary object. Porta adds a native desktop folder mental model over a replaceable provider layer. Password protection and visitor stats stay free because they live in Porta's local server, not the tunnel provider.

*Sources: [ngrok free plan limits](https://ngrok.com/docs/pricing-limits/free-plan-limits), [ngrok pricing](https://ngrok.com/pricing), [Cloudflare Quick Tunnels docs](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/), [LocalCan](https://www.localcan.com/), [Tailscale Funnel](https://tailscale.com/docs/features/tailscale-funnel), [awesome-tunneling](https://github.com/anderspitman/awesome-tunneling), [Pinggy: ngrok alternatives](https://pinggy.io/blog/best_ngrok_alternatives/).*

---

## 3. Architecture

```
┌────────────────────────── Porta.app (Tauri v2) ──────────────────────────┐
│  WebView: React UI (ui/ — FINAL, do not redesign)                        │
│      ⇅ invoke/events == contract in ui/src/lib/ipc.ts                    │
│  Rust core:                                                              │
│    ShareManager  — persistence (JSON in app-data), lifecycle state machine│
│    FileServer    — axum + tower: ServeDir per folder share on 127.0.0.1:0 │
│                    • listing.html rendering (server-templates/)           │
│                    • basic-auth middleware (password from OS credentials) │
│                    • multipart upload handler (if allowUploads)           │
│                    • stats middleware (visitors/requests/bytes)           │
│    TunnelManager — resolves a provider profile, launches its direct CLI   │
│                    discovers the public URL, supervises, and restarts     │
│    Tray          — menu-bar/notification icon, quick toggles, Quit        │
│    Autostart     — tauri-plugin-autostart (launch at login)               │
└───────────────────────────────────────────────────────────────────────────┘
         folder share:  Browser → selected provider → tunnel CLI → axum → disk
         port share:    Browser → selected provider → tunnel CLI → localhost:PORT
```

**Key decisions (do not relitigate):**
- **Tauri v2**, stable channel. Bundle target-specific `cloudflared` for the zero-config default and managed Cloudflare profiles. ngrok/custom profiles use an executable explicitly selected by the user.
- One axum server per active folder share, normally bound to `127.0.0.1:0`; provider profiles may require a fixed local port. Port shares skip axum and point the provider at the user's port directly.
- `ProviderProfile` supports Cloudflare Quick, managed Cloudflare, ngrok, and custom direct commands. Custom arguments are never shell-parsed and can expand `{origin}`, `{host}`, and `{port}`.
- URL/readiness discovery is provider-specific and bounded to 30 seconds. Supervision restarts the selected process with the existing 1 s → 2 s → 4 s… backoff and three-strike error policy.
- Passwords and provider tokens are stored in macOS Keychain or Windows Credential Manager via `keyring`, never in the JSON store or process arguments.
- Stats middleware counts requests/bytes and normalizes the selected provider's visitor header, with safe fallbacks for `Cf-Connecting-Ip`, `X-Forwarded-For`, and `X-Real-Ip`.
- Quick Tunnels don't support SSE and cap at 200 in-flight requests — irrelevant for file sharing, but document in README.

---

## 4. Repo layout

```
tunnel/
├── HANDOFF.md              ← this file; check boxes as you complete criteria
├── ui/                     ← React UI (FINAL — see §6 rules)
│   ├── package.json  vite.config.ts  tsconfig.json  index.html
│   └── src/
│       ├── main.tsx  App.tsx  styles.css
│       ├── lib/ipc.ts       ← ★ THE CONTRACT ★
│       └── components/{Icons,ShareCard,AddShareSheet,SettingsSheet,ProviderSettings,EmptyState}.tsx
├── server-templates/
│   └── listing.html         ← visitor-facing directory page (embed via include_str!)
└── src-tauri/               ← YOU CREATE THIS (tauri init, then implement)
    ├── Cargo.toml  tauri.conf.json  tauri.{macos,windows}.conf.json  capabilities/
    ├── binaries/cloudflared-{aarch64-apple-darwin,x86_64-pc-windows-msvc.exe}  ← sidecars
    ├── icons/
    └── src/{main,shares,server,tunnel,provider,credentials,tray,settings,stats}.rs
```

---

## 5. The IPC contract

`ui/src/lib/ipc.ts` is the single source of truth — read it fully every loop before touching Rust. Summary (shapes/serde must match the TS types **exactly**; use `#[serde(rename_all = "camelCase")]`):

| Command | Signature | Notes |
|---|---|---|
| `list_shares` | `() → Share[]` | persisted across launches |
| `create_share` | `(input: CreateShareInput) → Share` | starts immediately unless `startNow:false` |
| `start_share` | `(id) → void` | resolve at `starting`; `live`+url arrives via event |
| `stop_share` | `(id) → void` | kills cloudflared + axum for that share |
| `delete_share` | `(id) → void` | stop + remove + keychain cleanup |
| `update_share` | `(id, patch: UpdateShareInput) → Share` | live share restarts transparently |
| `pick_folder` | `() → string \| null` | native dialog (tauri-plugin-dialog) |
| `reveal_in_finder` | `(path) → void` | |
| `open_url` | `(url) → void` | default browser (tauri-plugin-opener) |
| `get_settings` / `update_settings` | `→ Settings` | see Settings type |
| `list_provider_profiles` | `() → ProviderProfile[]` | includes immutable Cloudflare Quick default |
| `save_provider_profile` | `(input) → ProviderProfile` | secret is transacted through OS credentials |
| `delete_provider_profile` | `(id) → void` | blocked while default or explicitly assigned |
| `test_provider` | `(id) → ProviderTestResult` | starts a temporary loopback origin and cleans up |
| `pick_provider_executable` | `() → string \| null` | native file picker; backend revalidates absolute file |

**Events:** emit `app_event` (payload = `AppEvent` union) on every status/url change, removal, and a stats tick **at most once per second** per share.

**Drag & drop wiring (backend → UI):** enable Tauri's drag-drop; on `Enter/Over` run `window.dispatchEvent(new CustomEvent('porta:drag-hover'))` (via `eval`), on `Leave` → `'porta:drag-cancel'`, on `Drop` of a **directory** → `'porta:folder-dropped'` with `detail` = absolute path (first dir only; ignore plain files but toast via the same mechanism is not required). The UI already listens for these three events.

**Settings semantics:**
- `launchAtLogin` → tauri-plugin-autostart register/unregister.
- `autoStartShares` → on app launch, start every share with `autoStart:true`.
- `showDockIcon` → macOS activation policy or Windows `set_skip_taskbar`; persisted name stays unchanged for compatibility.
- `copyUrlOnStart` → when a share transitions to `live`, write URL to clipboard (tauri-plugin-clipboard-manager).
- `notifyOnFirstVisitor` → native notification on a share's first unique visitor (tauri-plugin-notification): title = share name, body = "Someone just opened your link."
- `defaultProviderId` → the provider inherited by shares whose compatibility-preserving `providerId` field is absent/null. Changing it restarts only live inherited shares.

---

## 6. Hard rules (read every loop)

1. Keep the established UI design intact. Platform-specific copy, path handling, native title-bar spacing, and additive IPC-compatible fixes are allowed when both operating systems remain visually consistent.
2. Every command returns `Result<T, String>`; error strings are user-facing — plain English, actionable, no Rust debug noise.
3. Window close = hide (keep serving). Real quit only from tray. Quitting stops all tunnels cleanly within 3 seconds using the platform's supported process-termination path.
4. Serve **only** within the shared folder — canonicalize every request path and reject traversal (`..`, symlinks escaping the root) with 404.
5. Uploads: only when `allowUploads`; reject files > 2 GB; never overwrite — collide as `name (2).ext`.
6. No telemetry and no Porta-hosted network calls. Only the selected tunnel-provider process may connect externally.
7. Conventional commits; one milestone criterion (or coherent group) per commit.
8. `cargo clippy -- -D warnings` and `cargo fmt --check` must pass before any commit.

---

## 7. Milestones — the loop state

Check boxes (`[x]`) as criteria pass. Work strictly top-to-bottom.

### M0 — Scaffold
- [x] `src-tauri` initialized (Tauri v2, `identifier: com.porta.app`), window: 480×640 min 420×520, `titleBarStyle: Overlay`, `hiddenTitle: true`, resizable
- [x] `npm install && npm run build` succeeds in `ui/`
- [x] `cargo tauri dev` launches showing the React UI (mock data OK at this stage)
- [x] cloudflared universal binaries fetched into `src-tauri/binaries/` + configured as sidecar with shell-scope permission
- [x] Verify: `cargo tauri dev` renders EmptyState or mock cards with no console errors

### M1 — Share store + CRUD (no tunnel yet)
- [x] `Share`/`Settings` structs mirror `ipc.ts` exactly (camelCase serde); unit test deserializes a TS-shaped JSON fixture
- [x] JSON persistence in app-data dir; atomic writes (write temp + rename)
- [x] `list/create/update/delete_share`, `pick_folder`, `reveal_in_finder`, `open_url`, `get/update_settings` implemented; statuses still fake `stopped`
- [x] `app_event` plumbing works: `update_share` from UI reflects instantly in a second window/devtools
- [x] Verify: create → quit → relaunch → share persists; delete removes it

### M2 — Local file server
- [x] axum ServeDir per folder share on `127.0.0.1:0`; correct MIME; ETag/Range for media
- [x] `listing.html` rendered with all placeholders per §7-notes in the template header; dirs-first A→Z; human sizes; breadcrumbs correct at any depth
- [x] `showListing:false` → serve `index.html` at `/` or a minimal 403 page if none exists
- [x] Path traversal blocked (test with `curl --path-as-is /a/../../etc/passwd` → 404)
- [x] basic-auth middleware when password set (realm "Porta"); password stored/read via Keychain
- [x] uploads: multipart POST to current dir when `allowUploads`; collision-safe; 303 redirect back
- [x] Verify: integration test hits a temp dir share: listing renders, file downloads byte-identical, traversal 404s, wrong password 401s

### M3 — Tunnel lifecycle
- [x] TunnelManager spawns sidecar `cloudflared tunnel --url http://127.0.0.1:{port} --no-autoupdate`, parses URL ≤30 s, transitions `starting→live` with URL via event
- [x] `stop_share` tears down child processes — zero orphan `cloudflared` after quit (verify `pgrep cloudflared`)
- [x] Crash supervision with backoff per §3; 3 strikes → `error` + friendly message
- [x] No-internet path: unreachable → `error` "Couldn't reach Cloudflare — check your internet connection and try again."
- [x] Port shares (`kind:"port"`) tunnel directly to the user's port
- [x] `copyUrlOnStart` honored
- [~] Verify: toggle a real folder share; open the trycloudflare URL from a phone; download a file

### M4 — Stats + notifications
- [x] Stats middleware: requests, bytes, unique visitors via `Cf-Connecting-Ip`; `stats_updated` event ≤1/s per share
- [x] `notifyOnFirstVisitor` fires native notification once per session per share
- [~] Verify: hit share from two IPs (phone on cellular) → visitors=2 in UI

### M5 — Resident app: tray, login, drag-drop
- [x] Tray icon (macOS template image; colored Windows notification-area image) with menu: per-share rows "● Client Mockups — Copy link / Turn off", "Share a folder…", "Open Porta", separator, "Quit Porta"
- [x] Tray icon state: idle vs ≥1 live share (badge/filled variant)
- [x] Window close hides; app keeps serving; Dock/taskbar icon policy follows `showDockIcon` live
- [x] `launchAtLogin` via tauri-plugin-autostart; macOS path verified in System Settings › Login Items
- [x] `autoStartShares` on launch (only when master switch on)
- [x] Native drag-drop wired to the three `porta:*` CustomEvents; dropping a real folder from Finder/File Explorer opens the create sheet with the correct absolute path
- [x] Single-instance plugin: second launch focuses existing window
- [~] Verify: log out/in → Porta running in the menu bar/notification area, auto-start shares live, links work

### M6 — Polish & hardening
- [x] All error strings audited against §6.2 (no `Error:`, no paths-only, always an action)
- [x] Share names sanitized in listing.html (HTML-escape everything server-rendered)
- [x] App icon (folder+beam motif matching `Logo` in `Icons.tsx`), 1024px master, iconset generated
- [x] `cargo tauri build` produces signed-or-ad-hoc .dmg that launches on a clean machine
- [x] README.md: what it is, screenshot placeholder, honest security note (public-but-unguessable URLs, password option, Cloudflare ToS: not for production hosting)
- [~] Full QA pass: §9 checklist all green

### M7 — Release
- [x] Version 1.0.0 tagged; CHANGELOG.md written
- [x] .dmg < 25 MB (cloudflared included)
- [x] Zero clippy warnings, zero TS errors (`npm run build`), zero orphan processes after 10 start/stop cycles (scripted)

### M8 — Windows 1.1
- [x] Windows 10/11 x64 backend, paths, credential storage, taskbar, tray, autostart, and process cleanup pass on `windows-latest`
- [x] Per-user unsigned NSIS installer contains the checksum-verified `cloudflared` sidecar and survives install/launch/uninstall smoke testing
- [x] Version 1.1.0 release contains Apple-silicon DMG and Windows x64 setup EXE with matching SHA-256 attachments
- [x] README, release notes, and GitHub Pages present platform-specific downloads and Gatekeeper/SmartScreen instructions

### M9 — Configurable providers 1.2
- [x] Version-2 store migration preserves every 1.1 share and defaults absent provider fields to built-in Cloudflare Quick
- [x] Managed Cloudflare, ngrok, and custom profiles validate configuration, keep credentials in OS storage, pass secrets only through environment variables, and never invoke a shell
- [x] Settings UI supports provider add/edit/remove/test/default flows; individual shares can inherit or override the default provider
- [x] Tunnel lifecycle, retry, local-port binding, URL readiness, visitor headers, live-profile restarts, and cleanup are provider-neutral
- [x] Rust, TypeScript, clippy, formatting, browser UI, migration, secret-persistence, path, and process-lifecycle gates pass locally
- [x] Local Apple-silicon 1.2.0 DMG is ad-hoc signed, mounts with version metadata intact, starts and quits through the installed-app path, leaves no orphan, and remains below 25 MB
- [ ] Version 1.2.0 release contains refreshed Apple-silicon DMG and Windows x64 setup EXE with verified SHA-256 attachments, and GitHub Pages publishes the final checksums

---

## 8. Loop protocol (run this every iteration)

```
1. Read §1 (principles), §6 (rules), and ui/src/lib/ipc.ts.
2. Find the FIRST unchecked [ ] in §7. That is your only task.
3. Implement the smallest change that satisfies it.
4. Run the milestone's Verify step + `cargo clippy -- -D warnings`
   + `cargo fmt --check` + (if ui/ touched) `cd ui && npm run build`.
5. If green: flip [ ]→[x] in HANDOFF.md, commit (conventional message,
   include the criterion text in the body).
6. If blocked >2 attempts: append the blocker to §10 with what you tried,
   mark the box [~], move to the next box. Never delete a box.
7. Repeat until every box is [x] and §10 is empty (resolve [~] items last).
```

---

## 9. QA checklist (M6 gate)

- Share a folder with 1,000 files → listing renders < 1 s; filter works
- Share a 2 GB video → Range requests let it scrub in Safari on a phone
- Folder with spaces/emoji/CJK in name and filenames → URLs and listing correct
- Password share: wrong password 401; correct password browses; password removed via Edit → auth gone after transparent restart
- Toggle share off while a visitor downloads → connection drops, no crash, no orphan
- Sleep laptop 10 min with live share → wake → share recovers (supervision) or shows honest error
- Rename share while live → card updates, URL unchanged
- Two shares of the same folder → both work independently
- Delete the folder on disk while shared → status `error`: "This folder was moved or deleted. Pick it again to reshare."
- Quit from tray → no `cloudflared` process remains in Activity Monitor or Task Manager
- Add and test one managed Cloudflare, ngrok, and custom profile; confirm tokens never appear in `store.json` or process arguments
- Change the default while an inherited share is live, then switch one share to an override; confirm only affected tunnel processes restart
- Stop/test/quit every provider kind → no tunnel-provider process remains in Activity Monitor or Task Manager
- Fresh macOS or Windows user account → app launches, no missing-permission crashes

## 10. Blockers (append-only)

- [~] **M3 public-tunnel recheck for 1.2 (2026-07-15):** the ignored live smoke test again started the real bundled helper and received `https://employees-urban-casino-hometown.trycloudflare.com/porta-live-smoke.txt`, but every download attempt failed with local DNS error `Could not resolve host` until the bounded 90-second deadline. The guard stopped and reaped the helper. This reconfirms the environment-level DNS limitation below; deterministic server, adapter, package, and orphan gates remain green.
- [~] **M3 live phone verification (2026-07-15):** macOS Accessibility permission blocked automated interaction with the running Porta window. Two runs of `cargo test --test live_tunnel_smoke -- --ignored --nocapture` started Porta's real file server, launched the bundled cloudflared, received valid Quick Tunnel URLs, and left zero orphan processes, but this environment retained negative DNS results after `1.1.1.1` resolved the host; direct edge-IP download also timed out. Re-run the ignored smoke test on an unrestricted network, then open its printed URL on a phone and download the fixture.
- [~] **M4 cellular stats verification (2026-07-15):** `cargo test server::tests::counts_requests_streamed_bytes_and_unique_cloudflare_visitors -- --exact` passes and proves two distinct `Cf-Connecting-Ip` values produce `visitors=2`, but a physical cellular phone is unavailable and the M3 public-tunnel blocker prevents an honest UI/device check. After resolving M3, load the URL once on Wi-Fi and once on cellular, then confirm the card shows two visitors.
- [~] **M5 login-session verification (2026-07-15):** a temporary opted-in folder share proved that direct app startup registers `~/Library/LaunchAgents/Porta.plist`, creates the tray, auto-starts cloudflared, reaches `live`, and serves its public listing successfully. Two safer `launchctl bootstrap gui/$UID` login simulations loaded the agent but the unsigned debug executable stalled inside `dyld` before Porta setup; forcibly logging out the active Codex desktop would be disruptive and still would not represent the later bundled app. Re-run this check with the signed-or-ad-hoc `.app` produced by M6: enable Launch at Login, log out/in, then confirm the tray appears and an opted-in share becomes reachable.
- [~] **M5 signed LaunchAgent follow-up (2026-07-15):** `scripts/qa-login-startup.sh` mounts the release DMG, verifies its signature, backs up the current Porta store and login item, seeds an opted-in folder share, and exercises the registered LaunchAgent with `launchctl bootstrap gui/$UID`. The signed app remained running under launchd, auto-started its share to `live`, and returned the byte-exact fixture through its public Cloudflare URL; cleanup restored the original store and left zero Porta/cloudflared processes. This validates the same LaunchAgent path safely but does not replace the final literal log-out/log-in observation.
- [~] **M6 physical-device QA (2026-07-15):** three safe passes covered every automatable §9 boundary: 36 Rust tests exercise a 1,000-file listing under one second, Unicode paths, a sparse 2 GiB Safari-style Range request, wrong/correct/removed passwords, rename-without-URL-change, duplicate shares, active-download teardown, sleep/wake-style crash recovery policy, deleted-folder detection, and orphan cleanup; the in-app browser proved the real 1,000-row filter and empty state with zero console warnings; and the mounted ad-hoc DMG launched against an empty temporary home with no missing-permission crash or orphan. An actual iPhone/Safari scrub, a ten-minute Mac sleep/wake, and a separate fresh macOS account are unavailable in this active desktop environment. Run those three hardware/session checks before changing the M6 box to `[x]`.
- [~] **M3/M4 physical QR follow-up (2026-07-15):** `PORTA_PHONE_QA_HOLD_SECONDS=300 scripts/qa-login-startup.sh` now prints a temporary download URL, holds the signed app open, and requires a second unique visitor IP before passing. A Markdown-link trial was deliberately rejected after an immediate second IP indicated automated preview traffic. A second run exposed only a local QR image and stayed at one visitor for the full five minutes, proving no physical phone/cellular request arrived; it then timed out and restored the original Porta state with zero orphans. Re-run the command while a phone is available, scan the printed URL as a QR, and open it once on Wi-Fi and once on cellular.
- [~] **M6 Safari fixture follow-up (2026-07-15):** physical mode now creates a 60-second, iPhone-compatible H.264 test video and extends it with a valid sparse MP4 `free` atom to exactly 2,147,483,648 logical bytes while using about 4.3 MB on disk. `ffprobe` verifies the duration, codec, dimensions, pixel format, and exact size before Porta starts. The signed app then served a public `bytes 2147482624-2147483647/2147483648` response with exactly 1,024 bytes through Cloudflare, and the generated phone landing page embeds the video with a scrubber. This makes the remaining iPhone Safari observation turnkey but does not replace scanning the QR and physically dragging the scrubber.
