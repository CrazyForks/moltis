// ── Sandbox toggle ──────────────────────────────────────────

import { sendRpc } from "./helpers.js";
import * as S from "./state.js";

export function updateSandboxUI(enabled) {
	S.setSessionSandboxEnabled(!!enabled);
	if (!(S.sandboxLabel && S.sandboxToggleBtn)) return;
	if (S.sessionSandboxEnabled) {
		S.sandboxLabel.textContent = "sandboxed";
		S.sandboxToggleBtn.style.borderColor = "var(--accent, #f59e0b)";
		S.sandboxToggleBtn.style.color = "var(--accent, #f59e0b)";
	} else {
		S.sandboxLabel.textContent = "direct";
		S.sandboxToggleBtn.style.borderColor = "";
		S.sandboxToggleBtn.style.color = "var(--muted)";
	}
}

export function bindSandboxToggleEvents() {
	if (!S.sandboxToggleBtn) return;
	S.sandboxToggleBtn.addEventListener("click", () => {
		var newVal = !S.sessionSandboxEnabled;
		sendRpc("sessions.patch", {
			key: S.activeSessionKey,
			sandbox_enabled: newVal,
		}).then((res) => {
			if (res?.result) {
				updateSandboxUI(res.result.sandbox_enabled);
			} else {
				updateSandboxUI(newVal);
			}
		});
	});
}
