import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';
import istanbul from 'vite-plugin-istanbul';
import { svelteTesting } from '@testing-library/svelte/vite';

// `static-dist/askama-common.css` is emitted by `scripts/emit-askama-common.mjs`,
// wired into `package.json` as a `postbuild` step. That runs AFTER
// `@sveltejs/adapter-static` finalises `static-dist/`, avoiding the
// wipe-and-copy race that would eat any file a `writeBundle` hook wrote
// during the Vite build phase. See the script header for the pipeline
// rationale and the single-source-of-truth invariant it preserves.

// Backend dev server (cargo run) — the Vite dev server proxies API/protocol
// traffic here so cookies, CSRF, and the auth-refresh flow are same-origin.
const BACKEND = process.env.OXICLOUD_BACKEND ?? 'http://localhost:8086';

// When COVERAGE=1, instrument the app source with Istanbul so Playwright e2e
// runs can read `window.__coverage__` and report SvelteKit code coverage. Off
// by default so normal dev/release builds carry no instrumentation overhead.
const COVERAGE = process.env.COVERAGE === '1';

const proxy = {
	'/api': { target: BACKEND, changeOrigin: true },
	'/locales': { target: BACKEND, changeOrigin: true },
	'/.well-known': { target: BACKEND, changeOrigin: true },
	'/remote.php': { target: BACKEND, changeOrigin: true },
	'/ocs': { target: BACKEND, changeOrigin: true },
	'/status.php': { target: BACKEND, changeOrigin: true },
	'/webdav': { target: BACKEND, changeOrigin: true },
	'/caldav': { target: BACKEND, changeOrigin: true },
	'/carddav': { target: BACKEND, changeOrigin: true },
	'/wopi': { target: BACKEND, changeOrigin: true },
	'/magic': { target: BACKEND, changeOrigin: true }
};

export default defineConfig({
	plugins: [
		sveltekit(),
		// Compiles Svelte components in client mode for Vitest component tests
		// (so onMount etc. run); a no-op outside the test runner.
		svelteTesting(),
		...(COVERAGE
			? [
					istanbul({
						include: 'src/**/*.{ts,svelte}',
						exclude: ['node_modules', 'src/**/*.{test,spec}.{js,ts}'],
						extension: ['.ts', '.svelte'],
						requireEnv: false,
						forceBuildInstrument: true
					})
				]
			: [])
	],
	server: {
		port: 5173,
		proxy
	},
	test: {
		environment: 'jsdom',
		// vitest-coverage.ts is a no-op unless COVERAGE=1; it collects Istanbul
		// coverage into tests/e2e/.nyc_output_unit for the combined report.
		setupFiles: ['./vitest-setup.ts', './vitest-coverage.ts'],
		include: ['src/**/*.{test,spec}.{js,ts}'],
		globals: true
	}
});
