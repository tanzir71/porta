# Porta — Codex Handoff & Build Loop

**Porta** is a free, UI-first tunneling app for macOS. Users visually pick folders (or local ports) and get a public `https://` link instantly — like ngrok, but zero-config, zero-cost, and designed for non-terminal people.

This document is a **loopable work order**. Run it iteration after iteration until every checkbox in [§7 Milestones](#7-milestones--the-loop-state) is checked. §8 defines the loop protocol. The React UI is **already built and final** — your job is the Rust/Tauri backend and wiring.

---

## 1. Product principles (read every loop)

1. **Free forever.** No accounts, no servers we run, no paid APIs. Tunnels ride Cloudflare Quick Tunnels (`cloudflared`), which need no account and have no bandwidth cap.
2. **Grandma-simple.** Drag folder in → link on clipboard. Every error message must say what to do next, in plain words.
3. **The UI is the spec.** Everything visible in `ui/src` must work exactly as the components imply. Never simplify a feature because the backend is hard.
4. **Quiet resident app.** Menu-bar first: closing the window hides it; the app keeps serving. Quit only from the tray menu.
5. **Honest security.** Quick Tunnel URLs are unguessable but public. Password protection is our auth layer (basic-auth at the local server). Never claim end-to-end encryption.

**Non-goals (do not build):** custom domains, TCP tunnels, accounts/sync, Windows/Linux (structure code so ports are feasible, nothing more), analytics/telemetry of any kind.

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

**The gap Porta fills:** every free option is CLI-only; every GUI option costs money or needs an account. Nobody makes *folder sharing* (not port forwarding) the primary object. Porta = LocalCan's polish + cloudflared's free transport + Finder-native folder mental model. Password protection and visitor stats — paid features elsewhere — are free here because they live in our local server, not the tunnel provider.

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
│                    • basic-auth middleware (password from macOS Keychain) │
│                    • multipart upload handler (if allowUploads)           │
│                    • stats middleware (visitors/requests/bytes)           │
│    TunnelManager — spawns bundled `cloudflared tunnel --url http://…`     │
│                    parses public URL from stderr, supervises, restarts    │
│    Tray          — menu-bar icon, per-share quick toggles, Quit           │
│    Autostart     — tauri-plugin-autostart (launch at login)               │
└───────────────────────────────────────────────────────────────────────────┘
         folder share:  Browser → Cloudflare edge → cloudflared → axum → disk
         port share:    Browser → Cloudflare edge → cloudflared → localhost:PORT
```

**Key decisions (do not relitigate):**
- **Tauri v2**, stable channel. Bundle `cloudflared` (universal macOS binary) as a Tauri *sidecar* — never require the user to install anything.
- One axum server per active folder share, bound to `127.0.0.1:0` (OS-assigned port). Port shares skip axum; cloudflared points at the user's port directly.
- URL parsing: cloudflared prints `https://<random>.trycloudflare.com` on stderr within ~5 s. Regex `https://[a-z0-9-]+\.trycloudflare\.com`. Timeout 30 s → status `error` with message "Couldn't reach Cloudflare — check your internet connection and try again."
- Supervision: if cloudflared exits while a share is `live`, auto-restart with backoff 1 s → 2 s → 4 s… (max 60 s), keep status `live` unless 3 consecutive failures → `error`: "The tunnel keeps dropping. Porta will retry when you toggle it back on."
- Passwords: store in macOS Keychain (`security` via `keyring` crate), never in the JSON store. JSON keeps only `passwordProtected: bool`.
- Stats: middleware counts requests/bytes; "visitors" = unique `Cf-Connecting-Ip` header values since start (HashSet, reset on start).
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
│       └── components/{Icons,ShareCard,AddShareSheet,SettingsSheet,EmptyState}.tsx
├── server-templates/
│   └── listing.html         ← visitor-facing directory page (embed via include_str!)
└── src-tauri/               ← YOU CREATE THIS (tauri init, then implement)
    ├── Cargo.toml  tauri.conf.json  capabilities/
    ├── binaries/cloudflared-aarch64-apple-darwin (+x86_64)  ← sidecar
    ├── icons/
    └── src/{main,shares,server,tunnel,tray,settings,stats}.rs
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

**Events:** emit `app_event` (payload = `AppEvent` union) on every status/url change, removal, and a stats tick **at most once per second** per share.

**Drag & drop wiring (backend → UI):** enable Tauri's drag-drop; on `Enter/Over` run `window.dispatchEvent(new CustomEvent('porta:drag-hover'))` (via `eval`), on `Leave` → `'porta:drag-cancel'`, on `Drop` of a **directory** → `'porta:folder-dropped'` with `detail` = absolute path (first dir only; ignore plain files but toast via the same mechanism is not required). The UI already listens for these three events.

**Settings semantics:**
- `launchAtLogin` → tauri-plugin-autostart register/unregister.
- `autoStartShares` → on app launch, start every share with `autoStart:true`.
- `showDockIcon` → `app.set_activation_policy(Regular|Accessory)` live-switchable.
- `copyUrlOnStart` → when a share transitions to `live`, write URL to clipboard (tauri-plugin-clipboard-manager).
- `notifyOnFirstVisitor` → native notification on a share's first unique visitor (tauri-plugin-notification): title = share name, body = "Someone just opened your link."

---

## 6. Hard rules (read every loop)

1. **Never modify** `ui/src/components/*`, `ui/src/App.tsx`, or `ui/src/styles.css` — the design is final. Allowed in `ui/`: bugfix-level changes to `ipc.ts` *only if* both sides are updated in the same commit and the change is additive.
2. Every command returns `Result<T, String>`; error strings are user-facing — plain English, actionable, no Rust debug noise.
3. Window close = hide (keep serving). Real quit only from tray. Quitting stops all tunnels cleanly (SIGTERM to cloudflared, then wait ≤3 s).
4. Serve **only** within the shared folder — canonicalize every request path and reject traversal (`..`, symlinks escaping the root) with 404.
5. Uploads: only when `allowUploads`; reject files > 2 GB; never overwrite — collide as `name (2).ext`.
6. No telemetry, no network calls except cloudflared itself.
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
- [ ] Path traversal blocked (test with `curl --path-as-is /a/../../etc/passwd` → 404)
- [ ] basic-auth middleware when password set (realm "Porta"); password stored/read via Keychain
- [ ] uploads: multipart POST to current dir when `allowUploads`; collision-safe; 303 redirect back
- [ ] Verify: integration test hits a temp dir share: listing renders, file downloads byte-identical, traversal 404s, wrong password 401s

### M3 — Tunnel lifecycle
- [ ] TunnelManager spawns sidecar `cloudflared tunnel --url http://127.0.0.1:{port} --no-autoupdate`, parses URL ≤30 s, transitions `starting→live` with URL via event
- [ ] `stop_share` tears down child processes — zero orphan `cloudflared` after quit (verify `pgrep cloudflared`)
- [ ] Crash supervision with backoff per §3; 3 strikes → `error` + friendly message
- [ ] No-internet path: unreachable → `error` "Couldn't reach Cloudflare — check your internet connection and try again."
- [ ] Port shares (`kind:"port"`) tunnel directly to the user's port
- [ ] `copyUrlOnStart` honored
- [ ] Verify: toggle a real folder share; open the trycloudflare URL from a phone; download a file

### M4 — Stats + notifications
- [ ] Stats middleware: requests, bytes, unique visitors via `Cf-Connecting-Ip`; `stats_updated` event ≤1/s per share
- [ ] `notifyOnFirstVisitor` fires native notification once per session per share
- [ ] Verify: hit share from two IPs (phone on cellular) → visitors=2 in UI

### M5 — Resident app: tray, login, drag-drop
- [ ] Tray icon (template image, correct dark-mode) with menu: per-share rows "● Client Mockups — Copy link / Turn off", "Share a folder…", "Open Porta", separator, "Quit Porta"
- [ ] Tray icon state: idle vs ≥1 live share (badge/filled variant)
- [ ] Window close hides; app keeps serving; dock icon policy follows `showDockIcon` live
- [ ] `launchAtLogin` via tauri-plugin-autostart verified in System Settings › Login Items
- [ ] `autoStartShares` on launch (only when master switch on)
- [ ] Native drag-drop wired to the three `porta:*` CustomEvents; dropping a real folder from Finder opens the create sheet with the correct absolute path
- [ ] Single-instance plugin: second launch focuses existing window
- [ ] Verify: log out/in → Porta running in menu bar, auto-start shares live, links work

### M6 — Polish & hardening
- [ ] All error strings audited against §6.2 (no `Error:`, no paths-only, always an action)
- [ ] Share names sanitized in listing.html (HTML-escape everything server-rendered)
- [ ] App icon (folder+beam motif matching `Logo` in `Icons.tsx`), 1024px master, iconset generated
- [ ] `cargo tauri build` produces signed-or-ad-hoc .dmg that launches on a clean machine
- [ ] README.md: what it is, screenshot placeholder, honest security note (public-but-unguessable URLs, password option, Cloudflare ToS: not for production hosting)
- [ ] Full QA pass: §9 checklist all green

### M7 — Release
- [ ] Version 1.0.0 tagged; CHANGELOG.md written
- [ ] .dmg < 25 MB (cloudflared included)
- [ ] Zero clippy warnings, zero TS errors (`npm run build`), zero orphan processes after 10 start/stop cycles (scripted)

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
- Quit from tray → `pgrep cloudflared` empty
- Fresh macOS user account → app launches, no missing-permission crashes

## 10. Blockers (append-only)

*(empty)*
