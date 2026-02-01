// ── First-run setup page ─────────────────────────────────────

import { html } from "htm/preact";
import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import { registerPage } from "./router.js";

function SetupPage() {
	var [password, setPassword] = useState("");
	var [confirm, setConfirm] = useState("");
	var [setupCode, setSetupCode] = useState("");
	var [codeRequired, setCodeRequired] = useState(false);
	var [error, setError] = useState(null);
	var [saving, setSaving] = useState(false);

	useEffect(() => {
		fetch("/api/auth/status")
			.then((r) => r.json())
			.then((data) => {
				if (data.setup_code_required) setCodeRequired(true);
			})
			.catch(() => {});
	}, []);

	function onSubmit(e) {
		e.preventDefault();
		setError(null);
		if (codeRequired && setupCode.trim().length === 0) {
			setError("Enter the setup code shown in the terminal.");
			return;
		}
		if (password.length < 8) {
			setError("Password must be at least 8 characters.");
			return;
		}
		if (password !== confirm) {
			setError("Passwords do not match.");
			return;
		}
		setSaving(true);
		var body = { password };
		if (codeRequired) body.setup_code = setupCode.trim();
		fetch("/api/auth/setup", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify(body),
		})
			.then((r) => {
				if (r.ok) {
					location.href = "/";
				} else {
					return r.text().then((t) => {
						setError(t || "Setup failed");
						setSaving(false);
					});
				}
			})
			.catch((err) => {
				setError(err.message);
				setSaving(false);
			});
	}

	return html`<div class="auth-page">
		<div class="auth-card">
			<h1 class="auth-title">Welcome to moltis</h1>
			<p class="auth-subtitle">Set a password to secure your instance.</p>
			<form onSubmit=${onSubmit}>
				${
					codeRequired
						? html`<div class="auth-field">
							<label class="settings-label">Setup code</label>
							<input
								type="text"
								class="settings-input"
								inputmode="numeric"
								pattern="[0-9]*"
								value=${setupCode}
								onInput=${(e) => setSetupCode(e.target.value)}
								placeholder="6-digit code from terminal"
								autofocus
							/>
						</div>`
						: null
				}
				<div class="auth-field">
					<label class="settings-label">Password</label>
					<input
						type="password"
						class="settings-input"
						value=${password}
						onInput=${(e) => setPassword(e.target.value)}
						placeholder="At least 8 characters"
						autofocus=${!codeRequired}
					/>
				</div>
				<div class="auth-field">
					<label class="settings-label">Confirm password</label>
					<input
						type="password"
						class="settings-input"
						value=${confirm}
						onInput=${(e) => setConfirm(e.target.value)}
						placeholder="Repeat password"
					/>
				</div>
				${error ? html`<p class="auth-error">${error}</p>` : null}
				<button type="submit" class="settings-btn auth-submit" disabled=${saving}>
					${saving ? "Setting up\u2026" : "Set password"}
				</button>
			</form>
		</div>
	</div>`;
}

var containerRef = null;

registerPage(
	"/setup",
	(container) => {
		containerRef = container;
		container.style.cssText = "display:flex;align-items:center;justify-content:center;height:100%;";
		render(html`<${SetupPage} />`, container);
	},
	() => {
		if (containerRef) render(null, containerRef);
		containerRef = null;
	},
);
