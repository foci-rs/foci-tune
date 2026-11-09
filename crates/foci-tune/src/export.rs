//! Support bundles: one session or one tune with everything stored for it, as a `.tar.zst`.
//!
//! A session bundle holds `session.json` and every run directory (`run.json`, `series.json`,
//! `capture.bin.zst`). A tune bundle holds `tune.json` and each session the tune's steps own
//! under `sessions/`. Both end with a `manifest.json` that names the bundle kind, the
//! foci-tune version, the sessions a tune names that are no longer on disk and the captures left
//! out because their run had not finished. The manifest is the last entry, since both lists are
//! only known once the files have been walked.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use structured_zstd::encoding::{CompressionLevel, StreamingEncoder};

use crate::model::{SessionId, TuneId};
use crate::store::{Store, StoreError};
use crate::tune::Tune;

/// Media type of a bundle.
pub const CONTENT_TYPE: &str = "application/zstd";

type Archive = tar::Builder<StreamingEncoder<Vec<u8>>>;

fn start() -> Archive {
    let mut archive =
        tar::Builder::new(StreamingEncoder::new(Vec::new(), CompressionLevel::Fastest));
    archive.mode(tar::HeaderMode::Deterministic);
    archive
}

fn finish(archive: Archive) -> io::Result<Vec<u8>> {
    archive.into_inner()?.finish()
}

fn mtime() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn append_bytes(archive: &mut Archive, name: &str, bytes: &[u8]) -> io::Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(mtime());
    archive.append_data(&mut header, name, bytes)
}

fn append_manifest(archive: &mut Archive, kind: &str, id: &str, extra: Value) -> io::Result<()> {
    let mut manifest = json!({
        "kind": kind,
        "id": id,
        "foci_tune_version": env!("CARGO_PKG_VERSION"),
        "exported_unix_s": mtime(),
    });
    if let (Some(m), Value::Object(extra)) = (manifest.as_object_mut(), extra) {
        m.extend(extra);
    }
    let text = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    append_bytes(archive, "manifest.json", &text)
}

const CAPTURE_FILE: &str = "capture.bin.zst";
const RUN_FILE: &str = "run.json";

fn capture_is_finished(run_dir: &Path) -> bool {
    std::fs::read(run_dir.join(RUN_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|run| !run["capture"].is_null())
}

fn append_tree(
    archive: &mut Archive,
    dir: &Path,
    prefix: &str,
    unfinished: &mut Vec<String>,
) -> io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.ends_with(".tmp") {
            continue;
        }
        let path = format!("{prefix}/{name}");
        let kind = entry.file_type()?;
        if kind.is_dir() {
            append_tree(archive, &entry.path(), &path, unfinished)?;
        } else if kind.is_file() {
            if name == CAPTURE_FILE && !entry.path().parent().is_some_and(capture_is_finished) {
                unfinished.push(prefix.to_owned());
                continue;
            }
            let mut file = std::fs::File::open(entry.path())?;
            archive.append_file(&path, &mut file)?;
        }
    }
    Ok(())
}

/// Bundles one session. A capture whose run has not recorded it as finished may be cut off in
/// the middle of a zstd frame, so it is left out and named under `unfinished_captures`.
///
/// # Errors
/// [`StoreError::Io`] with `NotFound` when the session does not exist, and other I/O errors.
pub fn session_bundle(store: &Store, session: &SessionId) -> Result<Vec<u8>, StoreError> {
    let dir = store.session_dir(session);
    if !dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("session {session} not found"),
        )
        .into());
    }
    let mut archive = start();
    let mut unfinished = Vec::new();
    append_tree(&mut archive, &dir, session.as_str(), &mut unfinished)?;
    append_manifest(
        &mut archive,
        "session",
        session.as_str(),
        json!({"unfinished_captures": unfinished}),
    )?;
    Ok(finish(archive)?)
}

/// Bundles one tune and the sessions its steps own.
///
/// # Errors
/// [`StoreError::Io`] with `NotFound` when the tune does not exist, [`StoreError::Json`] when
/// its record is unreadable, and other I/O errors.
pub fn tune_bundle(store: &Store, id: &TuneId) -> Result<Vec<u8>, StoreError> {
    let record = std::fs::read(store.tune_path(id))?;
    let tune: Tune = serde_json::from_slice(&record)?;
    let owned: BTreeSet<&SessionId> = tune
        .steps
        .iter()
        .flat_map(|s| {
            [
                &s.sessions.probe,
                &s.sessions.tracking,
                &s.sessions.standstill,
            ]
        })
        .collect();
    let (present, missing): (Vec<_>, Vec<_>) = owned
        .into_iter()
        .partition(|s| store.session_dir(s).is_dir());

    let mut archive = start();
    append_bytes(&mut archive, &format!("{id}/tune.json"), &record)?;
    let mut unfinished = Vec::new();
    for session in present {
        let prefix = format!("{id}/sessions/{session}");
        append_tree(
            &mut archive,
            &store.session_dir(session),
            &prefix,
            &mut unfinished,
        )?;
    }
    append_manifest(
        &mut archive,
        "tune",
        id.as_str(),
        json!({
            "missing_sessions": missing.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            "unfinished_captures": unfinished,
        }),
    )?;
    Ok(finish(archive)?)
}
