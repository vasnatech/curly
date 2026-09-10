import { useEffect, useState } from "react";
import * as api from "../api";
import type { Variable } from "../types";

interface EnvironmentTabViewProps {
  name: string | null;
  refreshKey: number;
  onNotice: (msg: string) => void;
  onSaved: () => void;
  onDeleted: () => void;
}

export function EnvironmentTabView({ name, refreshKey, onNotice, onSaved, onDeleted }: EnvironmentTabViewProps) {
  const [variables, setVariables] = useState<Variable[]>([]);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    if (!name) {
      setVariables([]);
      setLoaded(false);
      return;
    }
    setLoaded(false);
    api
      .loadEnvironment(name)
      .then((env) => {
        setVariables(env.variables);
        setLoaded(true);
      })
      .catch((e) => onNotice(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [name, refreshKey]);

  if (!name) {
    return (
      <div className="extra-tab-view">
        <div className="weak">No environment selected — pick or create one in the sidebar.</div>
      </div>
    );
  }

  function updateVar(i: number, patch: Partial<Variable>) {
    setVariables((prev) => prev.map((v, idx) => (idx === i ? { ...v, ...patch } : v)));
  }

  function removeVar(i: number) {
    setVariables((prev) => prev.filter((_, idx) => idx !== i));
  }

  async function save() {
    try {
      const toSave = variables.filter((v) => v.key.trim() !== "");
      await api.saveEnvironment({ name: name!, variables: toSave });
      onNotice(`saved ${toSave.length} variable(s) in "${name}"`);
      onSaved();
    } catch (e) {
      onNotice(`failed to save: ${e}`);
    }
  }

  async function del() {
    try {
      await api.deleteEnvironment(name!);
      onNotice(`deleted environment "${name}"`);
      onDeleted();
    } catch (e) {
      onNotice(`failed to delete: ${e}`);
    }
  }

  return (
    <div className="extra-tab-view">
      <h2>Environment: {name}</h2>
      {loaded && (
        <>
          <div className="env-vars">
            {variables.map((v, i) => (
              <div className="header-row" key={i}>
                <input placeholder="KEY" value={v.key} onChange={(e) => updateVar(i, { key: e.target.value })} />
                <input placeholder="value" value={v.value} onChange={(e) => updateVar(i, { value: e.target.value })} />
                <label className="secret-label">
                  <input type="checkbox" checked={v.secret} onChange={(e) => updateVar(i, { secret: e.target.checked })} />
                  secret
                </label>
                <button className="btn-icon" onClick={() => removeVar(i)}>
                  ✕
                </button>
              </div>
            ))}
            <button className="btn-add" onClick={() => setVariables((prev) => [...prev, { key: "", value: "", secret: false }])}>
              + Add variable
            </button>
          </div>
          <div className="extra-tab-actions">
            <button className="btn-save" onClick={save}>
              Save
            </button>
            <button onClick={del}>Delete environment</button>
          </div>
        </>
      )}
    </div>
  );
}
