import React, { useMemo, useState } from "react";
import type {
  ProviderKind,
  ProviderProfile,
  ProviderTestResult,
  SaveProviderProfileInput,
} from "../lib/ipc";

type EditableProviderKind = Exclude<ProviderKind, "cloudflareQuick">;

interface ProviderSettingsProps {
  profiles: ProviderProfile[];
  defaultProviderId: string;
  onDefaultChange: (id: string) => Promise<void>;
  onSave: (input: SaveProviderProfileInput) => Promise<ProviderProfile>;
  onDelete: (id: string) => Promise<void>;
  onTest: (id: string) => Promise<ProviderTestResult>;
  onPickExecutable: () => Promise<string | null>;
}

interface Draft {
  id?: string;
  name: string;
  kind: EditableProviderKind;
  executable: string;
  argumentsText: string;
  publicUrl: string;
  urlPattern: string;
  credentialEnv: string;
  forwardedIpHeader: string;
  localPort: string;
  credential: string;
  clearCredential: boolean;
  credentialConfigured: boolean;
}

const blankDraft = (): Draft => ({
  name: "",
  kind: "ngrok",
  executable: "",
  argumentsText: "{origin}",
  publicUrl: "",
  urlPattern: "(?P<url>https://[^\\s\"']+)",
  credentialEnv: "",
  forwardedIpHeader: "X-Forwarded-For",
  localPort: "",
  credential: "",
  clearCredential: false,
  credentialConfigured: false,
});

const draftFromProfile = (profile: ProviderProfile): Draft => ({
  id: profile.id,
  name: profile.name,
  kind: profile.kind as EditableProviderKind,
  executable: profile.executable ?? "",
  argumentsText: profile.arguments.join("\n"),
  publicUrl: profile.publicUrl ?? "",
  urlPattern: profile.urlPattern ?? "",
  credentialEnv: profile.credentialEnv ?? "",
  forwardedIpHeader: profile.forwardedIpHeader ?? "",
  localPort: profile.localPort?.toString() ?? "",
  credential: "",
  clearCredential: false,
  credentialConfigured: profile.credentialConfigured,
});

export const providerKindLabel = (kind: ProviderKind) => {
  switch (kind) {
    case "cloudflareQuick": return "Cloudflare Quick Tunnel";
    case "cloudflareManaged": return "Cloudflare managed tunnel";
    case "ngrok": return "ngrok";
    case "custom": return "Custom command";
  }
};

const errorText = (error: unknown) =>
  typeof error === "string" ? error : error instanceof Error ? error.message : "Porta couldn't finish that provider action. Try again.";

export function ProviderSettings(props: ProviderSettingsProps) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<{ tone: "success" | "error"; text: string } | null>(null);

  const defaultProfile = useMemo(
    () => props.profiles.find((profile) => profile.id === props.defaultProviderId),
    [props.defaultProviderId, props.profiles],
  );

  const update = <K extends keyof Draft>(key: K, value: Draft[K]) =>
    setDraft((current) => current ? { ...current, [key]: value } : current);

  const save = async () => {
    if (!draft) return;
    setBusy("save");
    setNotice(null);
    const input: SaveProviderProfileInput = {
      id: draft.id,
      name: draft.name,
      kind: draft.kind,
      executable: draft.executable || undefined,
      arguments: draft.argumentsText.split("\n").filter((argument) => argument.length > 0),
      publicUrl: draft.publicUrl || undefined,
      urlPattern: draft.urlPattern || undefined,
      credentialEnv: draft.credentialEnv || undefined,
      forwardedIpHeader: draft.forwardedIpHeader || undefined,
      localPort: draft.localPort ? Number(draft.localPort) : undefined,
      credential: draft.credential || undefined,
      clearCredential: draft.clearCredential,
    };
    try {
      const saved = await props.onSave(input);
      setDraft(draftFromProfile(saved));
      setNotice({ tone: "success", text: `${saved.name} saved.` });
    } catch (error) {
      setNotice({ tone: "error", text: errorText(error) });
    } finally {
      setBusy(null);
    }
  };

  const test = async (profile: ProviderProfile) => {
    setBusy(`test:${profile.id}`);
    setNotice(null);
    try {
      const result = await props.onTest(profile.id);
      setNotice({ tone: "success", text: `${result.message} ${result.url}` });
    } catch (error) {
      setNotice({ tone: "error", text: errorText(error) });
    } finally {
      setBusy(null);
    }
  };

  const remove = async (profile: ProviderProfile) => {
    if (!window.confirm(`Remove “${profile.name}”?`)) return;
    setBusy(`delete:${profile.id}`);
    setNotice(null);
    try {
      await props.onDelete(profile.id);
      if (draft?.id === profile.id) setDraft(null);
      setNotice({ tone: "success", text: `${profile.name} removed.` });
    } catch (error) {
      setNotice({ tone: "error", text: errorText(error) });
    } finally {
      setBusy(null);
    }
  };

  const chooseExecutable = async () => {
    const executable = await props.onPickExecutable();
    if (executable) update("executable", executable);
  };

  const needsCredential = draft?.kind === "cloudflareManaged"
    || draft?.kind === "ngrok"
    || (draft?.kind === "custom" && !!draft.credentialEnv);
  const missingCredential = !!draft && needsCredential
    && (!draft.credentialConfigured || draft.clearCredential)
    && !draft.credential;
  const numericLocalPort = draft?.localPort ? Number(draft.localPort) : undefined;
  const invalidLocalPort = numericLocalPort !== undefined
    && (!Number.isInteger(numericLocalPort) || numericLocalPort < 1 || numericLocalPort > 65535);
  const invalidDraft = !draft?.name.trim()
    || (draft.kind !== "cloudflareManaged" && !draft.executable)
    || (draft.kind === "cloudflareManaged" && (!draft.publicUrl || !draft.localPort))
    || (draft.kind === "custom" && !draft.publicUrl && !draft.urlPattern)
    || invalidLocalPort
    || missingCredential;

  return (
    <div className="provider-settings">
      <div className="field">
        <label htmlFor="default-provider">Default tunnel provider</label>
        <select
          id="default-provider"
          className="select select-wide"
          value={props.defaultProviderId}
          onChange={async (event) => {
            setNotice(null);
            try {
              await props.onDefaultChange(event.target.value);
            } catch (error) {
              setNotice({ tone: "error", text: errorText(error) });
            }
          }}
        >
          {props.profiles.map((profile) => (
            <option key={profile.id} value={profile.id} disabled={!profile.credentialConfigured}>
              {profile.name}{profile.credentialConfigured ? "" : " — setup required"}
            </option>
          ))}
        </select>
        <div className="field-hint">
          New shares inherit {defaultProfile?.name ?? "this provider"}; each share can override it.
        </div>
      </div>

      <div className="provider-list">
        {props.profiles.map((profile) => (
          <div className="provider-card" key={profile.id}>
            <div className="provider-card-copy">
              <div className="provider-card-title">
                {profile.name}
                <span className={`provider-state ${profile.credentialConfigured ? "ready" : "needs-setup"}`}>
                  {profile.credentialConfigured ? "Ready" : "Setup required"}
                </span>
              </div>
              <div className="provider-card-kind">{providerKindLabel(profile.kind)}</div>
            </div>
            <div className="provider-card-actions">
              <button className="btn btn-ghost" disabled={!!busy || !profile.credentialConfigured} onClick={() => test(profile)}>
                {busy === `test:${profile.id}` ? "Testing…" : "Test"}
              </button>
              {!profile.builtIn && (
                <>
                  <button className="btn btn-ghost" disabled={!!busy} onClick={() => {
                    setDraft(draftFromProfile(profile));
                    setNotice(null);
                  }}>Edit</button>
                  <button className="btn btn-ghost danger-text" disabled={!!busy} onClick={() => remove(profile)}>
                    Remove
                  </button>
                </>
              )}
            </div>
          </div>
        ))}
      </div>

      {!draft && (
        <button className="btn btn-secondary add-provider" onClick={() => {
          setDraft(blankDraft());
          setNotice(null);
        }}>Add provider profile</button>
      )}

      {draft && (
        <div className="provider-editor">
          <div className="provider-editor-head">
            <strong>{draft.id ? "Edit provider" : "Add provider"}</strong>
            <button className="btn btn-ghost" onClick={() => setDraft(null)}>Close</button>
          </div>

          <div className="field-grid">
            <div className="field">
              <label htmlFor="provider-name">Profile name</label>
              <input id="provider-name" className="text-input" value={draft.name} onChange={(event) => update("name", event.target.value)} placeholder="Work ngrok" />
            </div>
            <div className="field">
              <label htmlFor="provider-kind">Provider type</label>
              <select id="provider-kind" className="select select-wide" value={draft.kind} onChange={(event) => {
                const kind = event.target.value as EditableProviderKind;
                setDraft((current) => current ? {
                  ...current,
                  kind,
                  credential: "",
                  clearCredential: false,
                  credentialConfigured: current.kind === kind && current.credentialConfigured,
                } : current);
              }}>
                <option value="cloudflareManaged">Cloudflare managed</option>
                <option value="ngrok">ngrok</option>
                <option value="custom">Custom command</option>
              </select>
            </div>
          </div>

          {draft.kind !== "cloudflareManaged" && (
            <div className="field">
              <label htmlFor="provider-executable">Executable</label>
              <div className="input-action-row">
                <input id="provider-executable" className="text-input" value={draft.executable} onChange={(event) => update("executable", event.target.value)} placeholder="Choose the vendor CLI executable" />
                <button className="btn btn-secondary" onClick={chooseExecutable}>Choose…</button>
              </div>
            </div>
          )}

          {draft.kind === "cloudflareManaged" && (
            <>
              <div className="field">
                <label htmlFor="provider-public-url">Published application URL</label>
                <input id="provider-public-url" className="text-input" value={draft.publicUrl} onChange={(event) => update("publicUrl", event.target.value)} placeholder="https://share.example.com" />
              </div>
              <div className="field">
                <label htmlFor="provider-local-port">Dashboard route local port</label>
                <input id="provider-local-port" className="text-input" type="number" min="1" max="65535" value={draft.localPort} onChange={(event) => update("localPort", event.target.value)} placeholder="43123" />
                <div className="field-hint">Set the Cloudflare route service to http://localhost:&lt;this port&gt;. Use one managed profile per simultaneously active share.</div>
              </div>
            </>
          )}

          {draft.kind === "ngrok" && (
            <div className="field">
              <label htmlFor="provider-public-url">Reserved URL (optional)</label>
              <input id="provider-public-url" className="text-input" value={draft.publicUrl} onChange={(event) => update("publicUrl", event.target.value)} placeholder="https://your-name.ngrok.app" />
            </div>
          )}

          {draft.kind === "custom" && (
            <>
              <div className="field">
                <label htmlFor="provider-arguments">Arguments — one per line</label>
                <textarea id="provider-arguments" className="text-area code-input" value={draft.argumentsText} onChange={(event) => update("argumentsText", event.target.value)} />
                <div className="field-hint">Available placeholders: {"{origin}"}, {"{host}"}, and {"{port}"}. Arguments are passed directly without a shell.</div>
              </div>
              <div className="field">
                <label htmlFor="provider-url-pattern">URL or ready-message pattern</label>
                <input id="provider-url-pattern" className="text-input code-input" value={draft.urlPattern} onChange={(event) => update("urlPattern", event.target.value)} placeholder={`(?P<url>https://[^\\s"']+)`} />
              </div>
              <div className="field">
                <label htmlFor="provider-public-url">Fixed public URL (optional)</label>
                <input id="provider-public-url" className="text-input" value={draft.publicUrl} onChange={(event) => update("publicUrl", event.target.value)} placeholder="https://share.example.com" />
                <div className="field-hint">If the pattern matches a ready message instead of a URL, Porta uses this fixed URL.</div>
              </div>
              <div className="field-grid">
                <div className="field">
                  <label htmlFor="provider-env">Credential environment variable</label>
                  <input id="provider-env" className="text-input code-input" value={draft.credentialEnv} onChange={(event) => update("credentialEnv", event.target.value)} placeholder="VENDOR_TOKEN" />
                </div>
                <div className="field">
                  <label htmlFor="provider-header">Visitor IP header</label>
                  <input id="provider-header" className="text-input code-input" value={draft.forwardedIpHeader} onChange={(event) => update("forwardedIpHeader", event.target.value)} placeholder="X-Forwarded-For" />
                </div>
              </div>
              <div className="field">
                <label htmlFor="provider-local-port">Fixed local port (optional)</label>
                <input id="provider-local-port" className="text-input" type="number" min="1" max="65535" value={draft.localPort} onChange={(event) => update("localPort", event.target.value)} placeholder="OS-assigned" />
              </div>
            </>
          )}

          {needsCredential && (
            <div className="field">
              <label htmlFor="provider-credential">
                {draft.kind === "cloudflareManaged" ? "Tunnel token" : draft.kind === "ngrok" ? "Authtoken" : "Credential"}
              </label>
              <input
                id="provider-credential"
                className="text-input"
                type="password"
                value={draft.credential}
                onChange={(event) => setDraft((current) => current ? {
                  ...current,
                  credential: event.target.value,
                  clearCredential: false,
                } : current)}
                placeholder={draft.credentialConfigured ? "Stored securely — type to replace" : "Stored in your OS credential manager"}
              />
              {draft.credentialConfigured && (
                <label className="check-row">
                  <input type="checkbox" checked={draft.clearCredential} onChange={(event) => update("clearCredential", event.target.checked)} />
                  Remove the stored credential
                </label>
              )}
            </div>
          )}

          <div className="provider-editor-actions">
            <button className="btn btn-secondary" onClick={() => setDraft(null)}>Cancel</button>
            <button className="btn btn-primary" disabled={invalidDraft || !!busy} onClick={save}>
              {busy === "save" ? "Saving…" : "Save provider"}
            </button>
          </div>
        </div>
      )}

      {notice && <div className={`provider-notice ${notice.tone}`}>{notice.text}</div>}
    </div>
  );
}
