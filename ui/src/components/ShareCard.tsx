import React, { useState } from "react";
import type { Share } from "../lib/ipc";
import {
  FolderIcon, GlobeIcon, CopyIcon, CheckIcon, LockIcon,
  TrashIcon, PencilIcon, EyeIcon, ArrowUpDownIcon, AlertIcon,
} from "./Icons";
import { openFolderLabel } from "../lib/platform";

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1048576) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1073741824) return `${(n / 1048576).toFixed(1)} MB`;
  return `${(n / 1073741824).toFixed(2)} GB`;
}

const STATUS_LABEL: Record<Share["status"], string> = {
  live: "Live",
  starting: "Starting…",
  stopped: "Off",
  error: "Error",
};

export interface ShareCardProps {
  share: Share;
  onToggle: (share: Share) => void;
  onCopy: (url: string) => void;
  onOpenUrl: (url: string) => void;
  onReveal: (path: string) => void;
  onEdit: (share: Share) => void;
  onDelete: (share: Share) => void;
}

export function ShareCard({ share, onToggle, onCopy, onOpenUrl, onReveal, onEdit, onDelete }: ShareCardProps) {
  const [copied, setCopied] = useState(false);
  const live = share.status === "live";
  const busy = share.status === "starting";

  const copy = () => {
    if (!share.url) return;
    onCopy(share.url);
    setCopied(true);
    setTimeout(() => setCopied(false), 1600);
  };

  return (
    <div className="share-card">
      <div className="row-top">
        <div className={`share-icon ${live ? "live" : ""}`}>
          {share.kind === "folder" ? <FolderIcon /> : <GlobeIcon />}
        </div>

        <div className="share-meta">
          <div className="share-name">
            {share.name}
            <span className={`pill pill-${share.status}`}>
              <span className="dot" />
              {STATUS_LABEL[share.status]}
            </span>
          </div>
          <div
            className="share-path"
            title={share.kind === "folder" ? `${openFolderLabel}: ${share.path}` : undefined}
            onClick={() => share.path && onReveal(share.path)}
          >
            {share.kind === "folder" ? share.path : `localhost:${share.port}`}
          </div>
        </div>

        <button
          className="switch"
          role="switch"
          aria-checked={live || busy}
          aria-label={live || busy ? "Stop sharing" : "Start sharing"}
          disabled={busy}
          onClick={() => onToggle(share)}
        />
      </div>

      <div className="url-row">
        {share.url ? (
          <div className="url-box">
            {share.passwordProtected && (
              <span className="lock" title="Password protected"><LockIcon /></span>
            )}
            <span className="url-text" title="Open in browser" onClick={() => onOpenUrl(share.url!)}>
              {share.url.replace(/^https:\/\//, "")}
            </span>
          </div>
        ) : (
          <div className="url-box placeholder">
            {busy ? "Requesting public link…" : "Turn on to get a public link"}
          </div>
        )}
        <button className={`copy-btn ${copied ? "copied" : ""}`} onClick={copy} disabled={!share.url}>
          {copied ? <CheckIcon /> : <CopyIcon />}
          {copied ? "Copied" : "Copy link"}
        </button>
      </div>

      {share.status === "error" && share.error && (
        <div className="err-note"><AlertIcon /> {share.error}</div>
      )}

      <div className="share-foot">
        <div className="share-stats">
          <span title="Unique visitors"><EyeIcon /> {share.stats.visitors}</span>
          <span title="Data served"><ArrowUpDownIcon /> {fmtBytes(share.stats.bytesServed)}</span>
          {share.autoStart && <span title="Starts when Porta launches">auto-starts</span>}
        </div>
        <div className="share-actions">
          <button className="btn-ghost icon-btn" title="Edit share" onClick={() => onEdit(share)}>
            <PencilIcon />
          </button>
          <button className="btn-ghost icon-btn" title="Remove share" onClick={() => onDelete(share)}>
            <TrashIcon />
          </button>
        </div>
      </div>
    </div>
  );
}
