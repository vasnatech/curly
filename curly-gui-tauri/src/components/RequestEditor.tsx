import { useState } from "react";
import { emptyHeader, type AuthEditorState, type RequestTabState } from "../tabState";
import type { Extraction, KvPair } from "../types";

const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

interface RequestEditorProps {
  tab: RequestTabState;
  onChange: (patch: Partial<RequestTabState>) => void;
  onSend: () => void;
  onSave: () => void;
  onSaveAs: () => void;
  onRenameBreadcrumb: () => void;
}

function splitFolderAndName(path: string): [string, string] {
  const idx = path.lastIndexOf("/");
  if (idx === -1) return ["", path];
  return [path.slice(0, idx), path.slice(idx + 1)];
}

function blankExtraction(source: Extraction["source"]): Extraction {
  switch (source) {
    case "body":
      return { source: "body", name: "", path: "", secret: false };
    case "header":
      return { source: "header", name: "", header: "", secret: false };
    case "template":
      return { source: "template", name: "", template: "", secret: false };
  }
}

/** Reconstruct `rule` as a different `source` variant, carrying `name`/
 * `secret` over and starting the kind-specific field blank — mirrors the
 * egui GUI's `rebuild_extraction`. */
function rebuildExtraction(rule: Extraction, source: Extraction["source"]): Extraction {
  const blank = blankExtraction(source);
  return { ...blank, name: rule.name, secret: rule.secret };
}

export function RequestEditor({ tab, onChange, onSend, onSave, onSaveAs, onRenameBreadcrumb }: RequestEditorProps) {
  const [subtab, setSubtab] = useState<"variables" | "auth" | "headers" | "body" | "extraction">("headers");

  function updateHeader(i: number, patch: Partial<KvPair>) {
    const headers = tab.headers.map((h, idx) => (idx === i ? { ...h, ...patch } : h));
    onChange({ headers });
  }

  function removeHeader(i: number) {
    onChange({ headers: tab.headers.filter((_, idx) => idx !== i) });
  }

  function updateAuth(patch: Partial<AuthEditorState>) {
    onChange({ auth: { ...tab.auth, ...patch } });
  }

  function updateOverride(i: number, patch: Partial<KvPair>) {
    onChange({ overrides: tab.overrides.map((o, idx) => (idx === i ? { ...o, ...patch } : o)) });
  }

  function removeOverride(i: number) {
    onChange({ overrides: tab.overrides.filter((_, idx) => idx !== i) });
  }

  function updateExtractField(i: number, patch: Partial<Extraction>) {
    onChange({
      extractRules: tab.extractRules.map((r, idx) => (idx === i ? ({ ...r, ...patch } as Extraction) : r)),
    });
  }

  function setExtractKind(i: number, source: Extraction["source"]) {
    onChange({
      extractRules: tab.extractRules.map((r, idx) => (idx === i ? rebuildExtraction(r, source) : r)),
    });
  }

  function removeExtractRule(i: number) {
    onChange({ extractRules: tab.extractRules.filter((_, idx) => idx !== i) });
  }

  return (
    <div className="request-editor">
      <div className="breadcrumb">
        {tab.loadedRequest ? (
          <>
            <span className="weak">
              {tab.loadedRequest.collection} ›{" "}
              {splitFolderAndName(tab.loadedRequest.path)[0] && `${splitFolderAndName(tab.loadedRequest.path)[0]} › `}
            </span>
            <input
              className="breadcrumb-name"
              value={tab.breadcrumbDraft}
              onChange={(e) => onChange({ breadcrumbDraft: e.target.value })}
              onKeyDown={(e) => {
                if (e.key === "Enter") onRenameBreadcrumb();
              }}
            />
            <button className="btn-icon" title="Save name" onClick={onRenameBreadcrumb}>
              ✓
            </button>
          </>
        ) : (
          <span className="weak">New request — not yet saved in a collection</span>
        )}
      </div>

      <div className="method-url-row">
        <select value={tab.method} onChange={(e) => onChange({ method: e.target.value })}>
          {METHODS.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
        <input
          className="url-field"
          value={tab.url}
          placeholder="https://api.example.com/..."
          onChange={(e) => onChange({ url: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Enter") onSend();
          }}
        />
        <button className="btn-send" disabled={tab.inFlight || tab.url.trim() === ""} onClick={onSend}>
          {tab.inFlight ? "Sending…" : "Send"}
        </button>
      </div>

      <div className="save-row">
        <button className="btn-save" onClick={onSave}>
          Save
        </button>
        <button className="btn-save" onClick={onSaveAs}>
          Save As…
        </button>
      </div>

      <div className="subtabs">
        <button className={subtab === "variables" ? "subtab active" : "subtab"} onClick={() => setSubtab("variables")}>
          Variables{tab.overrides.length > 0 ? ` (${tab.overrides.length})` : ""}
        </button>
        <button className={subtab === "auth" ? "subtab active" : "subtab"} onClick={() => setSubtab("auth")}>
          Authorization
        </button>
        <button className={subtab === "headers" ? "subtab active" : "subtab"} onClick={() => setSubtab("headers")}>
          Headers
        </button>
        <button className={subtab === "body" ? "subtab active" : "subtab"} onClick={() => setSubtab("body")}>
          Body
        </button>
        <button className={subtab === "extraction" ? "subtab active" : "subtab"} onClick={() => setSubtab("extraction")}>
          Extraction{tab.extractRules.length > 0 ? ` (${tab.extractRules.length})` : ""}
        </button>
      </div>

      {subtab === "variables" && (
        <div className="headers-table">
          <div className="weak" style={{ marginBottom: 6 }}>
            Ad-hoc overrides for this tab's own sends only — highest precedence, never saved anywhere.
          </div>
          {tab.overrides.map((o, i) => (
            <div className="header-row" key={i}>
              <input type="checkbox" checked={o.enabled} onChange={(e) => updateOverride(i, { enabled: e.target.checked })} />
              <input placeholder="KEY" value={o.name} onChange={(e) => updateOverride(i, { name: e.target.value })} />
              <input placeholder="value" value={o.value} onChange={(e) => updateOverride(i, { value: e.target.value })} />
              <button className="btn-icon" onClick={() => removeOverride(i)}>
                ✕
              </button>
            </div>
          ))}
          <button className="btn-add" onClick={() => onChange({ overrides: [...tab.overrides, emptyHeader()] })}>
            + Add override
          </button>
        </div>
      )}

      {subtab === "headers" && (
        <div className="headers-table">
          {tab.headers.map((h, i) => (
            <div className="header-row" key={i}>
              <input type="checkbox" checked={h.enabled} onChange={(e) => updateHeader(i, { enabled: e.target.checked })} />
              <input placeholder="Name" value={h.name} onChange={(e) => updateHeader(i, { name: e.target.value })} />
              <input placeholder="Value" value={h.value} onChange={(e) => updateHeader(i, { value: e.target.value })} />
              <button className="btn-icon" onClick={() => removeHeader(i)}>
                ✕
              </button>
            </div>
          ))}
          <button className="btn-add" onClick={() => onChange({ headers: [...tab.headers, emptyHeader()] })}>
            + Add header
          </button>
        </div>
      )}

      {subtab === "body" && (
        <textarea
          className="body-editor"
          value={tab.body}
          onChange={(e) => onChange({ body: e.target.value })}
          placeholder="Raw request body…"
        />
      )}

      {subtab === "auth" && (
        <div className="auth-editor">
          <div className="auth-kind-row">
            {(["none", "basic", "bearer"] as const).map((kind) => (
              <label key={kind}>
                <input
                  type="radio"
                  name="auth-kind"
                  checked={tab.auth.kind === kind}
                  onChange={() => updateAuth({ kind })}
                />
                {kind === "none" ? "None" : kind === "basic" ? "Basic" : "Bearer"}
              </label>
            ))}
          </div>
          {tab.auth.kind === "basic" && (
            <div className="auth-fields">
              <input
                placeholder="Username"
                value={tab.auth.username}
                onChange={(e) => updateAuth({ username: e.target.value })}
              />
              <input
                placeholder="Password"
                type="password"
                value={tab.auth.password}
                onChange={(e) => updateAuth({ password: e.target.value })}
              />
            </div>
          )}
          {tab.auth.kind === "bearer" && (
            <div className="auth-fields">
              <input placeholder="Token" value={tab.auth.token} onChange={(e) => updateAuth({ token: e.target.value })} />
            </div>
          )}
        </div>
      )}

      {subtab === "extraction" && (
        <div className="headers-table">
          <div className="weak" style={{ marginBottom: 6 }}>
            On a successful send, pulls a value out of the response into the session.
          </div>
          {tab.extractRules.map((rule, i) => (
            <div className="header-row" key={i}>
              <select value={rule.source} onChange={(e) => setExtractKind(i, e.target.value as Extraction["source"])}>
                <option value="body">Body</option>
                <option value="header">Header</option>
                <option value="template">Template</option>
              </select>
              <input placeholder="NAME" value={rule.name} onChange={(e) => updateExtractField(i, { name: e.target.value })} />
              {rule.source === "body" && (
                <input
                  placeholder="JSON path"
                  value={rule.path}
                  onChange={(e) => updateExtractField(i, { path: e.target.value })}
                />
              )}
              {rule.source === "header" && (
                <input
                  placeholder="Header name"
                  value={rule.header}
                  onChange={(e) => updateExtractField(i, { header: e.target.value })}
                />
              )}
              {rule.source === "template" && (
                <input
                  placeholder="template, e.g. Bearer {{TOKEN}}"
                  value={rule.template}
                  onChange={(e) => updateExtractField(i, { template: e.target.value })}
                />
              )}
              <label className="secret-label">
                <input type="checkbox" checked={rule.secret} onChange={(e) => updateExtractField(i, { secret: e.target.checked })} />
                secret
              </label>
              <button className="btn-icon" onClick={() => removeExtractRule(i)}>
                ✕
              </button>
            </div>
          ))}
          <button className="btn-add" onClick={() => onChange({ extractRules: [...tab.extractRules, blankExtraction("body")] })}>
            + Add rule
          </button>
        </div>
      )}
    </div>
  );
}
