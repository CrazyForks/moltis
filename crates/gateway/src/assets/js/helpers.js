// ── Helpers ──────────────────────────────────────────────────
import * as S from "./state.js";

export function nextId() {
	S.setReqId(S.reqId + 1);
	return `ui-${S.reqId}`;
}

export function esc(s) {
	return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

export function renderMarkdown(raw) {
	var s = esc(raw);
	s = s.replace(/```(\w*)\n([\s\S]*?)```/g, (_, _lang, code) => `<pre><code>${code}</code></pre>`);
	s = s.replace(/`([^`]+)`/g, "<code>$1</code>");
	s = s.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
	return s;
}

export function sendRpc(method, params) {
	return new Promise((resolve) => {
		var id = nextId();
		S.pending[id] = resolve;
		S.ws.send(JSON.stringify({ type: "req", id: id, method: method, params: params }));
	});
}

export function formatTokens(n) {
	if (n >= 1000000) return `${(n / 1000000).toFixed(1)}M`;
	if (n >= 1000) return `${(n / 1000).toFixed(1)}K`;
	return String(n);
}

export function formatBytes(b) {
	if (b >= 1024) return `${(b / 1024).toFixed(1)} KB`;
	return `${b} B`;
}

function classifyJsonErrorObj(errObj) {
	var resetsAt = errObj.resets_at ? errObj.resets_at * 1000 : null;
	if (errObj.type === "usage_limit_reached" || (errObj.message && errObj.message.indexOf("usage limit") !== -1)) {
		return {
			icon: "",
			title: "Usage limit reached",
			detail: `Your ${errObj.plan_type || "current"} plan limit has been reached.`,
			resetsAt: resetsAt,
		};
	}
	if (errObj.type === "rate_limit_exceeded" || (errObj.message && errObj.message.indexOf("rate limit") !== -1)) {
		return {
			icon: "\u26A0\uFE0F",
			title: "Rate limited",
			detail: errObj.message || "Too many requests. Please wait a moment.",
			resetsAt: resetsAt,
		};
	}
	if (errObj.message) {
		return { icon: "\u26A0\uFE0F", title: "Error", detail: errObj.message, resetsAt: null };
	}
	return null;
}

function parseJsonError(message) {
	var jsonMatch = message.match(/\{[\s\S]*\}$/);
	if (!jsonMatch) return null;
	try {
		var err = JSON.parse(jsonMatch[0]);
		return classifyJsonErrorObj(err.error || err);
	} catch (_e) {
		/* fall through */
	}
	return null;
}

function parseHttpStatusError(message) {
	var statusMatch = message.match(/HTTP (\d{3})/);
	var code = statusMatch ? parseInt(statusMatch[1], 10) : 0;
	if (code === 401 || code === 403)
		return {
			icon: "\uD83D\uDD12",
			title: "Authentication error",
			detail: "Your session may have expired.",
			resetsAt: null,
		};
	if (code === 429)
		return {
			icon: "",
			title: "Rate limited",
			detail: "Too many requests.",
			resetsAt: null,
		};
	if (code >= 500)
		return {
			icon: "\uD83D\uDEA8",
			title: "Server error",
			detail: "The upstream provider returned an error.",
			resetsAt: null,
		};
	return null;
}

export function parseErrorMessage(message) {
	return (
		parseJsonError(message) ||
		parseHttpStatusError(message) || {
			icon: "\u26A0\uFE0F",
			title: "Error",
			detail: message,
			resetsAt: null,
		}
	);
}

export function updateCountdown(el, resetsAtMs) {
	var now = Date.now();
	var diff = resetsAtMs - now;
	if (diff <= 0) {
		el.textContent = "Limit should be reset now \u2014 try again!";
		el.className = "error-countdown reset-ready";
		return true;
	}
	var hours = Math.floor(diff / 3600000);
	var mins = Math.floor((diff % 3600000) / 60000);
	var parts = [];
	if (hours > 0) parts.push(`${hours}h`);
	parts.push(`${mins}m`);
	el.textContent = `Resets in ${parts.join(" ")}`;
	return false;
}

export function createEl(tag, attrs, children) {
	var el = document.createElement(tag);
	if (attrs) {
		Object.keys(attrs).forEach((k) => {
			if (k === "className") el.className = attrs[k];
			else if (k === "textContent") el.textContent = attrs[k];
			else if (k === "style") el.style.cssText = attrs[k];
			else el.setAttribute(k, attrs[k]);
		});
	}
	if (children) {
		children.forEach((c) => {
			if (c) el.appendChild(c);
		});
	}
	return el;
}
