#!/bin/sh

set -eu

repo_root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
dmg=${1:-"$repo_root/src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/Porta_1.1.0_aarch64.dmg"}
uid=$(id -u)
store_dir="$HOME/Library/Application Support/com.porta.app"
store="$store_dir/store.json"
plist="$HOME/Library/LaunchAgents/Porta.plist"
qa_root=$(mktemp -d /tmp/porta-login-qa.XXXXXX)
backup="$qa_root/backup"
mount_point=""
label=""
app_pid=""
had_store=false
had_plist=false

mkdir -p "$backup"

if [ -f "$store" ]; then
  ditto "$store" "$backup/store.json"
  had_store=true
fi
if [ -f "$plist" ]; then
  ditto "$plist" "$backup/Porta.plist"
  had_plist=true
fi

stop_pid() {
  pid=$1
  if ! kill -0 "$pid" 2>/dev/null; then
    return
  fi
  kill -TERM "$pid" 2>/dev/null || true
  attempts=0
  while kill -0 "$pid" 2>/dev/null && [ "$attempts" -lt 30 ]; do
    sleep 0.1
    attempts=$((attempts + 1))
  done
  if kill -0 "$pid" 2>/dev/null; then
    kill -KILL "$pid" 2>/dev/null || true
  fi
}

finish() {
  status=$?
  trap - EXIT INT TERM

  if [ -n "$label" ]; then
    launchctl bootout "gui/$uid/$label" >/dev/null 2>&1 || true
  fi
  if [ -n "$app_pid" ]; then
    stop_pid "$app_pid"
  fi
  if [ -d "$qa_root/Porta.app" ]; then
    qa_executable=$(realpath "$qa_root/Porta.app/Contents/MacOS/porta")
    for pid in $(pgrep -f "$qa_executable" 2>/dev/null || true); do
      stop_pid "$pid"
    done
    qa_app=$(realpath "$qa_root/Porta.app")
    for pid in $(pgrep -f "$qa_app/Contents/MacOS/cloudflared" 2>/dev/null || true); do
      stop_pid "$pid"
    done
  fi

  if $had_store; then
    mkdir -p "$store_dir"
    ditto "$backup/store.json" "$store"
  else
    rm -f "$store"
  fi
  if $had_plist; then
    mkdir -p "$(dirname "$plist")"
    ditto "$backup/Porta.plist" "$plist"
  else
    rm -f "$plist"
  fi
  if [ -n "$mount_point" ]; then
    hdiutil detach "$mount_point" -quiet >/dev/null 2>&1 || true
  fi
  rm -rf "$qa_root"

  if pgrep -x porta >/dev/null || pgrep -x cloudflared >/dev/null; then
    echo "Login QA left a Porta or cloudflared process running." >&2
    pgrep -fl 'porta|cloudflared' >&2 || true
    status=1
  fi
  exit "$status"
}
trap finish EXIT INT TERM

if [ ! -f "$dmg" ]; then
  echo "Build the Porta disk image before running login QA: $dmg" >&2
  exit 1
fi
if pgrep -x porta >/dev/null || pgrep -x cloudflared >/dev/null; then
  echo "Quit Porta and cloudflared before running login QA." >&2
  exit 1
fi

phone_hold=${PORTA_PHONE_QA_HOLD_SECONDS:-0}
case "$phone_hold" in
  '' | *[!0-9]*)
    echo "PORTA_PHONE_QA_HOLD_SECONDS must be a whole number of seconds." >&2
    exit 1
    ;;
esac

attach_output=$(hdiutil attach "$dmg" -nobrowse -readonly -plist)
mount_point=$(printf '%s' "$attach_output" | plutil -extract system-entities xml1 -o - - | \
  plutil -convert json -o - - | jq -r '.[] | select(."mount-point" != null) | ."mount-point"' | head -n 1)
if [ -z "$mount_point" ] || [ ! -d "$mount_point/Porta.app" ]; then
  echo "The disk image did not contain Porta.app." >&2
  exit 1
fi

ditto "$mount_point/Porta.app" "$qa_root/Porta.app"
hdiutil detach "$mount_point" -quiet
mount_point=""
codesign --verify --deep --strict "$qa_root/Porta.app"

mkdir -p "$qa_root/Shared Folder" "$store_dir"
printf 'Porta signed login startup fixture\n' >"$qa_root/Shared Folder/fixture.txt"
printf 'Porta phone download fixture\n' >"$qa_root/Shared Folder/porta-phone-download.bin"
if [ "$phone_hold" -gt 0 ]; then
  for command in ffmpeg ffprobe perl truncate; do
    if ! command -v "$command" >/dev/null; then
      echo "Install $command before running the physical phone QA fixture." >&2
      exit 1
    fi
  done

  video="$qa_root/Shared Folder/porta-safari-2gb.mp4"
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i 'testsrc2=size=640x360:rate=15:duration=60' \
    -an -c:v libx264 -preset ultrafast -crf 34 -profile:v baseline -level 3.0 \
    -pix_fmt yuv420p -g 15 -movflags +faststart "$video"
  original_size=$(stat -f %z "$video")
  target_size=2147483648
  free_size=$((target_size - original_size))
  perl -e 'print pack("Na4Q>", 1, "free", $ARGV[0])' "$free_size" >>"$video"
  truncate -s "$target_size" "$video"

  video_size=$(ffprobe -v error -show_entries format=size -of default=nw=1:nk=1 "$video")
  video_duration=$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$video")
  if [ "$video_size" != "$target_size" ] || [ "$video_duration" != "60.000000" ]; then
    echo "The Safari QA video was not created as a playable 2 GiB fixture." >&2
    exit 1
  fi

  printf '%s\n' \
    '<!doctype html>' \
    '<meta charset="utf-8">' \
    '<meta name="viewport" content="width=device-width, initial-scale=1">' \
    '<title>Porta phone QA</title>' \
    '<style>body{font:17px system-ui;margin:24px;line-height:1.45}a{display:inline-block;padding:12px 16px;background:#3451b2;color:white;border-radius:10px;text-decoration:none}video{display:block;width:100%;max-width:640px;margin-top:24px;background:#111}</style>' \
    '<h1>Porta phone QA</h1>' \
    '<ol><li>On Wi-Fi, download the fixture.</li><li>Play the video and drag its scrubber to several positions.</li><li>Turn Wi-Fi off, reload this page over cellular, and download the fixture again.</li></ol>' \
    '<a href="porta-phone-download.bin" download>Download fixture</a>' \
    '<video controls playsinline preload="metadata" src="porta-safari-2gb.mp4"></video>' \
    '<p>The H.264 video has a 60-second timeline and an exact logical size of 2 GiB.</p>' \
    >"$qa_root/Shared Folder/porta-phone-qa.html"
fi
share_id=$(uuidgen | tr '[:upper:]' '[:lower:]')
created_at=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
jq -n \
  --arg id "$share_id" \
  --arg path "$qa_root/Shared Folder" \
  --arg createdAt "$created_at" \
  '{
    version: 1,
    shares: [{
      id: $id,
      kind: "folder",
      name: "Signed Login QA",
      path: $path,
      port: null,
      url: null,
      status: "stopped",
      error: null,
      passwordProtected: false,
      showListing: true,
      allowUploads: false,
      autoStart: true,
      stats: { visitors: 0, requests: 0, bytesServed: 0 },
      createdAt: $createdAt
    }],
    settings: {
      launchAtLogin: true,
      autoStartShares: true,
      showDockIcon: false,
      notifyOnFirstVisitor: false,
      copyUrlOnStart: false,
      theme: "system"
    }
  }' >"$store"

qa_executable="$qa_root/Porta.app/Contents/MacOS/porta"
qa_executable=$(realpath "$qa_executable")
"$qa_executable" >"$qa_root/direct.log" 2>&1 &
app_pid=$!

attempts=0
while [ ! -f "$plist" ] && [ "$attempts" -lt 100 ]; do
  sleep 0.1
  attempts=$((attempts + 1))
done
if [ ! -f "$plist" ]; then
  echo "The signed app did not register its LaunchAgent." >&2
  exit 1
fi

label=$(plutil -extract Label raw "$plist")
registered_executable=$(plutil -extract ProgramArguments.0 raw "$plist")
registered_executable=$(realpath "$registered_executable")
if [ "$registered_executable" != "$qa_executable" ]; then
  echo "The LaunchAgent registered the wrong executable: $registered_executable" >&2
  exit 1
fi

stop_pid "$app_pid"
wait "$app_pid" 2>/dev/null || true
app_pid=""
launchctl bootout "gui/$uid/$label" >/dev/null 2>&1 || true
sleep 1
launchctl bootstrap "gui/$uid" "$plist"

attempts=0
login_pid=""
while [ "$attempts" -lt 200 ]; do
  candidate_pid=$(launchctl print "gui/$uid/$label" 2>/dev/null | awk '/pid =/ { print $3; exit }')
  if [ -n "$candidate_pid" ] && kill -0 "$candidate_pid" 2>/dev/null; then
    login_pid=$candidate_pid
    break
  fi
  sleep 0.1
  attempts=$((attempts + 1))
done
if [ -z "$login_pid" ] || ! kill -0 "$login_pid" 2>/dev/null; then
  echo "launchd did not keep the signed Porta app running." >&2
  launchctl print "gui/$uid/$label" >&2 || true
  exit 1
fi
app_pid=$login_pid

attempts=0
url=""
while [ -z "$url" ] && [ "$attempts" -lt 120 ]; do
  url=$(jq -r --arg id "$share_id" '.shares[] | select(.id == $id and .status == "live") | .url // empty' "$store")
  if [ -z "$url" ]; then
    sleep 0.5
  fi
  attempts=$((attempts + 1))
done
if [ -z "$url" ]; then
  status=$(jq -r --arg id "$share_id" '.shares[] | select(.id == $id) | [.status, (.error // "")] | @tsv' "$store")
  echo "The login-started share did not become live: $status" >&2
  exit 1
fi

host=${url#https://}
edge_ip=""
if ! download=$(curl --fail --silent --show-error --retry 2 --retry-all-errors --max-time 20 "$url/fixture.txt"); then
  attempts=0
  while [ -z "$edge_ip" ] && [ "$attempts" -lt 20 ]; do
    edge_ip=$(dig +short @1.1.1.1 A "$host" | head -n 1)
    if [ -z "$edge_ip" ]; then
      sleep 1
    fi
    attempts=$((attempts + 1))
  done
  if [ -z "$edge_ip" ]; then
    echo "The public link did not resolve through either the system or 1.1.1.1 DNS." >&2
    exit 1
  fi
  download=$(curl --fail --silent --show-error --retry 2 --retry-all-errors --max-time 20 \
    --resolve "$host:443:$edge_ip" "$url/fixture.txt")
fi
if [ "$download" != "Porta signed login startup fixture" ]; then
  echo "The login-started public link returned the wrong file." >&2
  exit 1
fi

attempts=0
visitors=0
while [ "$visitors" -lt 1 ] && [ "$attempts" -lt 20 ]; do
  visitors=$(jq -r --arg id "$share_id" '.shares[] | select(.id == $id) | .stats.visitors' "$store")
  if [ "$visitors" -lt 1 ]; then
    sleep 0.1
  fi
  attempts=$((attempts + 1))
done
if [ "$visitors" -lt 1 ]; then
  echo "The public download did not reach Porta's visitor stats." >&2
  exit 1
fi

if [ "$phone_hold" -gt 0 ]; then
  range_start=2147482624
  range_url="$url/porta-safari-2gb.mp4"
  if [ -n "$edge_ip" ]; then
    curl --fail --silent --show-error --max-time 20 \
      --resolve "$host:443:$edge_ip" --range "$range_start-" \
      --max-filesize 2048 \
      --dump-header "$qa_root/range.headers" --output "$qa_root/range.bin" "$range_url"
  else
    curl --fail --silent --show-error --max-time 20 --range "$range_start-" --max-filesize 2048 \
      --dump-header "$qa_root/range.headers" --output "$qa_root/range.bin" "$range_url"
  fi
  range_bytes=$(wc -c <"$qa_root/range.bin" | tr -d ' ')
  if [ "$range_bytes" != "1024" ] || ! tr -d '\r' <"$qa_root/range.headers" | \
    grep -Fqix "content-range: bytes $range_start-2147483647/2147483648"; then
    echo "The public 2 GiB video did not return the expected end-of-file Range response." >&2
    exit 1
  fi

  echo "PORTA_PHONE_QA_URL=$url/porta-phone-qa.html"
  echo "PORTA_SAFARI_QA_URL=$range_url"
  deadline=$(($(date +%s) + phone_hold))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    visitors=$(jq -r --arg id "$share_id" '.shares[] | select(.id == $id) | .stats.visitors' "$store")
    if [ "$visitors" -ge 2 ]; then
      echo "Phone QA passed: public download recorded a second visitor IP."
      break
    fi
    sleep 1
  done
  if [ "$visitors" -lt 2 ]; then
    echo "Phone QA timed out before a second visitor IP reached Porta." >&2
    exit 1
  fi
fi
echo "Signed login QA passed: LaunchAgent=$label pid=$login_pid status=live public_download=ok visitors=$visitors"
