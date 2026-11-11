export interface OpenProjectDialogState {
  path: string;
  error: string | null;
}

interface OpenProjectDialogProps {
  state: OpenProjectDialogState;
  onChange: (patch: Partial<OpenProjectDialogState>) => void;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * Asks for a project directory by path, because a browser cannot answer with one.
 *
 * This replaced a native folder picker (`@tauri-apps/plugin-dialog`) when the Tauri shell was
 * deleted. There is no equivalent on the web and that is deliberate on the browsers' part: the File
 * System Access API hands back an opaque handle, never a filesystem path, precisely so a page
 * cannot learn where things live. `openProject` needs a path, since the server opens it — so the
 * honest replacement is to ask.
 *
 * It is less convenient than a picker and it is not a downgrade in capability: the server reads the
 * directory either way, and `curly serve` runs on the same machine as the files.
 */
export function OpenProjectDialog({ state, onChange, onConfirm, onCancel }: OpenProjectDialogProps) {
  return (
    <div className="dialog-overlay">
      <div className="dialog">
        <h3>Open Project</h3>
        <div className="weak">The directory holding a `.curly` folder, as the server sees it.</div>
        <label>
          Path
          <input
            value={state.path}
            placeholder="/home/you/projects/my-api"
            autoFocus
            onChange={(e) => onChange({ path: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === "Enter") onConfirm();
            }}
          />
        </label>
        {state.error && <div className="dialog-error">{state.error}</div>}
        <div className="dialog-actions">
          <button className="btn-save" onClick={onConfirm}>
            Open
          </button>
          <button onClick={onCancel}>Cancel</button>
        </div>
      </div>
    </div>
  );
}
