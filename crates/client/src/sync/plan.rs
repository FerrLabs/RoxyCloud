use chrono::{DateTime, Utc};
use roxycloud_core::name::MAX_NAME_LEN;

use super::path::RelPath;
use super::snapshot::{Entry, Snapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    CreateLocalDirectory(RelPath),
    Download(RelPath),
    Upload(RelPath),
    DeleteLocal(RelPath),
    DeleteRemote(RelPath),
    RemoveLocalDirectory(RelPath),
    RemoveRemoteDirectory(RelPath),
    Forget(RelPath),
    KeepBoth { path: RelPath, local_copy: RelPath },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Plan {
    pub actions: Vec<Action>,
    pub blocked: Vec<RelPath>,
}

impl Plan {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty() && self.blocked.is_empty()
    }
}

#[must_use]
pub fn reconcile(local: &Snapshot, remote: &Snapshot, base: &Snapshot, now: DateTime<Utc>) -> Plan {
    let mut created = Vec::new();
    let mut transfers = Vec::new();
    let mut deletions = Vec::new();
    let mut removed = Vec::new();
    let mut removed_remotely = Vec::new();
    let mut forgotten = Vec::new();
    let mut blocked = Vec::new();

    let paths = local
        .keys()
        .chain(remote.keys())
        .chain(base.keys())
        .collect::<std::collections::BTreeSet<_>>();

    for path in paths {
        match (local.get(path), remote.get(path)) {
            (Some(Entry::Directory), Some(Entry::Directory)) => {}
            (None, None) => {
                if base.contains_key(path) {
                    forgotten.push(Action::Forget(path.clone()));
                }
            }
            (None, Some(Entry::Directory)) => {
                if !base.contains_key(path) {
                    created.push(Action::CreateLocalDirectory(path.clone()));
                } else if holds_only_what_was_synced(remote, base, path) {
                    removed_remotely.push(Action::RemoveRemoteDirectory(path.clone()));
                }
            }
            (Some(Entry::Directory), None) => {
                if base.contains_key(path) {
                    removed.push(Action::RemoveLocalDirectory(path.clone()));
                }
            }
            (Some(left), Some(right)) if left.is_directory() != right.is_directory() => {
                blocked.push(path.clone());
            }
            (local_entry, remote_entry) => {
                if let Some(action) = for_file(path, local_entry, remote_entry, base.get(path), now)
                {
                    match action {
                        Action::DeleteLocal(_) | Action::DeleteRemote(_) => deletions.push(action),
                        other => transfers.push(other),
                    }
                }
            }
        }
    }

    removed.reverse();
    removed_remotely.reverse();

    let mut actions = created;
    actions.extend(transfers);
    actions.extend(deletions);
    actions.extend(removed_remotely);
    actions.extend(removed);
    actions.extend(forgotten);

    Plan { actions, blocked }
}

fn holds_only_what_was_synced(remote: &Snapshot, base: &Snapshot, directory: &RelPath) -> bool {
    remote
        .range(directory.clone()..)
        .skip_while(|(path, _)| *path == directory)
        .take_while(|(path, _)| path.as_str().starts_with(directory.as_str()))
        .filter(|(path, _)| path.is_inside(directory))
        .all(|(path, entry)| base.get(path) == Some(entry))
}

fn for_file(
    path: &RelPath,
    local: Option<&Entry>,
    remote: Option<&Entry>,
    base: Option<&Entry>,
    now: DateTime<Utc>,
) -> Option<Action> {
    match (local, remote) {
        (Some(left), Some(right)) if left == right => None,
        (Some(_), None) => Some(match base {
            Some(recorded) if Some(recorded) == local => Action::DeleteLocal(path.clone()),
            _ => Action::Upload(path.clone()),
        }),
        (None, Some(_)) => Some(match base {
            Some(recorded) if Some(recorded) == remote => Action::DeleteRemote(path.clone()),
            _ => Action::Download(path.clone()),
        }),
        (Some(left), Some(right)) => {
            if base == Some(left) {
                Some(Action::Download(path.clone()))
            } else if base == Some(right) {
                Some(Action::Upload(path.clone()))
            } else {
                conflict_copy(path, now).map(|local_copy| Action::KeepBoth {
                    path: path.clone(),
                    local_copy,
                })
            }
        }
        (None, None) => None,
    }
}

fn conflict_copy(path: &RelPath, now: DateTime<Utc>) -> Option<RelPath> {
    let name = path.file_name();
    let suffix = format!(" (conflict {})", now.format("%Y%m%dT%H%M%SZ"));
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };

    let extension = if suffix.len() + extension.len() < MAX_NAME_LEN {
        extension
    } else {
        String::new()
    };
    let room = MAX_NAME_LEN - suffix.len() - extension.len();
    let stem = &stem[..floor_char_boundary(stem, room)];

    path.with_file_name(&format!("{stem}{suffix}{extension}"))
        .ok()
}

fn floor_char_boundary(text: &str, limit: usize) -> usize {
    if limit >= text.len() {
        return text.len();
    }
    (0..=limit)
        .rev()
        .find(|at| text.is_char_boundary(*at))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxycloud_core::blob::BlobHash;

    fn at(input: &str) -> RelPath {
        RelPath::parse(input).expect("valid path")
    }

    fn file(contents: &[u8]) -> Entry {
        let size = u64::try_from(contents.len()).expect("small test payload");
        Entry::file(BlobHash::from(blake3::hash(contents)), size)
    }

    fn snapshot(entries: &[(&str, Entry)]) -> Snapshot {
        entries
            .iter()
            .map(|(path, entry)| (at(path), entry.clone()))
            .collect()
    }

    fn now() -> DateTime<Utc> {
        "2026-09-03T12:00:00Z"
            .parse()
            .expect("a fixed instant so conflict names are predictable")
    }

    fn plan(local: &Snapshot, remote: &Snapshot, base: &Snapshot) -> Vec<Action> {
        reconcile(local, remote, base, now()).actions
    }

    #[test]
    fn agreement_produces_no_work() {
        let both = snapshot(&[("a.txt", file(b"same"))]);
        assert!(reconcile(&both, &both, &both, now()).is_empty());
    }

    #[test]
    fn a_new_local_file_is_uploaded() {
        let local = snapshot(&[("a.txt", file(b"new"))]);
        assert_eq!(
            plan(&local, &Snapshot::new(), &Snapshot::new()),
            [Action::Upload(at("a.txt"))]
        );
    }

    #[test]
    fn a_new_remote_file_is_downloaded() {
        let remote = snapshot(&[("a.txt", file(b"new"))]);
        assert_eq!(
            plan(&Snapshot::new(), &remote, &Snapshot::new()),
            [Action::Download(at("a.txt"))]
        );
    }

    #[test]
    fn a_local_edit_is_uploaded() {
        let base = snapshot(&[("a.txt", file(b"old"))]);
        let local = snapshot(&[("a.txt", file(b"edited"))]);
        assert_eq!(plan(&local, &base, &base), [Action::Upload(at("a.txt"))]);
    }

    #[test]
    fn a_remote_edit_is_downloaded() {
        let base = snapshot(&[("a.txt", file(b"old"))]);
        let remote = snapshot(&[("a.txt", file(b"edited"))]);
        assert_eq!(plan(&base, &remote, &base), [Action::Download(at("a.txt"))]);
    }

    #[test]
    fn a_file_deleted_on_the_remote_is_deleted_locally() {
        let base = snapshot(&[("a.txt", file(b"agreed"))]);
        assert_eq!(
            plan(&base, &Snapshot::new(), &base),
            [Action::DeleteLocal(at("a.txt"))]
        );
    }

    #[test]
    fn a_file_deleted_locally_is_deleted_on_the_remote() {
        let base = snapshot(&[("a.txt", file(b"agreed"))]);
        assert_eq!(
            plan(&Snapshot::new(), &base, &base),
            [Action::DeleteRemote(at("a.txt"))]
        );
    }

    #[test]
    fn a_local_edit_survives_a_remote_delete() {
        let base = snapshot(&[("a.txt", file(b"agreed"))]);
        let local = snapshot(&[("a.txt", file(b"edited after the delete"))]);
        assert_eq!(
            plan(&local, &Snapshot::new(), &base),
            [Action::Upload(at("a.txt"))]
        );
    }

    #[test]
    fn a_remote_edit_survives_a_local_delete() {
        let base = snapshot(&[("a.txt", file(b"agreed"))]);
        let remote = snapshot(&[("a.txt", file(b"edited after the delete"))]);
        assert_eq!(
            plan(&Snapshot::new(), &remote, &base),
            [Action::Download(at("a.txt"))]
        );
    }

    #[test]
    fn both_sides_changed_keeps_both_copies() {
        let base = snapshot(&[("notes/a.txt", file(b"agreed"))]);
        let local = snapshot(&[("notes/a.txt", file(b"mine"))]);
        let remote = snapshot(&[("notes/a.txt", file(b"theirs"))]);

        assert_eq!(
            plan(&local, &remote, &base),
            [Action::KeepBoth {
                path: at("notes/a.txt"),
                local_copy: at("notes/a (conflict 20260903T120000Z).txt"),
            }]
        );
    }

    #[test]
    fn two_sides_creating_the_same_path_is_also_a_conflict() {
        let local = snapshot(&[("a.txt", file(b"mine"))]);
        let remote = snapshot(&[("a.txt", file(b"theirs"))]);
        assert_eq!(
            plan(&local, &remote, &Snapshot::new()),
            [Action::KeepBoth {
                path: at("a.txt"),
                local_copy: at("a (conflict 20260903T120000Z).txt"),
            }]
        );
    }

    #[test]
    fn two_sides_creating_identical_bytes_is_not_a_conflict() {
        let same = snapshot(&[("a.txt", file(b"identical"))]);
        assert!(reconcile(&same, &same, &Snapshot::new(), now()).is_empty());
    }

    #[test]
    fn a_conflict_copy_keeps_the_extension_and_the_directory() {
        let copy = conflict_copy(&at("photos/summer/x.tar.gz"), now()).expect("a name fits");
        assert_eq!(
            copy.as_str(),
            "photos/summer/x.tar (conflict 20260903T120000Z).gz"
        );
    }

    #[test]
    fn a_dotfile_keeps_its_leading_dot_and_gains_no_extension() {
        let copy = conflict_copy(&at(".env"), now()).expect("a name fits");
        assert_eq!(copy.as_str(), ".env (conflict 20260903T120000Z)");
    }

    #[test]
    fn a_conflict_copy_of_a_very_long_name_stays_a_valid_name() {
        let long = format!("{}.txt", "é".repeat(120));
        assert!(long.len() <= MAX_NAME_LEN, "the original name is legal");

        let copy = conflict_copy(&at(&long), now()).expect("a name fits");
        let name = copy.file_name();
        assert!(name.len() <= MAX_NAME_LEN, "{} bytes", name.len());
        assert_eq!(
            std::path::Path::new(name).extension(),
            Some("txt".as_ref()),
            "the extension survives: {name}"
        );
        assert!(
            name.contains(" (conflict "),
            "the stem is truncated, not the marker: {name}"
        );
    }

    #[test]
    fn a_new_remote_directory_is_created_locally() {
        let remote = snapshot(&[("photos", Entry::Directory)]);
        assert_eq!(
            plan(&Snapshot::new(), &remote, &Snapshot::new()),
            [Action::CreateLocalDirectory(at("photos"))]
        );
    }

    #[test]
    fn a_directory_the_user_removed_locally_is_removed_on_the_server() {
        let base = snapshot(&[("photos", Entry::Directory)]);
        let remote = base.clone();
        assert_eq!(
            plan(&Snapshot::new(), &remote, &base),
            [Action::RemoveRemoteDirectory(at("photos"))],
            "and it is not recreated locally either"
        );
    }

    #[test]
    fn a_directory_is_removed_on_the_server_after_its_contents() {
        let base = snapshot(&[
            ("photos", Entry::Directory),
            ("photos/summer", Entry::Directory),
            ("photos/summer/x.jpg", file(b"deep")),
            ("photos/y.jpg", file(b"shallow")),
        ]);
        let remote = base.clone();

        assert_eq!(
            plan(&Snapshot::new(), &remote, &base),
            [
                Action::DeleteRemote(at("photos/summer/x.jpg")),
                Action::DeleteRemote(at("photos/y.jpg")),
                Action::RemoveRemoteDirectory(at("photos/summer")),
                Action::RemoveRemoteDirectory(at("photos")),
            ]
        );
    }

    #[test]
    fn a_remote_file_the_local_side_never_saw_keeps_its_directory() {
        let base = snapshot(&[
            ("photos", Entry::Directory),
            ("photos/x.jpg", file(b"synced")),
        ]);
        let remote = snapshot(&[
            ("photos", Entry::Directory),
            ("photos/x.jpg", file(b"synced")),
            ("photos/added.jpg", file(b"from another machine")),
        ]);

        assert_eq!(
            plan(&Snapshot::new(), &remote, &base),
            [
                Action::Download(at("photos/added.jpg")),
                Action::DeleteRemote(at("photos/x.jpg")),
            ],
            "deleting the folder would take a file this side has never seen with it"
        );
    }

    #[test]
    fn a_remote_edit_under_a_removed_directory_keeps_its_directory() {
        let base = snapshot(&[
            ("photos", Entry::Directory),
            ("photos/x.jpg", file(b"before")),
        ]);
        let remote = snapshot(&[
            ("photos", Entry::Directory),
            ("photos/x.jpg", file(b"after")),
        ]);

        assert_eq!(
            plan(&Snapshot::new(), &remote, &base),
            [Action::Download(at("photos/x.jpg"))],
            "the same rule a single file gets: an edit outlives a delete"
        );
    }

    #[test]
    fn a_directory_removed_on_both_sides_is_dropped_from_the_base() {
        let base = snapshot(&[("photos", Entry::Directory)]);
        assert_eq!(
            plan(&Snapshot::new(), &Snapshot::new(), &base),
            [Action::Forget(at("photos"))],
            "a record that outlives both sides would read as a folder the user removed"
        );
    }

    #[test]
    fn a_directory_that_came_back_on_the_server_is_created_rather_than_removed() {
        let remote = snapshot(&[("photos", Entry::Directory)]);
        assert_eq!(
            plan(&Snapshot::new(), &remote, &Snapshot::new()),
            [Action::CreateLocalDirectory(at("photos"))],
            "once the stale record is gone, a folder another machine made is new again"
        );
    }

    #[test]
    fn a_sibling_that_merely_shares_a_prefix_does_not_hold_a_directory_back() {
        let base = snapshot(&[
            ("photos", Entry::Directory),
            ("photos!notes", file(b"sibling")),
        ]);
        let remote = snapshot(&[
            ("photos", Entry::Directory),
            ("photos!notes", file(b"edited elsewhere")),
        ]);

        assert_eq!(
            plan(&Snapshot::new(), &remote, &base),
            [
                Action::Download(at("photos!notes")),
                Action::RemoveRemoteDirectory(at("photos")),
            ]
        );
    }

    #[test]
    fn a_local_directory_without_a_remote_counterpart_is_left_alone() {
        let local = snapshot(&[("photos", Entry::Directory)]);
        assert!(reconcile(&local, &Snapshot::new(), &Snapshot::new(), now()).is_empty());
    }

    #[test]
    fn directories_are_created_before_their_contents_and_removed_after() {
        let remote = snapshot(&[
            ("photos", Entry::Directory),
            ("photos/x.jpg", file(b"bytes")),
        ]);
        let base = snapshot(&[("old", Entry::Directory), ("old/y.jpg", file(b"gone soon"))]);
        let local = base.clone();

        assert_eq!(
            plan(&local, &remote, &base),
            [
                Action::CreateLocalDirectory(at("photos")),
                Action::Download(at("photos/x.jpg")),
                Action::DeleteLocal(at("old/y.jpg")),
                Action::RemoveLocalDirectory(at("old")),
            ]
        );
    }

    #[test]
    fn nested_directory_removals_run_deepest_first() {
        let base = snapshot(&[
            ("a", Entry::Directory),
            ("a/b", Entry::Directory),
            ("a/b/c", Entry::Directory),
        ]);
        let local = base.clone();

        assert_eq!(
            plan(&local, &Snapshot::new(), &base),
            [
                Action::RemoveLocalDirectory(at("a/b/c")),
                Action::RemoveLocalDirectory(at("a/b")),
                Action::RemoveLocalDirectory(at("a")),
            ]
        );
    }

    #[test]
    fn a_path_that_is_a_file_on_one_side_and_a_directory_on_the_other_is_blocked() {
        let local = snapshot(&[("a", file(b"a file here"))]);
        let remote = snapshot(&[("a", Entry::Directory)]);

        let plan = reconcile(&local, &remote, &Snapshot::new(), now());
        assert!(plan.actions.is_empty());
        assert_eq!(plan.blocked, [at("a")]);
    }
}
