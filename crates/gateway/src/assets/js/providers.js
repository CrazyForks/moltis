// ── Provider modal ──────────────────────────────────────

import { sendRpc } from "./helpers.js";
import { fetchModels } from "./models.js";
import * as S from "./state.js";

export var providerModal = S.$("providerModal");
export var providerModalBody = S.$("providerModalBody");
export var providerModalTitle = S.$("providerModalTitle");
var providerModalClose = S.$("providerModalClose");

export function openProviderModal() {
	providerModal.classList.remove("hidden");
	providerModalTitle.textContent = "Add Provider";
	providerModalBody.textContent = "Loading...";
	sendRpc("providers.available", {}).then((res) => {
		if (!res?.ok) {
			providerModalBody.textContent = "Failed to load providers.";
			return;
		}
		var providers = res.payload || [];
		providerModalBody.textContent = "";
		providers.forEach((p) => {
			var item = document.createElement("div");
			item.className = `provider-item${p.configured ? " configured" : ""}`;
			var name = document.createElement("span");
			name.className = "provider-item-name";
			name.textContent = p.displayName;
			item.appendChild(name);

			var badges = document.createElement("div");
			badges.className = "badge-row";

			if (p.configured) {
				var check = document.createElement("span");
				check.className = "provider-item-badge configured";
				check.textContent = "configured";
				badges.appendChild(check);
			}

			var badge = document.createElement("span");
			badge.className = `provider-item-badge ${p.authType}`;
			badge.textContent = p.authType === "oauth" ? "OAuth" : "API Key";
			badges.appendChild(badge);
			item.appendChild(badges);

			item.addEventListener("click", () => {
				if (p.authType === "api-key") showApiKeyForm(p);
				else if (p.authType === "oauth") showOAuthFlow(p);
			});
			providerModalBody.appendChild(item);
		});
	});
}

export function closeProviderModal() {
	providerModal.classList.add("hidden");
}

export function showApiKeyForm(provider) {
	providerModalTitle.textContent = provider.displayName;
	providerModalBody.textContent = "";

	var form = document.createElement("div");
	form.className = "provider-key-form";

	var label = document.createElement("label");
	label.className = "text-xs text-[var(--muted)]";
	label.textContent = "API Key";
	form.appendChild(label);

	var inp = document.createElement("input");
	inp.className = "provider-key-input";
	inp.type = "password";
	inp.placeholder = "sk-...";
	form.appendChild(inp);

	var btns = document.createElement("div");
	btns.className = "btn-row";

	var backBtn = document.createElement("button");
	backBtn.className = "provider-btn provider-btn-secondary";
	backBtn.textContent = "Back";
	backBtn.addEventListener("click", openProviderModal);
	btns.appendChild(backBtn);

	var saveBtn = document.createElement("button");
	saveBtn.className = "provider-btn";
	saveBtn.textContent = "Save";
	saveBtn.addEventListener("click", () => {
		var key = inp.value.trim();
		if (!key) return;
		saveBtn.disabled = true;
		saveBtn.textContent = "Saving...";
		sendRpc("providers.save_key", {
			provider: provider.name,
			apiKey: key,
		}).then((res) => {
			if (res?.ok) {
				providerModalBody.textContent = "";
				var status = document.createElement("div");
				status.className = "provider-status";
				status.textContent = `${provider.displayName} configured successfully!`;
				providerModalBody.appendChild(status);
				fetchModels();
				if (S.refreshProvidersPage) S.refreshProvidersPage();
				setTimeout(closeProviderModal, 1500);
			} else {
				saveBtn.disabled = false;
				saveBtn.textContent = "Save";
				var err = res?.error?.message || "Failed to save";
				inp.classList.add("field-error");
				label.textContent = err;
				label.classList.add("text-error");
			}
		});
	});
	btns.appendChild(saveBtn);
	form.appendChild(btns);
	providerModalBody.appendChild(form);
	inp.focus();
}

export function showOAuthFlow(provider) {
	providerModalTitle.textContent = provider.displayName;
	providerModalBody.textContent = "";

	var wrapper = document.createElement("div");
	wrapper.className = "provider-key-form";

	var desc = document.createElement("div");
	desc.className = "text-xs text-[var(--muted)]";
	desc.textContent = `Click below to authenticate with ${provider.displayName} via OAuth.`;
	wrapper.appendChild(desc);

	var btns = document.createElement("div");
	btns.className = "btn-row";

	var backBtn = document.createElement("button");
	backBtn.className = "provider-btn provider-btn-secondary";
	backBtn.textContent = "Back";
	backBtn.addEventListener("click", openProviderModal);
	btns.appendChild(backBtn);

	var connectBtn = document.createElement("button");
	connectBtn.className = "provider-btn";
	connectBtn.textContent = "Connect";
	connectBtn.addEventListener("click", () => {
		connectBtn.disabled = true;
		connectBtn.textContent = "Starting...";
		sendRpc("providers.oauth.start", { provider: provider.name }).then((res) => {
			if (res?.ok && res.payload && res.payload.authUrl) {
				window.open(res.payload.authUrl, "_blank");
				connectBtn.textContent = "Waiting for auth...";
				pollOAuthStatus(provider);
			} else if (res?.ok && res.payload && res.payload.deviceFlow) {
				connectBtn.textContent = "Waiting for auth...";
				desc.classList.remove("text-error");
				desc.textContent = "";
				var linkEl = document.createElement("a");
				linkEl.href = res.payload.verificationUri;
				linkEl.target = "_blank";
				linkEl.className = "oauth-link";
				linkEl.textContent = res.payload.verificationUri;
				var codeEl = document.createElement("strong");
				codeEl.textContent = res.payload.userCode;
				desc.appendChild(document.createTextNode("Go to "));
				desc.appendChild(linkEl);
				desc.appendChild(document.createTextNode(" and enter code: "));
				desc.appendChild(codeEl);
				pollOAuthStatus(provider);
			} else {
				connectBtn.disabled = false;
				connectBtn.textContent = "Connect";
				desc.textContent = res?.error?.message || "Failed to start OAuth";
				desc.classList.add("text-error");
			}
		});
	});
	btns.appendChild(connectBtn);
	wrapper.appendChild(btns);
	providerModalBody.appendChild(wrapper);
}

function pollOAuthStatus(provider) {
	var attempts = 0;
	var maxAttempts = 60;
	var timer = setInterval(() => {
		attempts++;
		if (attempts > maxAttempts) {
			clearInterval(timer);
			providerModalBody.textContent = "";
			var timeout = document.createElement("div");
			timeout.className = "text-xs text-[var(--error)]";
			timeout.textContent = "OAuth timed out. Please try again.";
			providerModalBody.appendChild(timeout);
			return;
		}
		sendRpc("providers.oauth.status", { provider: provider.name }).then((res) => {
			if (res?.ok && res.payload && res.payload.authenticated) {
				clearInterval(timer);
				providerModalBody.textContent = "";
				var status = document.createElement("div");
				status.className = "provider-status";
				status.textContent = `${provider.displayName} connected successfully!`;
				providerModalBody.appendChild(status);
				fetchModels();
				if (S.refreshProvidersPage) S.refreshProvidersPage();
				setTimeout(closeProviderModal, 1500);
			}
		});
	}, 2000);
}

providerModalClose.addEventListener("click", closeProviderModal);
providerModal.addEventListener("click", (e) => {
	if (e.target === providerModal) closeProviderModal();
});
