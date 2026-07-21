// Self-unregistering stub — replaces the legacy vanilla-frontend
// service worker that shipped with OxiCloud ≤ 0.8.0.
//
// Browsers that installed the old SW keep it registered across upgrades
// and it intercepts every navigation, serving a stale index.html from its
// `oxicloud-cache*` Cache Storage. The stale shell's meta-CSP predates
// the SvelteKit build's inline-script hashes, so hydration is blocked by
// CSP and the app hangs on the spinner. Symptom: infinite loader on
// fresh visits, only cleared by a hard refresh. Ref: issue #560.
//
// SvelteKit itself does NOT register a service worker (no `src/service-worker`
// module exists) — this file exists solely to shepherd upgraders off the
// legacy SW. Browsers on a clean install fetch it, install it, immediately
// unregister it, and the URL stays a 200 for the next visitor with the
// same stale-SW problem.
//
// The install/activate handlers race the browser's normal SW lifecycle;
// `skipWaiting` + `clients.claim` fast-forward through the "waiting" and
// "activating" states so the tab that triggered the update gets reloaded
// with a controller-less document (no SW intercepting fetches) within
// the same page lifetime.

self.addEventListener('install', (event) => {
	event.waitUntil(self.skipWaiting());
});

self.addEventListener('activate', (event) => {
	event.waitUntil(
		(async () => {
			// 1. Drop every Cache Storage bucket the legacy SW may have
			//    populated. We match the `oxicloud-cache*` prefix the old
			//    SW used, plus a defensive wildcard clear if that prefix
			//    was ever changed in a fork/downstream build.
			if (self.caches) {
				const keys = await self.caches.keys();
				await Promise.all(keys.map((k) => self.caches.delete(k)));
			}

			// 2. Unregister this SW. After this the browser will not
			//    invoke `fetch` handlers from this registration on future
			//    navigations.
			await self.registration.unregister();

			// 3. Take control of open clients so we can reload them into
			//    a controller-less state (fresh HTML, matching CSP).
			await self.clients.claim();
			const clients = await self.clients.matchAll({ type: 'window' });
			for (const client of clients) {
				// `navigate` beats `location.reload()`-in-postMessage because
				// it works even if the page's JS is CSP-blocked (the case
				// we're fixing). Same URL → same-tab reload without controller.
				try {
					await client.navigate(client.url);
				} catch {
					/* opaque redirect / cross-origin — nothing we can do */
				}
			}
		})()
	);
});

// Explicit pass-through fetch handler. Without one, browsers may treat
// the SW as controlling — with an empty handler they short-circuit to
// the network. Belt-and-suspenders: we've already unregistered above,
// but a race between activation and an in-flight navigation could still
// hit this handler.
self.addEventListener('fetch', () => {
	/* fall through to network */
});
