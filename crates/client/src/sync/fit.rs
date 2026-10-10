use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use unicode_normalization::UnicodeNormalization;

use super::path::RelPath;
use super::snapshot::Snapshot;

const PROBE: &str = ".stashden-case-probe.stashpart";
const WINDOWS_FORBIDDEN: [char; 7] = ['<', '>', ':', '"', '|', '?', '*'];
const WINDOWS_RESERVED: [&str; 4] = ["con", "prn", "aux", "nul"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub windows_names: bool,
    pub folds_case: bool,
}

impl Rules {
    #[must_use]
    pub fn of(root: &Path) -> Self {
        Self {
            windows_names: cfg!(windows),
            folds_case: folds_case(root),
        }
    }
}

pub(crate) fn folds_case(root: &Path) -> bool {
    let lower = root.join(PROBE);
    if fs::write(&lower, b"").is_err() {
        return cfg!(any(windows, target_os = "macos"));
    }
    let folds = root.join(PROBE.to_uppercase()).exists();
    let _ = fs::remove_file(&lower);
    folds
}

#[must_use]
pub fn unfit(local: &Snapshot, remote: &Snapshot, rules: Rules) -> BTreeMap<RelPath, String> {
    let mut unfit = BTreeMap::new();

    if rules.windows_names {
        for path in remote.keys() {
            if let Some(name) = path.as_str().split('/').find(|name| !windows_holds(name)) {
                unfit.insert(
                    path.clone(),
                    format!("Windows cannot hold a file or folder named \"{name}\""),
                );
            }
        }
    }

    if rules.folds_case {
        let mut spellings: BTreeMap<String, BTreeSet<&RelPath>> = BTreeMap::new();
        for path in local.keys().chain(remote.keys()) {
            spellings.entry(folded(path)).or_default().insert(path);
        }
        for clashing in spellings.values().filter(|paths| paths.len() > 1) {
            for path in clashing {
                let others: Vec<&str> = clashing
                    .iter()
                    .filter(|other| *other != path)
                    .map(|other| other.as_str())
                    .collect();
                unfit.entry((*path).clone()).or_insert_with(|| {
                    format!(
                        "this folder cannot tell it apart from {}, which differs only in case or accents",
                        others.join(", ")
                    )
                });
            }
        }
    }

    let folders: Vec<RelPath> = unfit.keys().cloned().collect();
    for path in local.keys().chain(remote.keys()) {
        if let Some(folder) = folders.iter().find(|folder| path.is_inside(folder)) {
            unfit
                .entry(path.clone())
                .or_insert_with(|| format!("it is inside {folder}, which this folder cannot hold"));
        }
    }

    unfit
}

fn folded(path: &RelPath) -> String {
    path.as_str().nfc().collect::<String>().to_lowercase()
}

fn windows_holds(name: &str) -> bool {
    if name.contains(WINDOWS_FORBIDDEN) || name.ends_with('.') || name.ends_with(' ') {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_lowercase();
    !reserved_device(&stem)
}

fn reserved_device(stem: &str) -> bool {
    WINDOWS_RESERVED.contains(&stem)
        || ["com", "lpt"].into_iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|rest| matches!(rest.as_bytes(), [b'1'..=b'9']))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::snapshot::Entry;

    fn snapshot(paths: &[&str]) -> Snapshot {
        paths
            .iter()
            .map(|path| (RelPath::parse(path).expect("a path"), Entry::Directory))
            .collect()
    }

    fn unfit_paths(local: &[&str], remote: &[&str], rules: Rules) -> Vec<String> {
        unfit(&snapshot(local), &snapshot(remote), rules)
            .into_keys()
            .map(String::from)
            .collect()
    }

    const WINDOWS: Rules = Rules {
        windows_names: true,
        folds_case: true,
    };
    const LINUX: Rules = Rules {
        windows_names: false,
        folds_case: false,
    };

    #[test]
    fn names_windows_refuses_are_unfit_there() {
        for name in [
            "a:b.txt",
            "why?.txt",
            "star*.txt",
            "pipe|.txt",
            "quote\".txt",
            "angle<.txt",
            "trailing.",
            "trailing ",
            "CON",
            "con.txt",
            "Nul.tar.gz",
            "COM1",
            "lpt9.log",
        ] {
            assert_eq!(unfit_paths(&[], &[name], WINDOWS), vec![name], "{name}");
        }
    }

    #[test]
    fn names_that_only_look_reserved_are_fine() {
        for name in [
            "console.txt",
            "com10",
            "lpt0",
            "connect",
            "a.b.c",
            "nul-hypothesis.md",
        ] {
            assert_eq!(
                unfit_paths(&[], &[name], WINDOWS),
                Vec::<String>::new(),
                "{name}"
            );
        }
    }

    #[test]
    fn a_folder_windows_refuses_takes_what_is_inside_with_it() {
        assert_eq!(
            unfit_paths(&[], &["a:b", "a:b/c.txt"], WINDOWS),
            vec!["a:b", "a:b/c.txt"]
        );
    }

    #[test]
    fn linux_holds_what_windows_refuses() {
        assert_eq!(
            unfit_paths(&[], &["a:b.txt", "CON"], LINUX),
            Vec::<String>::new()
        );
    }

    #[test]
    fn two_server_names_that_differ_only_in_case_clash() {
        assert_eq!(
            unfit_paths(&[], &["A.txt", "a.txt", "b.txt"], WINDOWS),
            vec!["A.txt", "a.txt"]
        );
    }

    #[test]
    fn a_local_name_and_a_server_name_that_differ_only_in_case_clash() {
        assert_eq!(
            unfit_paths(&["Notes.txt"], &["notes.txt"], WINDOWS),
            vec!["Notes.txt", "notes.txt"]
        );
    }

    #[test]
    fn composed_and_decomposed_accents_clash() {
        let composed = "caf\u{e9}.txt";
        let decomposed = "cafe\u{301}.txt";
        assert_eq!(unfit_paths(&[], &[composed, decomposed], WINDOWS).len(), 2);
    }

    #[test]
    fn a_folder_that_clashes_takes_what_is_inside_with_it() {
        assert_eq!(
            unfit_paths(&[], &["Docs", "Docs/a.txt", "docs", "docs/a.txt"], WINDOWS),
            vec!["Docs", "Docs/a.txt", "docs", "docs/a.txt"]
        );
    }

    #[test]
    fn a_clashing_folder_takes_children_with_different_names_too() {
        assert_eq!(
            unfit_paths(&[], &["Docs", "Docs/a.txt", "docs", "docs/b.txt"], WINDOWS),
            vec!["Docs", "Docs/a.txt", "docs", "docs/b.txt"]
        );
    }

    #[test]
    fn a_clash_deep_in_the_tree_takes_everything_under_it() {
        assert_eq!(
            unfit_paths(
                &["top/Photos", "top/Photos/2026/beach.jpg"],
                &["top/photos", "top/keep.txt"],
                WINDOWS
            ),
            vec!["top/Photos", "top/Photos/2026/beach.jpg", "top/photos"]
        );
    }

    #[test]
    fn the_same_name_on_both_sides_is_not_a_clash() {
        assert_eq!(
            unfit_paths(&["a.txt"], &["a.txt"], WINDOWS),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_case_sensitive_folder_holds_both() {
        assert_eq!(
            unfit_paths(&[], &["A.txt", "a.txt"], LINUX),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_probe_tells_a_case_sensitive_folder_from_one_that_folds() {
        let root = std::env::temp_dir().join("stashden-fit-probe");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("scratch directory");

        let folds = folds_case(&root);

        assert_eq!(folds, cfg!(any(windows, target_os = "macos")));
        assert!(
            fs::read_dir(&root).expect("listing").next().is_none(),
            "the probe cleans up after itself"
        );
    }
}
