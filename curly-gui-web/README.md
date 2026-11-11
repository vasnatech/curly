# curly's web frontend

React + TypeScript + Vite. It talks to `curly serve` over HTTP and has no backend of its own.

```sh
npm install
npm run dev          # http://localhost:1420, proxying /api to curly serve
```

In another terminal:

```sh
ziman/bin/curly serve --port 8787
```

`/api` is **proxied** rather than reached by an absolute URL, because `curly serve` refuses a
request whose `Origin` does not match its `Host`: these commands read and write the user's files,
and a server that accepted them from any origin would hand every open tab a file-writing API. Set
`CURLY_PORT` to match a different `--port`.

## What used to be here

This was a Tauri shell -- a Rust process hosting a webview, with the frontend reaching storage
through Tauri's `invoke`. The Rust half (`src-tauri/`) was **deleted on 2026-11-11**: `curly serve`
had already replaced its bridge, `api.ts` had already moved to `fetch`, and the only thing still
reaching for Tauri was a native folder picker. A browser will not hand a page a filesystem path, so
that became a dialog asking for one -- the server opens it and runs on the same machine as the
files.

curly has two graphical front-ends now: this one, and `curly gui`, a desktop window written in
Ziman that needs no server at all. See `docs/GUI.md` and `docs/ZIMAN.md`.
