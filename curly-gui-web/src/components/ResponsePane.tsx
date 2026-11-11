import { useEffect, useMemo, useRef, useState } from "react";
import * as api from "../api";
import { bytesToBase64, imageFileExtension, isImageContentType } from "../bytes";
import { buildSegments, findMatches } from "../jsonHighlight";
import type { RequestTabState } from "../tabState";
import type { HistoryEntry } from "../types";

function prettyBody(body: string): { text: string; isJson: boolean } {
  try {
    return { text: JSON.stringify(JSON.parse(body), null, 2), isJson: true };
  } catch {
    return { text: body, isJson: false };
  }
}

function statusClass(status: number): string {
  if (status < 300) return "status-ok";
  if (status < 400) return "status-redirect";
  return "status-error";
}

interface ResponsePaneProps {
  tab: RequestTabState;
  onLoadHistoryEntry: (entry: HistoryEntry) => void;
}

export function ResponsePane({ tab, onLoadHistoryEntry }: ResponsePaneProps) {
  const [subtab, setSubtab] = useState<"body" | "headers" | "history">("body");
  const [search, setSearch] = useState("");
  const [currentMatch, setCurrentMatch] = useState(0);
  const [history, setHistory] = useState<HistoryEntry[] | null>(null);
  const [saveNotice, setSaveNotice] = useState<string | null>(null);
  const currentMatchRef = useRef<HTMLElement | null>(null);

  const { response, responseError, inFlight } = tab;

  // A response's `Content-Type` decides Body's whole rendering path: an
  // image renders as an <img>, everything else goes through the existing
  // pretty-print/highlight/search pipeline.
  const imageContentType = useMemo(() => (response ? isImageContentType(response.headers) : null), [response]);
  const imageDataUrl = useMemo(
    () => (response && imageContentType ? `data:${imageContentType};base64,${bytesToBase64(response.body_bytes)}` : null),
    [response, imageContentType],
  );

  const { text: bodyText, isJson } = useMemo(
    () => (response && !imageContentType ? prettyBody(response.body) : { text: "", isJson: false }),
    [response, imageContentType],
  );

  const searchLower = search.toLowerCase();
  const matches = useMemo(() => findMatches(bodyText, searchLower), [bodyText, searchLower]);
  const currentMatchStart = matches.length > 0 ? matches[currentMatch % matches.length] : null;
  const segments = useMemo(() => buildSegments(bodyText, isJson, searchLower, currentMatchStart), [bodyText, isJson, searchLower, currentMatchStart]);

  useEffect(() => {
    setCurrentMatch(0);
  }, [search, response]);

  useEffect(() => {
    currentMatchRef.current?.scrollIntoView({ block: "center" });
  }, [currentMatch, search]);

  useEffect(() => {
    setSaveNotice(null);
  }, [response]);

  useEffect(() => {
    if (subtab !== "history") return;
    api
      .listHistory(200)
      .then((entries) => setHistory(entries.filter((e) => e.method === tab.method && e.url === tab.url)))
      .catch(() => setHistory([]));
    // `tab.response` is included so a send completing while History is
    // already the open sub-tab refetches immediately — otherwise this only
    // re-ran on switching sub-tabs (`subtab` changing), so a fresh send
    // wouldn't show up until you navigated away and back.
  }, [subtab, tab.method, tab.url, tab.response]);

  function advance(forward: boolean) {
    if (matches.length === 0) return;
    setCurrentMatch((c) => (forward ? (c + 1) % matches.length : (c - 1 + matches.length) % matches.length));
  }

  /** Hands the user `response.body_bytes` verbatim — byte-exact, including
   * for an image, same as the egui GUI's own "Save Response…" (the naive
   * alternative, writing the lossy UTF-8-decoded `body` string, would
   * silently corrupt any binary response).
   *
   * **A download, not a path.** This used to open a native save dialog and
   * ask the Rust side to write the bytes to the path it returned; the
   * backend is an HTTP server now, which deliberately exposes no
   * write-to-any-path command (see `api.saveBytes`). So the user gets the
   * file through their browser and where it lands is between them and it —
   * which is also why the notice no longer names a directory it cannot
   * know. */
  async function onSaveResponse() {
    if (!response) return;
    const filename = imageContentType ? `response.${imageFileExtension(imageContentType)}` : isJson ? "response.json" : "response.txt";
    try {
      await api.saveBytes(filename, response.body_bytes);
      setSaveNotice(`downloading ${filename}`);
    } catch (e) {
      setSaveNotice(`failed to save: ${e}`);
    }
  }

  return (
    <div className="response-pane">
      <div className="response-header-row">
        <h3>Response</h3>
      </div>

      <div className="subtabs">
        <button className={subtab === "body" ? "subtab active" : "subtab"} onClick={() => setSubtab("body")}>
          Body
        </button>
        <button className={subtab === "headers" ? "subtab active" : "subtab"} onClick={() => setSubtab("headers")}>
          Headers
        </button>
        <button className={subtab === "history" ? "subtab active" : "subtab"} onClick={() => setSubtab("history")}>
          History
        </button>
      </div>

      {subtab === "history" ? (
        <div className="extra-tab-list">
          {history === null && <div className="weak">Loading…</div>}
          {history !== null && history.length === 0 && (
            <div className="weak">No history for this request yet — send it to start building history.</div>
          )}
          {history?.map((entry, i) => (
            <div key={i} className="tree-row" onClick={() => onLoadHistoryEntry(entry)}>
              <span className={statusClass(entry.status)}>{entry.status}</span>
              <span className="weak">
                {new Date(entry.sent_at).toLocaleString()} ({entry.elapsed_ms} ms)
              </span>
            </div>
          ))}
        </div>
      ) : (
        <>
          {inFlight && <div className="weak">Sending…</div>}
          {!inFlight && !response && !responseError && <div className="weak">No response yet.</div>}
          {!inFlight && responseError && <div className="response-error">Error: {responseError}</div>}

          {!inFlight && response && (
            <>
              <div className="response-meta">
                <span className={statusClass(response.status)}>{response.status}</span>
                <span>
                  {response.elapsed.secs}s {Math.round(response.elapsed.nanos / 1e6)}ms
                </span>
                <span>{response.body_bytes.length} bytes</span>
                <button className="btn-icon-labeled" onClick={onSaveResponse}>
                  Save Response…
                </button>
                {saveNotice && <span className="weak">{saveNotice}</span>}
              </div>

              {subtab === "headers" && (
                <pre className="response-body">{response.headers.map(([k, v]) => `${k}: ${v}`).join("\n")}</pre>
              )}

              {subtab === "body" && imageContentType && (
                <>
                  {imageDataUrl ? (
                    <div className="response-image-wrap">
                      <img className="response-image" src={imageDataUrl} alt="response" />
                    </div>
                  ) : (
                    <div className="response-error">Content-Type looks like an image, but the body couldn't be decoded.</div>
                  )}
                </>
              )}

              {subtab === "body" && !imageContentType && (
                <>
                  <div className="search-row">
                    <input
                      placeholder="filter body…"
                      value={search}
                      onChange={(e) => setSearch(e.target.value)}
                    />
                    {matches.length > 0 && (
                      <>
                        <button className="btn-icon" onClick={() => advance(false)}>
                          ◀
                        </button>
                        <span className="weak">
                          match {(currentMatch % matches.length) + 1} of {matches.length}
                        </span>
                        <button className="btn-icon" onClick={() => advance(true)}>
                          ▶
                        </button>
                      </>
                    )}
                    {search !== "" && matches.length === 0 && <span className="weak">0 match(es)</span>}
                  </div>
                  <pre className="response-body">
                    {segments.map((seg, i) => {
                      if (seg.match === "none") {
                        if (seg.tokens) {
                          return seg.tokens.map((t, j) => (
                            <span key={`${i}-${j}`} className={`json-${t.kind}`}>
                              {t.text}
                            </span>
                          ));
                        }
                        return <span key={i}>{seg.text}</span>;
                      }
                      if (seg.match === "current") {
                        return (
                          <mark
                            key={i}
                            className="match-current"
                            ref={(el) => {
                              currentMatchRef.current = el;
                            }}
                          >
                            {seg.text}
                          </mark>
                        );
                      }
                      return (
                        <mark key={i} className="match-other">
                          {seg.text}
                        </mark>
                      );
                    })}
                  </pre>
                </>
              )}
            </>
          )}
        </>
      )}
    </div>
  );
}
