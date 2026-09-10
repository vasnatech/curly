export interface FolderDialogState {
  collection: string;
  parentPath: string;
  name: string;
  error: string | null;
}

interface FolderDialogProps {
  state: FolderDialogState;
  onChange: (patch: Partial<FolderDialogState>) => void;
  onConfirm: () => void;
  onCancel: () => void;
}

export function FolderDialog({ state, onChange, onConfirm, onCancel }: FolderDialogProps) {
  const parent = state.parentPath ? `${state.collection}/${state.parentPath}` : state.collection;
  return (
    <div className="dialog-overlay">
      <div className="dialog">
        <h3>New Folder</h3>
        <div className="weak">In: "{parent}"</div>
        <label>
          Name
          <input value={state.name} onChange={(e) => onChange({ name: e.target.value })} />
        </label>
        {state.error && <div className="dialog-error">{state.error}</div>}
        <div className="dialog-actions">
          <button className="btn-save" onClick={onConfirm}>
            Create
          </button>
          <button onClick={onCancel}>Cancel</button>
        </div>
      </div>
    </div>
  );
}
