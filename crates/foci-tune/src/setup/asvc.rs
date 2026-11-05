//! Moonraker's `moonraker.asvc`: the list of services it may manage, one per line.

use std::path::{Path, PathBuf};

const FILE_NAME: &str = "moonraker.asvc";

/// `asvc` with `name` appended, or `None` if it is already listed.
pub(crate) fn with_service(asvc: &str, name: &str) -> Option<String> {
    if asvc.lines().any(|line| line.trim() == name) {
        return None;
    }
    let mut updated = asvc.to_owned();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(name);
    updated.push('\n');
    Some(updated)
}

/// `asvc` without `name`, or `None` if it is not listed.
pub(crate) fn without_service(asvc: &str, name: &str) -> Option<String> {
    if !asvc.lines().any(|line| line.trim() == name) {
        return None;
    }
    Some(
        asvc.lines()
            .filter(|line| line.trim() != name)
            .map(|line| format!("{line}\n"))
            .collect(),
    )
}

/// The `moonraker.asvc` that sits beside Moonraker's `logs` file root, in its data directory.
/// `None` when the root has no parent.
pub fn asvc_beside_logs(logs_root: &Path) -> Option<PathBuf> {
    Some(logs_root.parent()?.join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_then_removing_restores_the_original_and_other_lines_survive() {
        let asvc = "klipper_mcu\n# keep\nwebcamd\n";

        let added = with_service(asvc, "foci-tune").unwrap();

        assert_eq!(added, "klipper_mcu\n# keep\nwebcamd\nfoci-tune\n");
        assert_eq!(with_service(&added, "foci-tune"), None);
        assert_eq!(without_service(&added, "foci-tune").unwrap(), asvc);
        assert_eq!(without_service(asvc, "foci-tune"), None);
    }

    #[test]
    fn a_similar_name_is_not_taken_for_the_service() {
        assert_eq!(without_service("foci-tune-extra\n", "foci-tune"), None);
        assert!(with_service("foci-tune-extra\n", "foci-tune").is_some());
    }
}
