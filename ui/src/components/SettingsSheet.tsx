import React from "react";
import type {
  ProviderProfile,
  ProviderTestResult,
  SaveProviderProfileInput,
  Settings,
} from "../lib/ipc";
import { isWindows } from "../lib/platform";
import { ProviderSettings } from "./ProviderSettings";

export interface SettingsSheetProps {
  settings: Settings;
  onChange: (patch: Partial<Settings>) => Promise<void>;
  profiles: ProviderProfile[];
  onSaveProvider: (input: SaveProviderProfileInput) => Promise<ProviderProfile>;
  onDeleteProvider: (id: string) => Promise<void>;
  onTestProvider: (id: string) => Promise<ProviderTestResult>;
  onPickProviderExecutable: () => Promise<string | null>;
  onClose: () => void;
  version?: string;
}

interface RowProps {
  title: string;
  desc: string;
  checked: boolean;
  onToggle: () => void;
}

const Row = ({ title, desc, checked, onToggle }: RowProps) => (
  <div className="opt-row">
    <div>
      <div className="o-title">{title}</div>
      <div className="o-desc">{desc}</div>
    </div>
    <button className="switch" role="switch" aria-checked={checked} aria-label={title} onClick={onToggle} />
  </div>
);

export function SettingsSheet({
  settings,
  onChange,
  profiles,
  onSaveProvider,
  onDeleteProvider,
  onTestProvider,
  onPickProviderExecutable,
  onClose,
  version = "1.2.0",
}: SettingsSheetProps) {
  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet" role="dialog" aria-modal="true" aria-label="Settings">
        <h3>Settings</h3>
        <p className="sub">Porta {version} · free forever, bring your preferred tunnel provider</p>

        <div className="section-label">Startup</div>
        <Row
          title="Launch Porta at login"
          desc={isWindows
            ? "Porta opens quietly in the notification area when you sign in."
            : "Porta opens quietly in the menu bar when you sign in."}
          checked={settings.launchAtLogin}
          onToggle={() => onChange({ launchAtLogin: !settings.launchAtLogin })}
        />
        <Row
          title="Resume shares automatically"
          desc="Shares marked “start automatically” go live on launch."
          checked={settings.autoStartShares}
          onToggle={() => onChange({ autoStartShares: !settings.autoStartShares })}
        />
        <Row
          title={isWindows ? "Show taskbar icon" : "Show Dock icon"}
          desc={isWindows
            ? "Off = Porta lives in the notification area only."
            : "Off = Porta lives in the menu bar only."}
          checked={settings.showDockIcon}
          onToggle={() => onChange({ showDockIcon: !settings.showDockIcon })}
        />

        <div className="section-label">Tunnel providers</div>
        <ProviderSettings
          profiles={profiles}
          defaultProviderId={settings.defaultProviderId}
          onDefaultChange={(defaultProviderId) => onChange({ defaultProviderId })}
          onSave={onSaveProvider}
          onDelete={onDeleteProvider}
          onTest={onTestProvider}
          onPickExecutable={onPickProviderExecutable}
        />

        <div className="section-label">Sharing</div>
        <Row
          title="Copy link when a share goes live"
          desc="The public URL lands on your clipboard, ready to paste."
          checked={settings.copyUrlOnStart}
          onToggle={() => onChange({ copyUrlOnStart: !settings.copyUrlOnStart })}
        />
        <Row
          title="Notify on first visitor"
          desc="A notification when someone opens your link."
          checked={settings.notifyOnFirstVisitor}
          onToggle={() => onChange({ notifyOnFirstVisitor: !settings.notifyOnFirstVisitor })}
        />

        <div className="section-label">Appearance</div>
        <div className="opt-row">
          <div>
            <div className="o-title">Theme</div>
            <div className="o-desc">Follows your system by default.</div>
          </div>
          <select
            className="select"
            value={settings.theme}
            aria-label="Theme"
            onChange={(e) => onChange({ theme: e.target.value as Settings["theme"] })}
          >
            <option value="system">System</option>
            <option value="light">Light</option>
            <option value="dark">Dark</option>
          </select>
        </div>

        <div className="foot">
          <button className="btn btn-primary" onClick={onClose}>Done</button>
        </div>
      </div>
    </div>
  );
}
