/**
 * Shared utilities for WCES triage dashboards.
 * Included synchronously in <head> by ui/triage.html and docs/triage-ui/triage.html:
 *   <script src="/ui/js/triage-common.js"></script>
 *
 * The function declaration is hoisted, so escapeHtml is available to all
 * inline <script> blocks below this tag (no DOMContentLoaded needed).
 */

/**
 * Escape HTML special characters for safe innerHTML and attribute insertion.
 *
 * Encodes: & < > ' "  (full HTML-attribute-safe set)
 * SAFE for: text content, quoted attribute values, inline onclick handlers
 *
 * - null/undefined → '' (empty string)
 * - everything else → fully encoded via textContent + quote replacement
 */
function escapeHtml(str) {
    if (str === null || str === undefined) return '';
    var div = document.createElement('div');
    div.textContent = String(str);
    // textContent handles & < >  —  we add ' " for attribute safety (XSS fix)
    return div.innerHTML.replace(/'/g, '&#39;').replace(/"/g, '&quot;');
}
