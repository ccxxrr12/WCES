//! Path sanitization utilities shared across route handlers.

use std::path::Path;

/// Sanitize a user-supplied path segment, preventing directory traversal.
///
/// Extracts the final file-name component and rejects empty results,
/// path separators, and the special entries `.` and `..` (which
/// `Path::file_name()` treats as ordinary names).
///
/// Returns `Ok(&str)` pointing into the original `id` string when safe.
pub(crate) fn sanitize_path_segment(id: &str) -> Result<&str, &'static str> {
    let safe = Path::new(id)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");

    // Reject: empty, path-separator mismatch, or special entries "." / ".."
    if safe.is_empty() || safe != id || safe == "." || safe == ".." {
        Err("invalid path segment")
    } else {
        Ok(safe)
    }
}
