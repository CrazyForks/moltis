// ── Entry point ────────────────────────────────────────────

import { onEvent } from "./events.js";
import { renderSessionProjectSelect } from "./project-combo.js";
import { renderProjectSelect } from "./projects.js";
import { mount, navigate, registerPage } from "./router.js";
import { fetchSessions, refreshActiveSession, renderSessionList } from "./sessions.js";
import * as S from "./state.js";
import { initTheme, injectMarkdownStyles } from "./theme.js";
import { connect } from "./websocket.js";

// Import page modules to register their routes
import "./page-chat.js";
import "./page-crons.js";
import "./page-projects.js";
import "./page-providers.js";
import "./page-channels.js";
import "./page-logs.js";
import "./page-plugins.js";
import "./page-skills.js";
import "./page-settings.js";
import "./page-images.js";
import "./page-setup.js";
import { setHasPasskeys } from "./page-login.js";

// Import side-effect modules
import "./session-search.js";

// Redirect root to /chats
registerPage("/", () => {
	navigate("/chats");
});

initTheme();
injectMarkdownStyles();
onEvent("session", (payload) => {
	fetchSessions();
	if (payload && payload.kind === "patched" && payload.sessionKey === S.activeSessionKey) {
		refreshActiveSession();
	}
});

// Check auth status before mounting the app.
fetch("/api/auth/status")
	.then((r) => (r.ok ? r.json() : null))
	.then((auth) => {
		if (!auth) {
			// Auth endpoints not available — no auth configured, proceed normally.
			startApp();
			return;
		}
		if (auth.setup_required) {
			mount("/setup");
			return;
		}
		setHasPasskeys(auth.has_passkeys);
		if (!auth.authenticated) {
			mount("/login");
			return;
		}
		if (auth.auth_disabled) {
			showAuthDisabledBanner();
		}
		startApp();
	})
	.catch(() => {
		// If auth check fails, proceed anyway (backward compat).
		startApp();
	});

function showAuthDisabledBanner() {
	var el = document.getElementById("authDisabledBanner");
	if (el) el.style.display = "";
}

function applyModels(models) {
	S.setModels(models || []);
	if (S.models.length === 0) return;
	var saved = localStorage.getItem("moltis-model") || "";
	var found = S.models.find((m) => m.id === saved);
	if (found) {
		S.setSelectedModelId(found.id);
	} else {
		S.setSelectedModelId(S.models[0].id);
		localStorage.setItem("moltis-model", S.selectedModelId);
	}
}

function fetchBootstrap() {
	// Fetch bootstrap data asynchronously — populates sidebar, models, projects
	// as soon as the data arrives, without blocking the initial page render.
	fetch("/api/bootstrap")
		.then((r) => r.json())
		.then((boot) => {
			if (boot.onboarded === false && location.pathname !== "/settings") {
				navigate("/settings");
				return;
			}
			if (boot.channels) S.setCachedChannels(boot.channels.channels || boot.channels || []);
			if (boot.sessions) {
				S.setSessions(boot.sessions || []);
				renderSessionList();
			}
			if (boot.models) applyModels(boot.models);
			if (boot.projects) {
				S.setProjects(boot.projects || []);
				renderProjectSelect();
				renderSessionProjectSelect();
			}
			S.setSandboxInfo(boot.sandbox || null);
		})
		.catch(() => {
			/* WS connect will fetch this data anyway */
		});
}

function startApp() {
	mount(location.pathname);
	connect();
	fetchBootstrap();
}
