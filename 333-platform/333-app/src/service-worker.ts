// KG: TASK_333_INT_E2E, SPAN_333_Offline
// SvelteKit Service Worker — WASM cache-first, offline-first
// Taliban F4: Service Worker for PWA offline capability

/// <reference types="@sveltejs/kit" />
import { build, files, version } from '$service-worker';

const CACHE_NAME = `333-cache-${version}`;
const WASM_ASSETS = ['/333/wasm/triple_three.js', '/333/wasm/triple_three_bg.wasm'];
const STATIC_ASSETS = [...build, ...files, ...WASM_ASSETS];

// Install: pre-cache WASM + static assets
self.addEventListener('install', (event: ExtendableEvent) => {
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) => cache.addAll(STATIC_ASSETS))
  );
  (self as any).skipWaiting();
});

// Activate: clean old caches
self.addEventListener('activate', (event: ExtendableEvent) => {
  event.waitUntil(
    caches.keys().then((names) =>
      Promise.all(
        names.filter((n) => n !== CACHE_NAME).map((n) => caches.delete(n))
      )
    )
  );
  (self as any).clients.claim();
});

// Fetch: cache-first for WASM + static, network-first for navigation
self.addEventListener('fetch', (event: FetchEvent) => {
  const { request } = event;

  // WASM + static: cache-first (rarely changes)
  if (request.url.includes('/wasm/') || STATIC_ASSETS.some(a => request.url.includes(a))) {
    event.respondWith(
      caches.match(request).then((cached) => cached || fetch(request))
    );
    return;
  }

  // Navigation: network-first (fresh HTML, offline fallback)
  if (request.mode === 'navigate') {
    event.respondWith(
      fetch(request).catch(() => caches.match(request) as Promise<Response>)
    );
    return;
  }

  // Everything else: network with cache fallback
  event.respondWith(
    fetch(request).catch(() => caches.match(request) as Promise<Response>)
  );
});
