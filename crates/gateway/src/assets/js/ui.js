// ── Shared Preact UI components ───────────────────────────────

import { signal } from "@preact/signals";
import { html } from "htm/preact";
import { useEffect } from "preact/hooks";

// ── Toast notifications ──────────────────────────────────────
export var toasts = signal([]);
var toastId = 0;

export function showToast(message, type) {
	var id = ++toastId;
	toasts.value = toasts.value.concat([{ id: id, message: message, type: type }]);
	setTimeout(() => {
		toasts.value = toasts.value.filter((t) => t.id !== id);
	}, 4000);
}

export function Toasts() {
	return html`<div class="skills-toast-container">
    ${toasts.value.map((t) => {
			var bg = t.type === "error" ? "var(--error, #e55)" : "var(--accent)";
			return html`<div key=${t.id} style=${{
				pointerEvents: "auto",
				maxWidth: "420px",
				padding: "10px 16px",
				borderRadius: "6px",
				fontSize: ".8rem",
				fontWeight: 500,
				color: "#fff",
				background: bg,
				boxShadow: "0 4px 12px rgba(0,0,0,.15)",
			}}>${t.message}</div>`;
		})}
  </div>`;
}

// ── Modal wrapper ────────────────────────────────────────────
export function Modal(props) {
	var show = props.show;
	var onClose = props.onClose;
	var title = props.title;

	function onBackdrop(e) {
		if (e.target === e.currentTarget && onClose) onClose();
	}

	useEffect(() => {
		if (!show) return;
		function onKey(e) {
			if (e.key === "Escape" && onClose) onClose();
		}
		document.addEventListener("keydown", onKey);
		return () => document.removeEventListener("keydown", onKey);
	}, [show, onClose]);

	if (!show) return null;

	return html`<div class="modal-overlay" onClick=${onBackdrop} style="display:flex;position:fixed;inset:0;background:rgba(0,0,0,.45);z-index:100;align-items:center;justify-content:center;">
    <div class="modal-box" style="background:var(--surface);border-radius:var(--radius);padding:20px;max-width:500px;width:90%;max-height:85vh;overflow-y:auto;box-shadow:0 8px 32px rgba(0,0,0,.25);border:1px solid var(--border);">
      <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:14px;">
        <h3 style="margin:0;font-size:.95rem;font-weight:600;color:var(--text-strong)">${title}</h3>
        <button onClick=${onClose} style="background:none;border:none;color:var(--muted);font-size:1.1rem;cursor:pointer;padding:2px 6px">\u2715</button>
      </div>
      ${props.children}
    </div>
  </div>`;
}

// ── Confirm dialog ───────────────────────────────────────────
var confirmState = signal(null);

export function requestConfirm(message) {
	return new Promise((resolve) => {
		confirmState.value = { message: message, resolve: resolve };
	});
}

export function ConfirmDialog() {
	var s = confirmState.value;
	if (!s) return null;

	function yes() {
		s.resolve(true);
		confirmState.value = null;
	}
	function no() {
		s.resolve(false);
		confirmState.value = null;
	}

	return html`<${Modal} show=${true} onClose=${no} title="Confirm">
    <p style="font-size:.85rem;color:var(--text);margin:0 0 16px;">${s.message}</p>
    <div style="display:flex;gap:8px;justify-content:flex-end;">
      <button onClick=${no} class="provider-btn provider-btn-secondary">Cancel</button>
      <button onClick=${yes} class="provider-btn">Confirm</button>
    </div>
  </${Modal}>`;
}
