/**
 * OxiCloud - Shared format and escaping utilities
 * Centralized global helpers for date/size/text formatting and XSS-safe escaping.
 * Contains also checkers
 */

import { i18n } from './i18n.js';

/**
 *
 * @param {string} str
 * @returns {string}
 */
function escapeHtml(str) {
    if (typeof str !== 'string') return '';
    return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#039;');
}

/**
 *
 * @param {number} bytes
 * @returns {string}
 */
function formatFileSize(bytes) {
    if (bytes === 0) return '0 Bytes';

    const k = 1024;
    const sizes = ['Bytes', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    const num = new Intl.NumberFormat(i18n.getCurrentLocale(), { maximumFractionDigits: 2 }).format(
        bytes / k ** i
    );

    return `${num} ${sizes[i]}`;
}

/// Formats a byte count for quota display. When bytes is 0, returns "∞" (unlimited).
/**
 *
 * @param {number} bytes
 * @returns {string}
 */
function formatQuotaSize(bytes) {
    if (bytes === 0) return '∞';
    return formatFileSize(bytes);
}

/**
 *
 * @param {Date | number| null} value
 * @returns {string}
 */
function formatDateTime(value) {
    if (!value) return '';
    let dateValue;
    if (value instanceof Date) {
        dateValue = value;
    } else if (typeof value === 'number') {
        dateValue = new Date(value < 1e12 ? value * 1000 : value);
    } else {
        dateValue = new Date(value);
    }
    if (Number.isNaN(dateValue.getTime())) return String(value);
    const loc = i18n.getCurrentLocale();
    return `${dateValue.toLocaleDateString(loc)} ${dateValue.toLocaleTimeString(loc, { hour: '2-digit', minute: '2-digit' })}`;
}

/**
 *
 * @param {Date | number| null} value
 * @returns {string}
 */
function formatDateShort(value) {
    if (!value) return 'N/A';
    const dateValue = typeof value === 'number' ? new Date(value * 1000) : new Date(value);
    if (Number.isNaN(dateValue.getTime())) return String(value);
    return dateValue.toLocaleDateString(i18n.getCurrentLocale(), {
        year: 'numeric',
        month: 'short',
        day: 'numeric'
    });
}

/**
 * Locale-aware "time ago" via Intl.RelativeTimeFormat (replaces the per-view
 * hand-rolled timeAgo helpers).
 * @param {Date | number | string | null} value
 * @returns {string}
 */
function formatRelativeTime(value) {
    if (!value) return '';
    const date =
        value instanceof Date
            ? value
            : new Date(typeof value === 'number' && value < 1e12 ? value * 1000 : value);
    if (Number.isNaN(date.getTime())) return String(value);
    const diffSec = Math.round((date.getTime() - Date.now()) / 1000);
    const abs = Math.abs(diffSec);
    const rtf = new Intl.RelativeTimeFormat(i18n.getCurrentLocale(), { numeric: 'auto' });
    /** @type {Array<[Intl.RelativeTimeFormatUnit, number]>} */
    const units = [
        ['year', 31536000],
        ['month', 2592000],
        ['week', 604800],
        ['day', 86400],
        ['hour', 3600],
        ['minute', 60]
    ];
    for (const [unit, secs] of units) {
        if (abs >= secs) return rtf.format(Math.round(diffSec / secs), unit);
    }
    return rtf.format(diffSec, 'second');
}

const TEXT_TYPES = [
    'application/json',
    'application/xml',
    'application/javascript',
    'application/x-sh',
    'application/x-yaml',
    'application/toml',
    'application/x-toml',
    'application/sql'
];
// FIXME: move is to another file
/**
 *
 * @param {string} mimeType
 * @returns {boolean}
 */
function isTextViewable(mimeType) {
    if (!mimeType) return false;
    if (mimeType.startsWith('text/')) return true;

    return TEXT_TYPES.includes(mimeType);
}

/**
 * Chekif an email is valid
 * @param {string} email
 * @returns boolean
 */
function isEmailValid(email) {
    return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email);
}

/**
 * Normalize a date value into a human-readable bucket label.
 * Buckets (newest-first): Today | Last 7 days | Last 30 days | <YYYY>
 *
 * Accepts:
 *  - `string` — ISO-8601 date string (e.g. `granted_at` from the API)
 *  - `number` — Unix timestamp in **seconds** (e.g. `sort_date`, `modified_at`)
 *               Values < 1e12 are treated as seconds; larger values as milliseconds.
 *  - `Date`   — JavaScript Date object
 *
 * @param {string | number | Date} value
 * @returns {string}
 */
function normalizeDateBucket(value) {
    let date;
    if (value instanceof Date) {
        date = value;
    } else if (typeof value === 'number') {
        date = new Date(value < 1e12 ? value * 1000 : value);
    } else {
        date = new Date(value);
    }
    const diffDays = Math.floor((Date.now() - date.getTime()) / 86_400_000);
    if (diffDays === 0) return i18n.t('dateBucket.today', 'Today');
    if (diffDays <= 7) return i18n.t('dateBucket.last7days', 'Last 7 days');
    if (diffDays <= 30) return i18n.t('dateBucket.last30days', 'Last 30 days');
    return String(date.getFullYear());
}

/**
 * Normalize a future expiry value into a human-readable bucket label.
 * Buckets (soonest-first): Expired | Tomorrow | In less than 7 days | In less than 30 days | <YYYY> | No expiration
 *
 * Accepts the same input types as `normalizeDateBucket`.
 *
 * @param {string | number | Date | null | undefined} value
 * @returns {string}
 */
function normalizeExpiryBucket(value) {
    if (value === null || value === undefined) {
        return i18n.t('expiryBucket.noExpiry', 'No expiration');
    }
    /** @type {Date} */
    let date;
    if (value instanceof Date) {
        date = value;
    } else if (typeof value === 'number') {
        date = new Date(value < 1e12 ? value * 1000 : value);
    } else {
        date = new Date(value);
    }
    const daysUntil = Math.floor((date.getTime() - Date.now()) / 86_400_000);
    if (daysUntil < 0) return i18n.t('expiryBucket.expired', 'Expired');
    if (daysUntil === 0) return i18n.t('expiryBucket.today', 'Today');
    if (daysUntil === 1) return i18n.t('expiryBucket.tomorrow', 'Tomorrow');
    if (daysUntil <= 7) return i18n.t('expiryBucket.week', 'In less than 7 days');
    if (daysUntil <= 30) return i18n.t('expiryBucket.month', 'In less than 30 days');
    return String(date.getFullYear());
}

/**
 * Format a future timestamp as a precise "days until" label.
 *
 * Used by the Trash view's date column to surface the remaining lifetime
 * before the retention sweeper purges an item. Unlike `normalizeExpiryBucket`
 * (which produces coarse bucket labels for grouping), this returns an
 * exact-day count so users can see "In 27 days" at a glance.
 *
 * Buckets:
 *   < 0      → Expired
 *   = 0      → Today
 *   = 1      → Tomorrow
 *   > 1      → In N days
 *
 * @param {string | number | Date | null | undefined} value
 * @returns {string}
 */
function formatDaysRemaining(value) {
    if (value === null || value === undefined) return '';
    /** @type {Date} */
    let date;
    if (value instanceof Date) {
        date = value;
    } else if (typeof value === 'number') {
        date = new Date(value < 1e12 ? value * 1000 : value);
    } else {
        date = new Date(value);
    }
    if (Number.isNaN(date.getTime())) return String(value);

    const daysUntil = Math.floor((date.getTime() - Date.now()) / 86_400_000);
    if (daysUntil < 0) return i18n.t('daysRemaining.expired', 'Expired');
    if (daysUntil === 0) return i18n.t('daysRemaining.today', 'Today');
    if (daysUntil === 1) return i18n.t('daysRemaining.tomorrow', 'Tomorrow');
    // Translation file holds the `{{count}}` template; the fallback is only
    // used pre-load and embeds the literal count.
    const translated = i18n.t('daysRemaining.inDays', { count: daysUntil });
    return translated === 'daysRemaining.inDays' ? `${daysUntil} days` : translated;
}

/**
 * Format a date as a compact "Mar 5, 2026" label.
 *
 * Shared helper used by `formatExpiryChip` (read-only chip) and by
 * `buildExpiryChip` in `utils/expiryChip.js` (interactive editor) so the
 * displayed deadline reads identically across both surfaces.
 *
 * Accepts:
 *   - `string` YYYY-MM-DD — parsed at LOCAL midnight (avoids the off-by-one
 *      shift `new Date("2026-05-30")` causes in negative-offset zones).
 *   - `string` ISO-8601 with time — parsed as-is.
 *   - `number` Unix seconds/ms (auto-detected at 1e12).
 *   - `Date` object — used directly.
 *
 * @param {string | number | Date | null | undefined} value
 * @returns {string}
 */
function formatExpiryDate(value) {
    if (value === null || value === undefined) return '';
    /** @type {Date} */
    let date;
    if (value instanceof Date) {
        date = value;
    } else if (typeof value === 'number') {
        date = new Date(value < 1e12 ? value * 1000 : value);
    } else if (/^\d{4}-\d{2}-\d{2}$/.test(value)) {
        // Bare YYYY-MM-DD: pin to local midnight so the day doesn't shift.
        date = new Date(`${value}T00:00:00`);
    } else {
        date = new Date(value);
    }
    if (Number.isNaN(date.getTime())) return String(value);
    return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
}

/**
 * Render an expiry / retention date as a tiered chip.
 *
 * Single shared formatter used by My Shares (link expiry), Trash
 * (remaining lifetime before purge), and any other section that needs
 * to surface a future deadline. Output:
 *
 *     `<span class="expiry-chip expiry-chip--{tier}"><i class="..."></i>LABEL</span>`
 *
 * Six tiers escalate cool → hot:
 *   - null value     → `never`    (neutral, infinity icon, "Never")
 *   - > 30 days      → `normal`   (neutral grey, "Until DATE")
 *   - 8–30 days      → `caution`  (soft amber, "N days")
 *   - 2–7 days       → `soon`     (soft orange, "N days")
 *   - 0–1 days       → `urgent`   (soft red, "Today" / "Tomorrow")
 *   - past deadline  → `expired`  (deeper red, warning icon, "Expired")
 *
 * The CSS lives in `components/expiryChip.css`.
 * Caller is responsible for embedding the returned HTML safely — the
 * label is taken from a controlled i18n key set (no user input).
 *
 * @param {string | number | Date | null | undefined} value
 * @returns {string} HTML snippet
 */
function formatExpiryChip(value) {
    // null/undefined is a valid input meaning "no deadline".
    if (value === null || value === undefined) {
        const label = i18n.t('expiryChip.never', 'Never expires');
        return `<span class="expiry-chip expiry-chip--never"><i class="fas fa-infinity expiry-chip__icon"></i>${escapeHtml(label)}</span>`;
    }

    /** @type {Date} */
    let date;
    if (value instanceof Date) {
        date = value;
    } else if (typeof value === 'number') {
        date = new Date(value < 1e12 ? value * 1000 : value);
    } else {
        date = new Date(value);
    }
    if (Number.isNaN(date.getTime())) return escapeHtml(String(value));

    const daysUntil = Math.floor((date.getTime() - Date.now()) / 86_400_000);

    let tier;
    let icon;
    let label;
    if (daysUntil < 0) {
        tier = 'expired';
        icon = 'fa-exclamation-triangle';
        label = i18n.t('expiryChip.expired', 'Expired');
    } else if (daysUntil === 0) {
        tier = 'urgent';
        icon = 'fa-clock';
        label = i18n.t('expiryChip.today', 'Expires today');
    } else if (daysUntil === 1) {
        tier = 'urgent';
        icon = 'fa-clock';
        label = i18n.t('expiryChip.tomorrow', 'Expires tomorrow');
    } else if (daysUntil <= 7) {
        tier = 'soon';
        icon = 'fa-calendar';
        const translated = i18n.t('expiryChip.inDays', { count: daysUntil });
        label = translated === 'expiryChip.inDays' ? `Expires in ${daysUntil} days` : translated;
    } else if (daysUntil <= 30) {
        tier = 'caution';
        icon = 'fa-calendar';
        const translated = i18n.t('expiryChip.inDays', { count: daysUntil });
        label = translated === 'expiryChip.inDays' ? `Expires in ${daysUntil} days` : translated;
    } else {
        // Far future — show absolute date so users see the exact deadline.
        tier = 'normal';
        icon = 'fa-calendar';
        const fmt = formatExpiryDate(date);
        const translated = i18n.t('expiryChip.onDate', { date: fmt });
        label = translated === 'expiryChip.onDate' ? `Expires ${fmt}` : translated;
    }

    return `<span class="expiry-chip expiry-chip--${tier}"><i class="fas ${icon} expiry-chip__icon"></i>${escapeHtml(label)}</span>`;
}

/**
 * Maps a file size in bytes to a coarse, human-readable bucket label.
 *
 * Pass `-1` for folders — they sort before all files on the server and
 * receive the "Folders" label client-side.
 *
 * Buckets:
 *   -1                       → Folders
 *    0                       → Empty (0 B)
 *    1 – 1 048 575           → < 1 MB
 *    1 048 576 – 104 857 599 → 1 – 100 MB
 *    104 857 600 – 1 073 741 823 → 100 MB – 1 GB
 *    1 073 741 824 – 5 368 709 119 → 1 – 5 GB
 *    ≥ 5 368 709 120         → > 5 GB
 *
 * @param {number} bytes  File size in bytes, or -1 for folders.
 * @returns {string}
 */
// biome-ignore format: keep the following indent
function sizeBucket(bytes) {
    if (bytes < 0)                       return i18n.t('sizeBucket.folders', 'Folders');
    if (bytes === 0)                     return i18n.t('sizeBucket.empty',   'Empty (0 B)');
    if (bytes < 1_048_576)               return i18n.t('sizeBucket.tiny',    '< 1 MB');
    if (bytes < 104_857_600)             return i18n.t('sizeBucket.small',   '1 – 100 MB');
    if (bytes < 1_073_741_824)           return i18n.t('sizeBucket.medium',  '100 MB – 1 GB');
    if (bytes < 5 * 1_073_741_824)       return i18n.t('sizeBucket.large',   '1 – 5 GB');
    return                                      i18n.t('sizeBucket.huge',    '> 5 GB');
}

export {
    escapeHtml,
    formatDateShort,
    formatDateTime,
    formatDaysRemaining,
    formatExpiryChip,
    formatExpiryDate,
    formatFileSize,
    formatQuotaSize,
    formatRelativeTime,
    isEmailValid,
    isTextViewable,
    normalizeDateBucket,
    normalizeExpiryBucket,
    sizeBucket
};
