// JSON syntax highlighting + search-match segmentation for the response
// body — a JS-string-index port of the egui GUI's hand-rolled
// `tokenize_json`/`append_with_search_highlight` (crates/curly-gui/src/
// app.rs), simplified where a webview makes it easy to: matched text
// renders as a plain highlighted run (losing its syntax color for just
// those characters) rather than combining token-level coloring with
// match-level highlighting character-by-character — a common, simpler
// pattern most code-search UIs already use, and a JS string's UTF-16
// indices need none of the "stay on an ASCII boundary" care the original
// Rust byte-walk needed either.

export type JsonTokenKind = "string" | "number" | "keyword" | "punct" | "other";

export interface JsonToken {
  text: string;
  kind: JsonTokenKind;
}

const TOKEN_RE = /"(?:\\.|[^"\\])*"|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|true|false|null|[{}[\]:,]/g;

export function tokenizeJson(text: string): JsonToken[] {
  const tokens: JsonToken[] = [];
  let lastIndex = 0;
  for (const m of text.matchAll(TOKEN_RE)) {
    const start = m.index ?? 0;
    if (start > lastIndex) {
      tokens.push({ text: text.slice(lastIndex, start), kind: "other" });
    }
    const t = m[0];
    let kind: JsonTokenKind;
    if (t.startsWith('"')) kind = "string";
    else if (t === "true" || t === "false" || t === "null") kind = "keyword";
    else if (/^[{}[\]:,]$/.test(t)) kind = "punct";
    else kind = "number";
    tokens.push({ text: t, kind });
    lastIndex = start + t.length;
  }
  if (lastIndex < text.length) tokens.push({ text: text.slice(lastIndex), kind: "other" });
  return tokens;
}

/** Every case-insensitive match's start index, in order — drives the "N of
 * M" label and which occurrence "current" points at. */
export function findMatches(text: string, searchLower: string): number[] {
  if (searchLower === "") return [];
  const lower = text.toLowerCase();
  const out: number[] = [];
  let pos = 0;
  for (;;) {
    const idx = lower.indexOf(searchLower, pos);
    if (idx === -1) break;
    out.push(idx);
    pos = idx + searchLower.length;
  }
  return out;
}

export type Segment =
  | { text: string; match: "none"; tokens?: JsonToken[] }
  | { text: string; match: "current" | "other" };

/** Split `text` into renderable segments: search matches become their own
 * plain-highlighted segments (current match tagged separately so it can get
 * a visually distinct color — see the M4 find-next fix this mirrors), and
 * everything in between gets JSON-tokenized for syntax coloring when
 * `isJson`. */
export function buildSegments(text: string, isJson: boolean, searchLower: string, currentMatchStart: number | null): Segment[] {
  if (searchLower === "") {
    return [{ text, match: "none", tokens: isJson ? tokenizeJson(text) : undefined }];
  }
  const lower = text.toLowerCase();
  const segments: Segment[] = [];
  let pos = 0;
  for (;;) {
    const idx = lower.indexOf(searchLower, pos);
    if (idx === -1) {
      const rest = text.slice(pos);
      if (rest) segments.push({ text: rest, match: "none", tokens: isJson ? tokenizeJson(rest) : undefined });
      break;
    }
    if (idx > pos) {
      const chunk = text.slice(pos, idx);
      segments.push({ text: chunk, match: "none", tokens: isJson ? tokenizeJson(chunk) : undefined });
    }
    segments.push({ text: text.slice(idx, idx + searchLower.length), match: idx === currentMatchStart ? "current" : "other" });
    pos = idx + searchLower.length;
  }
  return segments;
}
