// ── Projects page (Preact + HTM + Signals) ──────────────────

import { signal } from "@preact/signals";
import { html } from "htm/preact";
import { render } from "preact";
import { useEffect, useRef } from "preact/hooks";
import { sendRpc } from "./helpers.js";
import { fetchProjects } from "./projects.js";
import { registerPage } from "./router.js";
import { projects as projectsSig } from "./signals.js";
import * as S from "./state.js";

var completions = signal([]);
var editingProject = signal(null);
var detecting = signal(false);

function PathInput(props) {
	var inputRef = useRef(null);
	var timerRef = useRef(null);

	function onInput() {
		clearTimeout(timerRef.current);
		timerRef.current = setTimeout(() => {
			var val = inputRef.current?.value || "";
			if (val.length < 2) {
				completions.value = [];
				return;
			}
			sendRpc("projects.complete_path", { partial: val }).then((res) => {
				if (!res?.ok) {
					completions.value = [];
					return;
				}
				completions.value = res.payload || [];
			});
		}, 200);
	}

	function selectPath(p) {
		if (inputRef.current) {
			inputRef.current.value = `${p}/`;
			inputRef.current.focus();
		}
		completions.value = [];
		onInput();
	}

	return html`<div class="project-dir-group">
    <div class="text-xs text-[var(--muted)]" style="margin-bottom:4px;">Directory</div>
    <input ref=${inputRef} type="text" class="provider-key-input"
      placeholder="/path/to/project" style="font-family:var(--font-mono);width:100%;"
      onInput=${onInput} />
    ${
			completions.value.length > 0 &&
			html`
      <div class="project-completion" style="display:block;">
        ${completions.value.map(
					(p) => html`
          <div key=${p} class="project-completion-item" onClick=${() => selectPath(p)}>${p}</div>
        `,
				)}
      </div>
    `
		}
    <button class="bg-[var(--accent-dim)] text-white border-none px-3 py-1.5 rounded text-xs cursor-pointer hover:bg-[var(--accent)] transition-colors"
      style="height:34px;margin-top:8px;"
      onClick=${() => {
				var dir = inputRef.current?.value.trim();
				if (!dir) return;
				props.onAdd(dir).then(() => {
					if (inputRef.current) inputRef.current.value = "";
				});
			}}>Add</button>
  </div>`;
}

function ProjectEditForm(props) {
	var p = props.project;
	var labelRef = useRef(null);
	var dirRef = useRef(null);
	var promptRef = useRef(null);
	var setupRef = useRef(null);
	var teardownRef = useRef(null);
	var prefixRef = useRef(null);
	var wtRef = useRef(null);

	function onSave() {
		var updated = JSON.parse(JSON.stringify(p));
		updated.label = labelRef.current?.value.trim() || p.label;
		updated.directory = dirRef.current?.value.trim() || p.directory;
		updated.system_prompt = promptRef.current?.value.trim() || null;
		updated.setup_command = setupRef.current?.value.trim() || null;
		updated.teardown_command = teardownRef.current?.value.trim() || null;
		updated.branch_prefix = prefixRef.current?.value.trim() || null;
		updated.auto_worktree = wtRef.current?.checked;
		updated.updated_at = Date.now();
		sendRpc("projects.upsert", updated).then(() => {
			editingProject.value = null;
			fetchProjects();
		});
	}

	function field(label, ref, value, placeholder, mono) {
		return html`<div class="project-edit-group">
      <div class="text-xs text-[var(--muted)] project-edit-label">${label}</div>
      <input ref=${ref} type="text" class="provider-key-input"
        value=${value || ""} placeholder=${placeholder || ""}
        style=${mono ? "font-family:var(--font-mono);width:100%;" : "width:100%;"} />
    </div>`;
	}

	return html`<div class="project-edit-form">
    ${field("Label", labelRef, p.label, "Project name")}
    ${field("Directory", dirRef, p.directory, "/path/to/project", true)}
    <div class="project-edit-group">
      <div class="text-xs text-[var(--muted)] project-edit-label">System prompt (optional)</div>
      <textarea ref=${promptRef} class="provider-key-input"
        placeholder="Extra instructions for the LLM when working on this project..."
        style="width:100%;min-height:60px;resize-y;font-size:.8rem;">${p.system_prompt || ""}</textarea>
    </div>
    ${field("Setup command", setupRef, p.setup_command, "e.g. pnpm install", true)}
    ${field("Teardown command", teardownRef, p.teardown_command, "e.g. docker compose down", true)}
    ${field("Branch prefix", prefixRef, p.branch_prefix, "default: moltis", true)}
    <div style="margin-bottom:10px;display:flex;align-items:center;gap:8px;">
      <input ref=${wtRef} type="checkbox" checked=${p.auto_worktree} />
      <span class="text-xs text-[var(--text)]">Auto-create git worktree per session</span>
    </div>
    <div style="display:flex;gap:8px;">
      <button class="provider-btn" onClick=${onSave}>Save</button>
      <button class="provider-btn provider-btn-secondary" onClick=${() => {
				editingProject.value = null;
			}}>Cancel</button>
    </div>
  </div>`;
}

function ProjectCard(props) {
	var p = props.project;

	function onDelete() {
		sendRpc("projects.delete", { id: p.id }).then(() => fetchProjects());
	}

	return html`<div class="provider-item" style="margin-bottom:6px;">
    <div style="flex:1;min-width:0;">
      <div class="flex items-center gap-2">
        <div class="provider-item-name">${p.label || p.id}</div>
        ${p.detected && html`<span class="provider-item-badge api-key">auto</span>`}
        ${p.auto_worktree && html`<span class="provider-item-badge oauth">worktree</span>`}
        ${p.setup_command && html`<span class="provider-item-badge api-key">setup</span>`}
        ${p.teardown_command && html`<span class="provider-item-badge api-key">teardown</span>`}
        ${p.branch_prefix && html`<span class="provider-item-badge oauth">${p.branch_prefix}/*</span>`}
      </div>
      <div style="font-size:.72rem;color:var(--muted);font-family:var(--font-mono);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;margin-top:2px;">
        ${p.directory}
      </div>
      ${
				p.system_prompt &&
				html`<div style="font-size:.7rem;color:var(--muted);margin-top:2px;font-style:italic;">
        System prompt: ${p.system_prompt.substring(0, 80)}${p.system_prompt.length > 80 ? "..." : ""}
      </div>`
			}
    </div>
    <div style="display:flex;gap:4px;flex-shrink:0;">
      <button class="session-action-btn" title="Edit project" onClick=${() => {
				editingProject.value = p.id;
			}}>edit</button>
      <button class="session-action-btn session-delete" title="Remove project" onClick=${onDelete}>x</button>
    </div>
  </div>`;
}

function ProjectsPage() {
	useEffect(() => {
		sendRpc("projects.list", {}).then((res) => {
			if (res?.ok) S.setProjects(res.payload || []);
		});
	}, []);

	function onAdd(dir) {
		return sendRpc("projects.detect", { directories: [dir] }).then((res) => {
			if (res?.ok) {
				var detected = res.payload || [];
				if (detected.length === 0) {
					var slug = dir.split("/").filter(Boolean).pop() || "project";
					var now = Date.now();
					return sendRpc("projects.upsert", {
						id: slug.toLowerCase().replace(/[^a-z0-9-]/g, "-"),
						label: slug,
						directory: dir,
						auto_worktree: false,
						detected: false,
						created_at: now,
						updated_at: now,
					}).then(() => fetchProjects());
				}
				fetchProjects();
			}
		});
	}

	function onDetect() {
		detecting.value = true;
		sendRpc("projects.detect", { directories: [] }).then(() => {
			detecting.value = false;
			fetchProjects();
		});
	}

	var list = projectsSig.value;

	return html`
    <div class="flex-1 flex flex-col min-w-0 p-4 gap-4 overflow-y-auto">
      <div class="flex items-center gap-3">
        <h2 class="text-lg font-medium text-[var(--text-strong)]">Projects</h2>
        <button class="text-xs text-[var(--muted)] border border-[var(--border)] px-2.5 py-1 rounded-md hover:text-[var(--text)] hover:border-[var(--border-strong)] transition-colors cursor-pointer bg-transparent"
          onClick=${onDetect} disabled=${detecting.value}>
          ${detecting.value ? "Detecting\u2026" : "Auto-detect"}
        </button>
      </div>
      <p class="text-xs text-[var(--muted)] leading-relaxed" style="max-width:600px;margin:0;">
        Projects bind sessions to a codebase directory. When a session is linked to a project, context files (CLAUDE.md, AGENTS.md) are loaded automatically and a custom system prompt can be injected. Enable auto-worktree to give each session its own git branch for isolated work.
      </p>
      <div class="project-form-row">
        <${PathInput} onAdd=${onAdd} />
      </div>
      <div style="max-width:600px;margin-top:8px;">
        ${
					list.length === 0 &&
					html`
          <div class="text-xs text-[var(--muted)]" style="padding:12px 0;">
            No projects configured. Add a directory above or use auto-detect.
          </div>
        `
				}
        ${list.map((p) =>
					editingProject.value === p.id
						? html`<${ProjectEditForm} key=${p.id} project=${p} />`
						: html`<${ProjectCard} key=${p.id} project=${p} />`,
				)}
      </div>
    </div>
  `;
}

registerPage(
	"/projects",
	function initProjects(container) {
		container.style.cssText = "flex-direction:column;padding:0;overflow:hidden;";
		editingProject.value = null;
		completions.value = [];
		detecting.value = false;
		render(html`<${ProjectsPage} />`, container);
	},
	function teardownProjects() {
		var container = S.$("pageContent");
		if (container) render(null, container);
	},
);
