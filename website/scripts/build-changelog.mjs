import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const rootDir = path.resolve(__dirname, "..");
const projectRoot = path.resolve(rootDir, "..");
const sourcePath = path.join(projectRoot, "CHANGELOG.md");
const outputDir = path.join(rootDir, "changelog");
const outputPath = path.join(outputDir, "index.html");

function escapeHtml(value) {
	return value
		.replaceAll("&", "&amp;")
		.replaceAll("<", "&lt;")
		.replaceAll(">", "&gt;")
		.replaceAll('"', "&quot;")
		.replaceAll("'", "&#39;");
}

function renderInline(raw) {
	let value = escapeHtml(raw);
	value = value.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" class="text-orange-600 dark:text-orange-400 hover:underline">$1</a>');
	value = value.replace(/`([^`]+)`/g, '<code class="px-1 py-0.5 bg-gray-100 dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded text-[0.85em]">$1</code>');
	value = value.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
	value = value.replace(/\*([^*]+)\*/g, "<em>$1</em>");
	return value;
}

/** Turn a version heading like "[20260327.02] - 2026-03-27" into a URL-safe id. */
function versionSlug(text) {
	const match = text.match(/\[([^\]]+)\]/);
	return match ? match[1] : text.toLowerCase().replace(/[^a-z0-9]+/g, "-");
}

const categoryClasses = {
	added: "bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-400",
	fixed: "bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-400",
	changed: "bg-amber-100 text-amber-700 dark:bg-amber-900/40 dark:text-amber-400",
	removed: "bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-400",
	security: "bg-violet-100 text-violet-700 dark:bg-violet-900/40 dark:text-violet-400",
	deprecated: "bg-stone-100 text-stone-600 dark:bg-stone-800/40 dark:text-stone-400",
};

function renderMarkdown(markdown) {
	const lines = markdown.replace(/\r\n/g, "\n").split("\n");
	const html = [];
	let paragraph = [];
	let inList = false;
	let isUnreleased = false;

	const flushParagraph = () => {
		if (paragraph.length === 0) return;
		const text = paragraph.join(" ").trim();
		if (text) html.push(`<p class="text-sm text-gray-500 dark:text-gray-400 my-1">${renderInline(text)}</p>`);
		paragraph = [];
	};

	const closeList = () => {
		if (!inList) return;
		html.push("</ul>");
		inList = false;
	};

	for (const line of lines) {
		if (line.match(/^#\s+Changelog/i)) continue;
		if (line.match(/^All notable changes/i)) continue;
		if (line.match(/^and this project adheres/i)) continue;

		const heading = line.match(/^(#{1,6})\s+(.+)$/);
		if (heading) {
			flushParagraph();
			closeList();
			const level = heading[1].length;
			const text = heading[2].trim();

			if (level === 2) {
				const slug = versionSlug(text);
				// Check if this is the Unreleased section
				isUnreleased = text.includes("Unreleased");
				const display = text.replace(/^\[([^\]]+)\]/, "$1");
				html.push(`<div class="mt-8 first:mt-0 border-t border-gray-200 dark:border-gray-800 pt-4" id="${escapeHtml(slug)}">`);
				html.push(`<h2 class="font-mono text-lg font-bold text-gray-900 dark:text-white m-0"><a href="#${escapeHtml(slug)}" class="no-underline hover:text-orange-600 dark:hover:text-orange-400">${escapeHtml(display)}</a></h2>`);
				html.push("</div>");
			} else if (level === 3) {
				const category = text.toLowerCase();
				const cls = categoryClasses[category] || "bg-gray-100 text-gray-600 dark:bg-gray-800 dark:text-gray-400";
				html.push(`<span class="inline-block mt-3 mb-1 px-2 py-0.5 rounded text-[0.7rem] font-bold uppercase tracking-wider ${cls}">${escapeHtml(text)}</span>`);
			}
			continue;
		}

		const listItem = line.match(/^\s*-\s+(.+)$/);
		if (listItem) {
			flushParagraph();
			if (!inList) {
				html.push('<ul class="list-none m-0 pl-0 space-y-0.5">');
				inList = true;
			}
			html.push(`<li class="font-mono text-[0.8rem] leading-snug text-gray-600 dark:text-gray-300 pl-4 relative before:content-['\\2022'] before:absolute before:left-0 before:text-gray-400 dark:before:text-gray-600">${renderInline(listItem[1].trim())}</li>`);
			continue;
		}

		if (line.trim() === "") {
			flushParagraph();
			closeList();
			// Skip empty sections inside Unreleased
			continue;
		}

		paragraph.push(line.trim());
	}

	flushParagraph();
	closeList();
	return html.join("\n");
}

function buildHtml(contentHtml) {
	return `<!DOCTYPE html>
<html lang="en">
<head>
    <script>
        (function(){
            var t = localStorage.getItem('theme') || 'system';
            if (t === 'dark' || (t === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)) {
                document.documentElement.classList.add('dark');
            }
        })();
    </script>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light dark">
    <title>Changelog - Moltis</title>
    <meta name="description" content="Release history and changelog for Moltis.">
    <link rel="icon" type="image/svg+xml" href="/favicon.svg">
    <link rel="preconnect" href="https://fonts.googleapis.com">
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
    <link href="https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@400;600;700&family=JetBrains+Mono:wght@400&family=Outfit:wght@700&display=swap" rel="stylesheet">
    <script src="https://cdn.tailwindcss.com"></script>
    <script>
        tailwind.config = {
            darkMode: 'class',
            theme: {
                extend: {
                    fontFamily: {
                        sans: ['Space Grotesk', 'system-ui', 'sans-serif'],
                        mono: ['JetBrains Mono', 'monospace'],
                    },
                },
            },
        }
        function applyTheme(mode) {
            if (mode === 'dark' || (mode === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)) {
                document.documentElement.classList.add('dark');
            } else {
                document.documentElement.classList.remove('dark');
            }
        }
        const savedTheme = localStorage.getItem('theme') || 'system';
        applyTheme(savedTheme);
        window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
            if ((localStorage.getItem('theme') || 'system') === 'system') applyTheme('system');
        });
    </script>
    <style>
        @keyframes pulse-slow {
            0%, 100% { opacity: 0.3; }
            50% { opacity: 0.7; }
        }
        .animate-pulse-slow { animation: pulse-slow 4s ease-in-out infinite; }
        .animation-delay-2s { animation-delay: 2s; }
        .font-outfit { font-family: 'Outfit', sans-serif; }
    </style>
</head>
<body class="bg-white dark:bg-gray-950 text-gray-900 dark:text-white min-h-screen relative transition-colors duration-300 overflow-x-hidden w-full">
    <!-- Background gradient blobs -->
    <div class="absolute inset-0 overflow-hidden pointer-events-none">
        <div class="absolute top-1/4 left-1/4 w-64 h-64 sm:w-96 sm:h-96 bg-orange-300/20 dark:bg-orange-600/15 rounded-full blur-3xl animate-pulse-slow"></div>
        <div class="absolute bottom-1/4 right-1/4 w-64 h-64 sm:w-96 sm:h-96 bg-red-300/20 dark:bg-red-600/15 rounded-full blur-3xl animate-pulse-slow animation-delay-2s"></div>
    </div>

    <!-- Navigation Bar -->
    <nav class="sticky top-0 z-40 bg-white/80 dark:bg-gray-950/80 backdrop-blur-md border-b border-gray-200/50 dark:border-gray-800/50">
        <div class="max-w-4xl mx-auto px-4 sm:px-6">
            <div class="flex items-center justify-between h-14 gap-4">
                <a href="/" class="flex items-center gap-2 shrink-0">
                    <img src="/favicon.svg" alt="Moltis" width="32" height="26">
                    <span class="text-xl sm:text-2xl font-bold bg-gradient-to-r from-orange-500 to-red-500 dark:from-orange-400 dark:to-red-400 bg-clip-text text-transparent font-outfit">Moltis</span>
                </a>
                <div class="flex items-center gap-2">
                    <a href="/" class="text-sm font-medium text-gray-500 dark:text-gray-400 hover:text-orange-600 dark:hover:text-orange-400 transition-colors">Home</a>
                    <span class="text-gray-300 dark:text-gray-700">/</span>
                    <span class="text-sm font-medium text-gray-900 dark:text-white">Changelog</span>
                </div>
                <div class="flex items-center gap-1 shrink-0">
                    <button id="theme-toggle" title="Toggle theme" class="p-1.5 rounded-md text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors">
                        <svg id="theme-icon-light" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="hidden"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
                        <svg id="theme-icon-system" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="hidden"><rect x="2" y="3" width="20" height="14" rx="2" ry="2"/><line x1="8" y1="21" x2="16" y2="21"/><line x1="12" y1="17" x2="12" y2="21"/></svg>
                        <svg id="theme-icon-dark" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="hidden"><path d="M21 12.79A9 9 0 1111.21 3 7 7 0 0021 12.79z"/></svg>
                    </button>
                    <a href="https://github.com/moltis-org/moltis" target="_blank" rel="noopener" class="inline-flex items-center gap-1 px-2.5 py-1 rounded-full border border-gray-200 dark:border-gray-700 text-xs font-medium text-gray-500 dark:text-gray-400 hover:border-gray-300 dark:hover:border-gray-600 hover:text-gray-700 dark:hover:text-gray-300 transition-colors">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z"/></svg>
                        GitHub
                    </a>
                </div>
            </div>
        </div>
    </nav>

    <!-- Content -->
    <main class="relative max-w-4xl mx-auto px-4 sm:px-6 py-8">
        ${contentHtml}
    </main>

    <!-- Theme toggle script -->
    <script>
        const modes = ['system', 'light', 'dark'];
        const icons = { light: 'theme-icon-light', system: 'theme-icon-system', dark: 'theme-icon-dark' };
        let current = localStorage.getItem('theme') || 'system';
        function updateIcon() {
            Object.values(icons).forEach(id => document.getElementById(id).classList.add('hidden'));
            document.getElementById(icons[current]).classList.remove('hidden');
        }
        updateIcon();
        document.getElementById('theme-toggle').addEventListener('click', () => {
            current = modes[(modes.indexOf(current) + 1) % modes.length];
            localStorage.setItem('theme', current);
            applyTheme(current);
            updateIcon();
        });
    </script>
</body>
</html>
`;
}

async function main() {
	const markdown = await readFile(sourcePath, "utf8");
	const contentHtml = renderMarkdown(markdown);
	const html = buildHtml(contentHtml);
	await mkdir(outputDir, { recursive: true });
	await writeFile(outputPath, html, "utf8");
	process.stdout.write(`Built changelog/index.html from CHANGELOG.md\n`);
}

main().catch((error) => {
	process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
	process.exit(1);
});
