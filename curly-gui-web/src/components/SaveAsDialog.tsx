export interface SaveAsDialogState {
  collection: string;
  folder: string;
  name: string;
  error: string | null;
}

interface SaveAsDialogProps {
  state: SaveAsDialogState;
  onChange: (patch: Partial<SaveAsDialogState>) => void;
  onConfirm: () => void;
  onCancel: () => void;
}

export function SaveAsDialog({ state, onChange, onConfirm, onCancel }: SaveAsDialogProps) {
  return (
    <div className="dialog-overlay">
      <div className="dialog">
        <h3>Save Request</h3>
        <label>
          Collection
          <input value={state.collection} onChange={(e) => onChange({ collection: e.target.value })} />
        </label>
        <label>
          Folder (optional)
          <input
            value={state.folder}
            placeholder="Auth/OAuth"
            onChange={(e) => onChange({ folder: e.target.value })}
          />
        </label>
        <label>
          Name
          <input value={state.name} onChange={(e) => onChange({ name: e.target.value })} />
        </label>
        {state.error && <div className="dialog-error">{state.error}</div>}
        <div className="dialog-actions">
          <button className="btn-save" onClick={onConfirm}>
            Save
          </button>
          <button onClick={onCancel}>Cancel</button>
        </div>
      </div>
    </div>
  );
}
