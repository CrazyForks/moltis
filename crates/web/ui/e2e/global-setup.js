// Global setup: verify that each gateway's WebSocket endpoint is accepting
// connections before any test runs.  The Playwright webServer health check
// only validates HTTP — the WS handler may not be ready yet on slow CI runners.

const http = require("node:http");

function waitForWs(baseUrl, timeoutMs = 60_000) {
	const wsUrl = baseUrl.replace(/^http/, "ws") + "/ws";
	const deadline = Date.now() + timeoutMs;

	return new Promise((resolve, reject) => {
		function attempt() {
			if (Date.now() > deadline) {
				reject(new Error(`WS at ${wsUrl} not ready after ${timeoutMs}ms`));
				return;
			}
			// Use a raw HTTP upgrade request to test if the WS endpoint responds.
			const url = new URL(baseUrl + "/ws");
			const req = http.request(
				{
					hostname: url.hostname,
					port: url.port,
					path: "/ws",
					method: "GET",
					timeout: 5000,
					headers: {
						Upgrade: "websocket",
						Connection: "Upgrade",
						"Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
						"Sec-WebSocket-Version": "13",
					},
				},
				(res) => {
					// Any response (101, 403, etc.) means the WS handler is alive
					req.destroy();
					resolve();
				},
			);
			req.on("upgrade", () => {
				req.destroy();
				resolve();
			});
			req.on("error", () => {
				setTimeout(attempt, 1000);
			});
			req.on("timeout", () => {
				req.destroy();
				setTimeout(attempt, 1000);
			});
			req.end();
		}
		attempt();
	});
}

module.exports = async function globalSetup(config) {
	// Wait for each webServer's WS endpoint to be ready
	for (const project of config.projects || []) {
		const baseURL = project.use?.baseURL || config.use?.baseURL;
		if (baseURL) {
			try {
				await waitForWs(baseURL, 60_000);
			} catch (e) {
				console.warn(`[global-setup] ${e.message}`);
			}
		}
	}
};
