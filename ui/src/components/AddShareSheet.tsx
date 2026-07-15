import React, { useState } from "react";
import type { CreateShareInput, Share, UpdateShareInput } from "../lib/ipc";
import { FolderIcon } from "./Icons";
import { folderBasename } from "../lib/platform";

/**
 * One sheet, two modes:
 *  - create: shown right after a folder is picked/dropped (path prefilled)
 *  - edit:   shown from a ShareCard's pencil button
 */
export interface AddShareSheetProps {
  mode: "create" | "edit";
  path?: string; // create mode
  share?: Share; // edit mode
  onCreate: (input: CreateShareInput) => void;
  onSave: (id: string, patch: UpdateShareInput) => void;
  onClose: () => void;
}

export function AddShareSheet(props: AddShareSheetProps) {
  const editing = props.mode === "edit" ? props.share! : null;
  const path = editing?.path ?? props.path ?? "";
  const basename = folderBasename(path);

  const [name, setName] = useState(editing?.name ?? basename);
  const [usePassword, setUsePassword] = useState(editing?.passwordProtected ?? false);
  const [password, setPassword] = useState("");
  const [showListing, setShowListing] = useState(editing?.showListing ?? true);
  const [allowUploads, setAllowUploads] = useState(editing?.allowUploads ?? false);
  const [autoStart, setAutoStart] = useState(editing?.autoStart ?? false);

  const passwordInvalid = usePassword && !editing?.passwordProtected && password.length === 0;

  const submit = () => {
    if (passwordInvalid) return;
    if (editing) {
      const patch: UpdateShareInput = { name, showListing, allowUploads, autoStart };
      if (!usePassword) patch.password = null;
      else if (password) patch.password = password;
      props.onSave(editing.id, patch);
    } else {
      props.onCreate({
        kind: "folder",
        path,
        name,
        password: usePassword ? password : undefined,
        showListing,
        allowUploads,
        autoStart,
        startNow: true,
      });
    }
  };

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && props.onClose()}>
      <div className="sheet" role="dialog" aria-modal="true" aria-label={editing ? "Edit share" : "Share a folder"}>
        <h3>{editing ? "Edit share" : "Share this folder"}</h3>
        <p className="sub">
          {editing
            ? "Changes apply immediately. A live share restarts with the same link flow."
            : "Anyone with the link can browse and download these files while sharing is on."}
        </p>

        <div className="picked-folder">
          <div className="share-icon"><FolderIcon /></div>
          <div className="meta">
            <div className="p-name">{basename}</div>
            <div className="p-path">{path}</div>
          </div>
        </div>

        <div className="field">
          <label htmlFor="share-name">Display name</label>
          <input
            id="share-name"
            className="text-input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={basename}
            autoFocus
          />
        </div>

        <div className="opt-row">
          <div>
            <div className="o-title">Require a password</div>
            <div className="o-desc">Visitors must enter it before seeing anything.</div>
          </div>
          <button
            className="switch" role="switch" aria-checked={usePassword}
            aria-label="Require a password"
            onClick={() => setUsePassword(!usePassword)}
          />
        </div>
        {usePassword && (
          <div className="field" style={{ marginTop: 10 }}>
            <input
              className="text-input"
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder={editing?.passwordProtected ? "Unchanged — type to replace" : "Choose a password"}
              aria-label="Share password"
            />
          </div>
        )}

        <div className="opt-row">
          <div>
            <div className="o-title">Show file listing</div>
            <div className="o-desc">Off = serve index.html only, like a website.</div>
          </div>
          <button
            className="switch" role="switch" aria-checked={showListing}
            aria-label="Show file listing"
            onClick={() => setShowListing(!showListing)}
          />
        </div>

        <div className="opt-row">
          <div>
            <div className="o-title">Allow uploads</div>
            <div className="o-desc">Visitors can drop files into this folder.</div>
          </div>
          <button
            className="switch" role="switch" aria-checked={allowUploads}
            aria-label="Allow uploads"
            onClick={() => setAllowUploads(!allowUploads)}
          />
        </div>

        <div className="opt-row">
          <div>
            <div className="o-title">Start automatically</div>
            <div className="o-desc">Goes live whenever Porta launches.</div>
          </div>
          <button
            className="switch" role="switch" aria-checked={autoStart}
            aria-label="Start automatically"
            onClick={() => setAutoStart(!autoStart)}
          />
        </div>

        <div className="foot">
          <button className="btn btn-secondary" onClick={props.onClose}>Cancel</button>
          <button className="btn btn-primary" onClick={submit} disabled={passwordInvalid}>
            {editing ? "Save changes" : "Share folder"}
          </button>
        </div>
      </div>
    </div>
  );
}
