// ── Preact signal bridge for shared state ─────────────────────
// Mirrors key state.js vars as Preact signals so that both imperative
// code (websocket.js) and Preact pages can coexist during migration.
//
// state.js setters are patched (below) to also update these signals.

import { signal } from "@preact/signals";

export var connected = signal(false);
export var models = signal([]);
export var projects = signal([]);
export var sessions = signal([]);
export var activeSessionKey = signal("");
export var selectedModelId = signal("");
export var cachedChannels = signal(null);
export var unseenErrors = signal(0);
export var unseenWarns = signal(0);
