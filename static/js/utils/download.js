// @ts-check

/**
 * Browser-native download helpers.
 *
 * Downloads are handed to the browser as same-origin navigations: the
 * response streams straight to disk with the browser's own progress UI,
 * and auth cookies travel automatically. Nothing is buffered in page
 * memory — unlike the old `fetch → blob → objectURL` pattern, which
 * materialized the entire payload in the tab's heap before the save
 * dialog could even appear.
 */

/**
 * Trigger a browser-native download for a same-origin URL.
 *
 * @param {string} url - Same-origin URL of the resource to download
 * @param {string} [filename] - Suggested file name. The server's
 *   `Content-Disposition` filename wins when present; an empty string
 *   keeps whatever the server (or URL) provides.
 */
export function triggerBrowserDownload(url, filename = '') {
    const link = document.createElement('a');
    link.href = url;
    link.download = filename;
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
}

/**
 * Build the GET URL for the batch ZIP download endpoint
 * (`GET /api/batch/download` accepts comma-separated id lists).
 * Shared by the batch toolbar download and the drag-out `DownloadURL`
 * builder so both stay in sync with the endpoint's query contract.
 *
 * @param {string[]} fileIds
 * @param {string[]} folderIds
 * @returns {string} Root-relative URL (prepend `window.location.origin`
 *   when an absolute URL is required, e.g. for `DataTransfer.setData`).
 */
export function buildBatchDownloadUrl(fileIds, folderIds) {
    return `/api/batch/download?file_ids=${fileIds.join(',')}&folder_ids=${folderIds.join(',')}`;
}
