use super::engine::PARTIAL_SUFFIX;
use super::path::RelPath;

const SYSTEM_FILES: [&str; 4] = [".ds_store", "thumbs.db", "ehthumbs.db", "desktop.ini"];

#[must_use]
pub fn is_ignored(name: &str) -> bool {
    let lowered = name.to_lowercase();
    SYSTEM_FILES.contains(&lowered.as_str())
        || name.starts_with("~$")
        || name.starts_with("._")
        || (name.starts_with(".~lock.") && name.ends_with('#'))
        || name.ends_with(PARTIAL_SUFFIX)
}

#[must_use]
pub fn hides(path: &RelPath) -> bool {
    path.as_str().split('/').any(is_ignored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_operating_systems_leave_behind_is_ignored() {
        for name in [
            ".DS_Store",
            "Thumbs.db",
            "thumbs.db",
            "desktop.ini",
            "Desktop.ini",
        ] {
            assert!(is_ignored(name), "{name}");
        }
    }

    #[test]
    fn office_and_libreoffice_lock_files_are_ignored() {
        assert!(is_ignored("~$report.docx"));
        assert!(is_ignored(".~lock.report.odt#"));
    }

    #[test]
    fn mac_resource_forks_and_unfinished_downloads_are_ignored() {
        assert!(is_ignored("._photo.jpg"));
        assert!(is_ignored("photo.jpg.stashpart"));
    }

    #[test]
    fn ordinary_files_are_not() {
        for name in [
            "report.docx",
            "~notes.txt",
            ".bashrc",
            "thumbs.db.txt",
            ".~lock.report.odt",
        ] {
            assert!(!is_ignored(name), "{name}");
        }
    }

    #[test]
    fn a_path_is_hidden_by_any_ignored_segment() {
        assert!(hides(&RelPath::parse("photos/.DS_Store").expect("a path")));
        assert!(!hides(&RelPath::parse("photos/beach.jpg").expect("a path")));
    }
}
