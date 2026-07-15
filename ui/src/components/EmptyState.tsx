import React from "react";
import { Logo, PlusIcon } from "./Icons";

export function EmptyState({ onPickFolder }: { onPickFolder: () => void }) {
  return (
    <div className="empty">
      <div className="art"><Logo size={40} /></div>
      <h2>Share a folder with anyone</h2>
      <p>
        Drop a folder here and Porta gives you a secure public link.
        No account, no limits, completely free.
      </p>
      <button className="btn btn-primary" onClick={onPickFolder}>
        <PlusIcon /> Choose a folder…
      </button>
      <div className="hint">or drag any folder into this window · <kbd>⌘</kbd><kbd>O</kbd></div>
    </div>
  );
}
