// E2E test compatibility shim.
//
// With Vite bundling, individual modules are no longer served. The real
// helpers module lives inside the bundle but is exposed on
// window.__moltis_modules["helpers"] from main.tsx.
//
// This shim re-exports everything the e2e tests need.

const M = window.__moltis_modules?.["helpers"] || {};

export default M;

export const localizeStructuredError = (...args) => M.localizeStructuredError?.(...args);
export const formatAssistantTokenUsage = (...args) => M.formatAssistantTokenUsage?.(...args);
export const formatTokens = (...args) => M.formatTokens?.(...args);
export const formatBytes = (...args) => M.formatBytes?.(...args);
// Wrap sendRpc with a 30s timeout so E2E tests don't hang forever
// if the WS connection drops between waitForWsConnected and the RPC call.
export const sendRpc = (...args) => {
	var result = M.sendRpc?.(...args);
	if (!result || typeof result.then !== "function") return result;
	return Promise.race([
		result,
		new Promise((_, reject) =>
			setTimeout(() => reject(new Error("WebSocket disconnected (RPC timeout)")), 30_000),
		),
	]);
};
export const renderMarkdown = (...args) => M.renderMarkdown?.(...args);
export const esc = (...args) => M.esc?.(...args);
export const toolCallSummary = (...args) => M.toolCallSummary?.(...args);
export const formatAudioDuration = (...args) => M.formatAudioDuration?.(...args);
