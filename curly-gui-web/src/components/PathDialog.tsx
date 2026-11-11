export interface PathDialogState {
  mode: "rename" | "duplicate";
  collection: string;
  oldPath: string;
  folder: string;
  name: string;
  error: string | null;
}

interface PathDialogProps {
  state: PathDialogState;
  onChange: (patch: Partial<PathDialogState>) => void;
  onConfirm: () => void;
  onCancel: () => void;
}

export function PathDialog({ state, onChange, onConfirm, onCancel }: PathDialogProps) {
  const title = state.mode === "rename" ? "Rename / Move Request" : "Duplicate Request";
  const confirmLabel = state.mode === "rename" ? "Rename" : "Duplicate";
  return (
    <div className="dialog-overlay">
      <div className="dialog">
        <h3>{title}</h3>
        <div className="weak">
          From: "{state.collection}/{state.oldPath}"
        </div>
        <label>
          Folder (optional)
          <input value={state.folder} placeholder="Auth/OAuth" onChange={(e) => onChange({ folder: e.target.value })} />
        </label>
        <label>
          Name
          <input value={state.name} onChange={(e) => onChange({ name: e.target.value })} />
        </label>
        {state.error && <div className="dialog-error">{state.error}</div>}
        <div className="dialog-actions">
          <button className="btn-save" onClick={onConfirm}>
            {confirmLabel}
          </button>
          <button onClick={onCancel}>Cancel</button>
        </div>
      </div>
    </div>
  );
}
