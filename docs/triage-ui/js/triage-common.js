/**
 * Shared utilities for WCES triage dashboards.
 * Included synchronously in <head> by ui/triage.html and docs/triage-ui/triage.html:
 *   <script src="/ui/js/triage-common.js"></script>
 *
 * The function declaration is hoisted, so escapeHtml is available to all
 * inline <script> blocks below this tag (no DOMContentLoaded needed).
 */

/**
 * Escape HTML special characters for safe innerHTML insertion.
 *
 * SAFE for: text content between tags and inside quoted attribute values
 *   whose content is drawn from a fixed server-side enum (CSS class names).
 * NOT safe for: attribute values that may contain user-controlled double
 *   quotes — textContent mapping does NOT encode " or '.  For those cases
 *   use data-* attributes + getAttribute() instead of inline on* handlers.
 *
 * - null/undefined → '' (empty string)
 * - 0, false, "" → their String representation (safe)
 * - everything else → DOM-encoded via textContent (& < > encoded)
 */
function escapeHtml(str) {
    if (str === null || str === undefined) return '';
    var div = document.createElement('div');
    div.textContent = String(str);
    return div.innerHTML;
}
