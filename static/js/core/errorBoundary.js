// Global error boundary.
//
// Surfaces uncaught errors and unhandled promise rejections as a terse,
// throttled toast instead of failing silently — the full detail always goes to
// the console. Intentionally dependency-free so an error handler can never
// itself fail to load.

let lastShown = 0;

/** @param {string} message */
function showToast(message) {
    const now = Date.now();
    if (now - lastShown < 4000) return; // throttle bursts of related errors
    lastShown = now;

    const el = document.createElement('div');
    el.className = 'error-toast';
    el.setAttribute('role', 'alert');
    el.textContent = message;
    document.body.appendChild(el);

    requestAnimationFrame(() => el.classList.add('is-visible'));
    setTimeout(() => {
        el.classList.remove('is-visible');
        setTimeout(() => el.remove(), 300);
    }, 5000);
}

const GENERIC = 'Something went wrong. Please try again.';

window.addEventListener('error', (event) => {
    console.error('[error-boundary]', event.error || event.message);
    showToast(GENERIC);
});

window.addEventListener('unhandledrejection', (event) => {
    console.error('[error-boundary] unhandled rejection:', event.reason);
    showToast(GENERIC);
});
