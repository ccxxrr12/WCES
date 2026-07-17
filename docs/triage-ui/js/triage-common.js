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
 * LOW-2 fix: previous comment claimed quotes were NOT encoded, but the
 * implementation has always encoded both ' and " (lines 27-28 below).
 * Updated to reflect actual behavior.
 *
 * SAFE for:
 *   - Text content between tags
 *   - Attribute values (single- or double-quoted), since both ' and "
 *     are encoded to their numeric entity forms
 *
 * The encoding chain is:
 *   1. textContent assignment encodes & < > (browser-standard)
 *   2. innerHTML readback returns the encoded string
 *   3. Additional regex replace encodes ' → &#39; and " → &quot;
 *
 * - null/undefined → '' (empty string)
 * - 0, false, "" → their String representation (safe)
 * - everything else → DOM-encoded + quote-encoded
 */
function escapeHtml(str) {
    if (str === null || str === undefined) return '';
    var div = document.createElement('div');
    div.textContent = String(str);
    // Also encode quotes for safe use in attribute values (onclick handlers etc.)
    return div.innerHTML.replace(/'/g, '&#39;').replace(/"/g, '&quot;');
}
