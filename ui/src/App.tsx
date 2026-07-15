import React, { useCallback, useEffect, useRef, useState } from "react";
import { ipc } from "./lib/ipc";
import type {
  CreateShareInput,
  ProviderProfile,
  SaveProviderProfileInput,
  Settings,
  Share,
  UpdateShareInput,
} from "./lib/ipc";
import { ShareCard } from "./components/ShareCard";
import { AddShareSheet } from "./components/AddShareSheet";
import { SettingsSheet } from "./components/SettingsSheet";
import { EmptyState } from "./components/EmptyState";
import { Logo, PlusIcon, GearIcon } from "./components/Icons";
import { appShortcutLabel, isWindows } from "./lib/platform";

type Sheet =
  | { kind: "none" }
  | { kind: "create"; path: string }
  | { kind: "edit"; share: Share }
  | { kind: "settings" };

interface Toast { id: number; text: string; }

export default function App() {
  const [shares, setShares] = useState<Share[] | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [profiles, setProfiles] = useState<ProviderProfile[] | null>(null);
  const [sheet, setSheet] = useState<Sheet>({ kind: "none" });
  const [dragging, setDragging] = useState(false);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const dragDepth = useRef(0);
  const toastId = useRef(0);

  const toast = useCallback((text: string) => {
    const id = ++toastId.current;
    setToasts((t) => [...t, { id, text }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 2400);
  }, []);

  // ---- initial load + backend events ----
  useEffect(() => {
    ipc.listShares().then(setShares);
    ipc.getSettings().then(setSettings);
    ipc.listProviderProfiles().then(setProfiles);
    return ipc.onEvent((e) => {
      setShares((prev) => {
        if (!prev) return prev;
        switch (e.type) {
          case "share_changed":
            return prev.some((s) => s.id === e.share.id)
              ? prev.map((s) => (s.id === e.share.id ? e.share : s))
              : [e.share, ...prev];
          case "share_removed":
            return prev.filter((s) => s.id !== e.id);
          case "stats_updated":
            return prev.map((s) => (s.id === e.id ? { ...s, stats: e.stats } : s));
        }
      });
    });
  }, []);

  // ---- theme override ----
  useEffect(() => {
    if (!settings) return;
    const root = document.documentElement;
    if (settings.theme === "system") root.removeAttribute("data-theme");
    else root.setAttribute("data-theme", settings.theme);
  }, [settings?.theme]);

  // ---- folder drop.
  // Browser/mock: HTML5 DnD below. Tauri: Rust forwards its native drag-drop
  // event as a `porta:folder-dropped` CustomEvent with detail = absolute path.
  useEffect(() => {
    const onDropped = (e: Event) => {
      const path = (e as CustomEvent<string>).detail;
      if (path) setSheet({ kind: "create", path });
      setDragging(false);
      dragDepth.current = 0;
    };
    const onHover = () => setDragging(true);
    const onCancel = () => { setDragging(false); dragDepth.current = 0; };
    window.addEventListener("porta:folder-dropped", onDropped);
    window.addEventListener("porta:drag-hover", onHover);
    window.addEventListener("porta:drag-cancel", onCancel);
    return () => {
      window.removeEventListener("porta:folder-dropped", onDropped);
      window.removeEventListener("porta:drag-hover", onHover);
      window.removeEventListener("porta:drag-cancel", onCancel);
    };
  }, []);

  // ---- keyboard: the platform's primary modifier + O picks a folder; Esc closes sheets ----
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "o") { e.preventDefault(); pickFolder(); }
      if (e.key === "Escape") setSheet({ kind: "none" });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // ---- actions ----
  const pickFolder = async () => {
    const path = await ipc.pickFolder();
    if (path) setSheet({ kind: "create", path });
  };

  const createShare = async (input: CreateShareInput) => {
    setSheet({ kind: "none" });
    const share = await ipc.createShare(input);
    setShares((prev) => (prev && !prev.some((s) => s.id === share.id) ? [share, ...prev] : prev));
    toast(`Sharing “${share.name}”`);
  };

  const saveShare = async (id: string, patch: UpdateShareInput) => {
    setSheet({ kind: "none" });
    await ipc.updateShare(id, patch);
    toast("Share updated");
  };

  const toggleShare = async (share: Share) => {
    if (share.status === "live" || share.status === "starting") {
      await ipc.stopShare(share.id);
    } else {
      await ipc.startShare(share.id);
    }
  };

  const deleteShare = async (share: Share) => {
    if (!window.confirm(`Remove “${share.name}”? The public link stops working immediately.`)) return;
    await ipc.deleteShare(share.id);
    toast("Share removed");
  };

  const copyUrl = async (url: string) => {
    try { await navigator.clipboard.writeText(url); } catch { /* WKWebView always allows */ }
    toast("Link copied to clipboard");
  };

  const changeSettings = async (patch: Partial<Settings>) => {
    const previous = settings;
    setSettings((current) => (current ? { ...current, ...patch } : current));
    try {
      const next = await ipc.updateSettings(patch);
      setSettings(next);
    } catch (error) {
      try {
        setSettings(await ipc.getSettings());
      } catch {
        setSettings(previous);
      }
      throw error;
    }
  };

  const saveProvider = async (input: SaveProviderProfileInput) => {
    try {
      const saved = await ipc.saveProviderProfile(input);
      setProfiles((current) => {
        if (!current) return [saved];
        return current.some((profile) => profile.id === saved.id)
          ? current.map((profile) => profile.id === saved.id ? saved : profile)
          : [...current, saved];
      });
      return saved;
    } catch (error) {
      try {
        setProfiles(await ipc.listProviderProfiles());
      } catch {
        // Keep the last readable list; reopening Settings retries the load.
      }
      throw error;
    }
  };

  const deleteProvider = async (id: string) => {
    await ipc.deleteProviderProfile(id);
    setProfiles((current) => current?.filter((profile) => profile.id !== id) ?? current);
  };

  // ---- HTML5 drag handlers (browser/mock path) ----
  const onDragEnter = (e: React.DragEvent) => {
    e.preventDefault();
    if (++dragDepth.current === 1) setDragging(true);
  };
  const onDragLeave = () => { if (--dragDepth.current <= 0) { setDragging(false); dragDepth.current = 0; } };
  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragging(false);
    dragDepth.current = 0;
    // Browsers can't give absolute paths; the mock fakes one for demo purposes.
    const item = e.dataTransfer.items[0];
    if (item) {
      const name = e.dataTransfer.files[0]?.name ?? "folder";
      setSheet({
        kind: "create",
        path: isWindows ? `C:\\Users\\you\\Dropped\\${name}` : `/Users/you/Dropped/${name}`,
      });
    }
  };

  return (
    <div className="app" onDragEnter={onDragEnter} onDragOver={(e) => e.preventDefault()} onDragLeave={onDragLeave} onDrop={onDrop}>
      <header className="titlebar">
        <div className="brand"><Logo /> Porta</div>
        <div className="actions">
          <button className="icon-btn" title={`Share a folder (${appShortcutLabel})`} onClick={pickFolder}><PlusIcon /></button>
          <button className="icon-btn" title="Settings" onClick={() => setSheet({ kind: "settings" })}><GearIcon /></button>
        </div>
      </header>

      <main className="content">
        {shares === null ? null : shares.length === 0 ? (
          <EmptyState onPickFolder={pickFolder} />
        ) : (
          <div className="share-list">
            {shares.map((s) => (
              <ShareCard
                key={s.id}
                share={s}
                providerName={profiles?.find((profile) => profile.id === (s.providerId ?? settings?.defaultProviderId))?.name ?? "Unknown provider"}
                onToggle={toggleShare}
                onCopy={copyUrl}
                onOpenUrl={(u) => ipc.openUrl(u)}
                onReveal={(p) => ipc.revealInFinder(p)}
                onEdit={(sh) => setSheet({ kind: "edit", share: sh })}
                onDelete={deleteShare}
              />
            ))}
          </div>
        )}
      </main>

      {dragging && <div className="drop-overlay">Drop to share this folder</div>}

      {sheet.kind === "create" && (
        <AddShareSheet
          mode="create"
          path={sheet.path}
          profiles={profiles ?? []}
          defaultProviderId={settings?.defaultProviderId ?? "cloudflare-quick"}
          onCreate={createShare}
          onSave={saveShare}
          onClose={() => setSheet({ kind: "none" })}
        />
      )}
      {sheet.kind === "edit" && (
        <AddShareSheet
          mode="edit"
          share={sheet.share}
          profiles={profiles ?? []}
          defaultProviderId={settings?.defaultProviderId ?? "cloudflare-quick"}
          onCreate={createShare}
          onSave={saveShare}
          onClose={() => setSheet({ kind: "none" })}
        />
      )}
      {sheet.kind === "settings" && settings && profiles && (
        <SettingsSheet
          settings={settings}
          profiles={profiles}
          onChange={changeSettings}
          onSaveProvider={saveProvider}
          onDeleteProvider={deleteProvider}
          onTestProvider={(id) => ipc.testProvider(id)}
          onPickProviderExecutable={() => ipc.pickProviderExecutable()}
          onClose={() => setSheet({ kind: "none" })}
        />
      )}

      <div className="toasts">
        {toasts.map((t) => <div key={t.id} className="toast">{t.text}</div>)}
      </div>
    </div>
  );
}
