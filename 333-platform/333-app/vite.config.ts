import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// KG: sprint7H-coop-coep-sharedarraybuffer-2026-04-16
// SharedArrayBuffer requires COOP + COEP on ALL responses (HTML, JS, WASM, assets).
// Without these headers, browsers refuse to expose SharedArrayBuffer entirely.
const securityHeaders = {
	'Cross-Origin-Opener-Policy': 'same-origin',
	'Cross-Origin-Embedder-Policy': 'require-corp',
	'Cross-Origin-Resource-Policy': 'cross-origin',
};

export default defineConfig({
	plugins: [sveltekit()],
	optimizeDeps: {
		exclude: ['triple_three']
	},
	server: {
		hmr: {
			// KG: TASK_333_E2E_HmrFix — prevent HMR timeout from poll_sync
			timeout: 30000
		},
		// KG: sprint7H-coop-coep-sharedarraybuffer-2026-04-16
		// COOP+COEP required for SharedArrayBuffer in 4-tab P2P test
		headers: securityHeaders,
	},
	preview: {
		// Also apply to `vite preview` (production preview mode)
		headers: securityHeaders,
	}
});
