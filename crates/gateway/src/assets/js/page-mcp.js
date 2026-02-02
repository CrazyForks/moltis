// ── MCP Servers page ─────────────────────────────────────────

import { signal, useSignal } from "@preact/signals";
import { html } from "htm/preact";
import { render } from "preact";
import { useEffect } from "preact/hooks";
import { sendRpc } from "./helpers.js";
import { registerPage } from "./router.js";
import * as S from "./state.js";

// ── Signals ─────────────────────────────────────────────────
var servers = signal([]);
var loading = signal(false);

async function refreshServers() {
	loading.value = true;
	var res = await sendRpc("mcp.list", {});
	if (res.ok) {
		servers.value = res.result || [];
	}
	loading.value = false;
}

// ── Components ──────────────────────────────────────────────

function StatusBadge({ state }) {
	var colors = {
		running: "var(--color-success, #22c55e)",
		stopped: "var(--color-muted, #888)",
		dead: "var(--color-error, #ef4444)",
		connecting: "var(--color-warning, #f59e0b)",
	};
	var color = colors[state] || colors.stopped;
	return html`<span
		style="display:inline-block;width:8px;height:8px;border-radius:50%;background:${color};margin-right:6px"
	></span>`;
}

function ServerCard({ server }) {
	var expanded = useSignal(false);
	var tools = useSignal(null);

	async function toggleTools() {
		expanded.value = !expanded.value;
		if (expanded.value && !tools.value) {
			var res = await sendRpc("mcp.tools", { name: server.name });
			if (res.ok) tools.value = res.result || [];
		}
	}

	async function toggleEnabled() {
		var method = server.enabled ? "mcp.disable" : "mcp.enable";
		await sendRpc(method, { name: server.name });
		await refreshServers();
	}

	async function restart() {
		await sendRpc("mcp.restart", { name: server.name });
		await refreshServers();
	}

	async function remove() {
		if (!confirm(`Remove MCP server '${server.name}'?`)) return;
		await sendRpc("mcp.remove", { name: server.name });
		await refreshServers();
	}

	return html`
		<div
			style="border:1px solid var(--color-border, #333);border-radius:8px;padding:12px;margin-bottom:8px"
		>
			<div
				style="display:flex;align-items:center;justify-content:space-between"
			>
				<div
					style="display:flex;align-items:center;gap:8px;cursor:pointer"
					onClick=${toggleTools}
				>
					<${StatusBadge} state=${server.state} />
					<strong>${server.name}</strong>
					<span style="color:var(--color-muted,#888);font-size:0.85em"
						>${server.tool_count} tool${server.tool_count !== 1 ? "s" : ""}</span
					>
				</div>
				<div style="display:flex;gap:6px">
					<button class="btn btn-sm" onClick=${toggleEnabled}>
						${server.enabled ? "Disable" : "Enable"}
					</button>
					<button
						class="btn btn-sm"
						onClick=${restart}
						disabled=${!server.enabled}
					>
						Restart
					</button>
					<button class="btn btn-sm btn-danger" onClick=${remove}>
						Remove
					</button>
				</div>
			</div>
			${
				expanded.value &&
				tools.value &&
				html`
				<div style="margin-top:8px;padding-left:20px">
					${tools.value.map(
						(t) => html`
							<div style="margin-bottom:4px">
								<code>${t.name}</code>
								${
									t.description &&
									html`<span
									style="color:var(--color-muted,#888);margin-left:8px"
									>${t.description}</span
								>`
								}
							</div>
						`,
					)}
					${
						tools.value.length === 0 &&
						html`<em style="color:var(--color-muted,#888)"
						>No tools</em
					>`
					}
				</div>
			`
			}
		</div>
	`;
}

function AddServerForm({ onAdded }) {
	var name = useSignal("");
	var command = useSignal("");
	var args = useSignal("");
	var adding = useSignal(false);

	async function handleSubmit(e) {
		e.preventDefault();
		if (!(name.value && command.value)) return;
		adding.value = true;
		var argsList = args.value ? args.value.split(/\s+/).filter(Boolean) : [];
		await sendRpc("mcp.add", {
			name: name.value,
			command: command.value,
			args: argsList,
		});
		name.value = "";
		command.value = "";
		args.value = "";
		adding.value = false;
		onAdded();
	}

	return html`
		<form
			onSubmit=${handleSubmit}
			style="display:flex;gap:8px;align-items:end;flex-wrap:wrap;margin-bottom:16px"
		>
			<label style="display:flex;flex-direction:column;gap:2px;font-size:0.85em">
				Name
				<input
					class="input"
					value=${name.value}
					onInput=${(e) => (name.value = e.target.value)}
					placeholder="filesystem"
					style="width:140px"
				/>
			</label>
			<label style="display:flex;flex-direction:column;gap:2px;font-size:0.85em">
				Command
				<input
					class="input"
					value=${command.value}
					onInput=${(e) => (command.value = e.target.value)}
					placeholder="npx -y @modelcontextprotocol/server-filesystem"
					style="width:340px"
				/>
			</label>
			<label style="display:flex;flex-direction:column;gap:2px;font-size:0.85em">
				Args
				<input
					class="input"
					value=${args.value}
					onInput=${(e) => (args.value = e.target.value)}
					placeholder="/path/to/dir"
					style="width:200px"
				/>
			</label>
			<button class="btn" type="submit" disabled=${adding.value}>
				Add
			</button>
		</form>
	`;
}

function McpPage() {
	useEffect(() => {
		refreshServers();
	}, []);

	return html`
		<div style="padding:20px;max-width:800px;overflow-y:auto;flex:1">
			<h2 style="margin-bottom:16px">MCP Servers</h2>
			<${AddServerForm} onAdded=${refreshServers} />
			${loading.value && servers.value.length === 0 && html`<p style="color:var(--color-muted,#888)">Loading...</p>`}
			${
				!loading.value &&
				servers.value.length === 0 &&
				html`<p style="color:var(--color-muted,#888)">
				No MCP servers configured. Add one above.
			</p>`
			}
			${servers.value.map((s) => html`<${ServerCard} key=${s.name} server=${s} />`)}
		</div>
	`;
}

// ── Router integration ──────────────────────────────────────
registerPage(
	"/mcp",
	function initMcp(container) {
		container.style.cssText = "flex-direction:column;padding:0;overflow:hidden;";
		render(html`<${McpPage} />`, container);
	},
	function teardownMcp() {
		var container = S.$("pageContent");
		if (container) render(null, container);
	},
);
