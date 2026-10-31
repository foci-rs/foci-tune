//! The embedded web UI.

use std::io::{self, Write};

use rust_embed::RustEmbed;

/// Marker the build script writes into the placeholder page it emits when the web build is
/// skipped.
pub const STUB_MARKER: &str = "<!-- foci-tune-ui-skipped -->";

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
#[exclude = ".gitkeep"]
struct Dist;

/// An embedded file and its MIME type.
pub struct Asset {
    /// File contents.
    pub data: std::borrow::Cow<'static, [u8]>,
    /// MIME type guessed from the file name.
    pub mime: String,
}

/// Looks up an embedded file by its path relative to the web build root.
pub fn get(path: &str) -> Option<Asset> {
    let file = Dist::get(path.trim_start_matches('/'))?;
    Some(Asset {
        mime: file.metadata.mimetype().to_owned(),
        data: file.data,
    })
}

/// The embedded `index.html`, if the build produced one.
pub fn index_html() -> Option<Asset> {
    get("index.html")
}

/// Names of every embedded file, sorted.
pub fn list() -> Vec<String> {
    let mut names: Vec<String> = Dist::iter().map(|n| n.into_owned()).collect();
    names.sort();
    names
}

/// Whether `index_html` is the placeholder emitted when the web build was skipped.
pub fn is_stub(index_html: &[u8]) -> bool {
    index_html
        .windows(STUB_MARKER.len())
        .any(|w| w == STUB_MARKER.as_bytes())
}

/// Whether the embedded `index.html` is the skipped-UI placeholder.
pub fn is_skipped_stub() -> bool {
    index_html().is_some_and(|a| is_stub(&a.data))
}

/// Writes the `--check` report for the given embedded index and file list.
///
/// Returns `true` when the UI is present, `false` when `index` is missing or is the skipped-UI
/// placeholder.
pub fn check_report(
    index: Option<&[u8]>,
    files: &[String],
    out: &mut impl Write,
) -> io::Result<bool> {
    writeln!(out, "embedded files: {}", files.len())?;
    for name in files {
        writeln!(out, "  {name}")?;
    }
    match index {
        None => {
            writeln!(
                out,
                "error: no embedded index.html; the web UI was not built"
            )?;
            Ok(false)
        }
        Some(bytes) if is_stub(bytes) => {
            writeln!(
                out,
                "error: the embedded index.html is the skipped-UI placeholder; rebuild without \
                 FOCI_TUNE_SKIP_WEB"
            )?;
            Ok(false)
        }
        Some(_) => {
            writeln!(out, "ok: web UI embedded")?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_is_recognised_only_by_its_marker() {
        let stub = format!("{STUB_MARKER}web UI skipped");
        assert!(is_stub(stub.as_bytes()));
        assert!(!is_stub(b"<!doctype html><html><body>app</body></html>"));
        assert!(!is_stub(b""));
    }

    #[test]
    fn check_fails_for_the_stub_and_a_missing_index_and_passes_for_a_real_page() {
        let files = vec!["index.html".to_owned()];
        let stub = format!("{STUB_MARKER}skipped");
        let mut out = Vec::new();

        assert!(!check_report(Some(stub.as_bytes()), &files, &mut out).unwrap());
        assert!(
            String::from_utf8(out.clone())
                .unwrap()
                .contains("skipped-UI")
        );
        assert!(!check_report(None, &files, &mut out).unwrap());
        assert!(check_report(Some(b"<!doctype html>"), &files, &mut out).unwrap());
    }

    #[test]
    fn mime_follows_the_file_extension() {
        let asset = index_html().expect("test build embeds an index.html");
        assert_eq!(asset.mime, "text/html");
    }
}
