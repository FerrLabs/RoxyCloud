use roxycloud_core::grant::{Access, SHARED_WITH_ME};

use super::path::RelPath;
use super::plan::{Action, Plan};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Held {
    shelf: Option<RelPath>,
    mounts: Vec<(RelPath, Access)>,
}

impl Held {
    #[must_use]
    pub fn from_mounts(mounts: impl IntoIterator<Item = (String, Access)>) -> Self {
        let Ok(shelf) = RelPath::parse(SHARED_WITH_ME) else {
            return Self::default();
        };
        let mounts = mounts
            .into_iter()
            .filter_map(|(name, access)| Some((shelf.child(&name).ok()?, access)))
            .collect();
        Self {
            shelf: Some(shelf),
            mounts,
        }
    }

    #[must_use]
    pub fn apply(&self, plan: Plan) -> Plan {
        let uprooted: Vec<RelPath> = plan
            .actions
            .iter()
            .filter_map(|action| match action {
                Action::RemoveRemoteDirectory(path) if self.fixed(path) => Some(path.clone()),
                _ => None,
            })
            .collect();

        let mut held = plan.held;
        let mut actions = Vec::with_capacity(plan.actions.len());
        for action in plan.actions {
            if let Action::KeepBoth { path, local_copy } = &action
                && self.refuses_upload(local_copy)
            {
                held.push(local_copy.clone());
                actions.push(Action::SetAside {
                    path: path.clone(),
                    local_copy: local_copy.clone(),
                });
                continue;
            }
            match self.holding(&action, &uprooted) {
                Some(path) => held.push(path.clone()),
                None => actions.push(action),
            }
        }
        Plan {
            actions,
            blocked: plan.blocked,
            held,
        }
    }

    fn holding<'a>(&self, action: &'a Action, uprooted: &[RelPath]) -> Option<&'a RelPath> {
        match action {
            Action::Upload(path) if self.refuses_upload(path) => Some(path),
            Action::DeleteRemote(path) | Action::RemoveRemoteDirectory(path)
                if self.read_only(path)
                    || self.fixed(path)
                    || uprooted.iter().any(|root| path.is_inside(root)) =>
            {
                Some(path)
            }
            _ => None,
        }
    }

    fn refuses_upload(&self, path: &RelPath) -> bool {
        self.read_only(path) || self.loose_on_the_shelf(path)
    }

    fn read_only(&self, path: &RelPath) -> bool {
        self.mounts
            .iter()
            .any(|(mount, access)| !access.may_write() && (path == mount || path.is_inside(mount)))
    }

    fn fixed(&self, path: &RelPath) -> bool {
        self.shelf.as_ref() == Some(path) || self.mounts.iter().any(|(mount, _)| mount == path)
    }

    fn loose_on_the_shelf(&self, path: &RelPath) -> bool {
        let Some(top) = &self.shelf else {
            return false;
        };
        if path == top {
            return true;
        }
        path.parent().as_ref() == Some(top) && !self.mounts.iter().any(|(mount, _)| mount == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str) -> RelPath {
        RelPath::parse(path).expect("valid path")
    }

    fn held() -> Held {
        Held::from_mounts([
            ("archive".to_owned(), Access::Read),
            ("inbox".to_owned(), Access::Write),
            ("report.pdf".to_owned(), Access::Write),
        ])
    }

    fn outcome(action: Action) -> Plan {
        held().apply(Plan {
            actions: vec![action],
            ..Plan::default()
        })
    }

    fn is_held(action: Action) -> bool {
        let plan = outcome(action);
        plan.actions.is_empty() && plan.held.len() == 1
    }

    #[test]
    fn nothing_is_written_into_a_read_only_share() {
        assert!(is_held(Action::Upload(at(
            "Shared with me/archive/new.jpg"
        ))));
        assert!(is_held(Action::DeleteRemote(at(
            "Shared with me/archive/old.jpg"
        ))));
        assert!(is_held(Action::RemoveRemoteDirectory(at(
            "Shared with me/archive/2019"
        ))));
    }

    #[test]
    fn a_write_share_is_written_to_like_anything_else() {
        assert!(!is_held(Action::Upload(at("Shared with me/inbox/new.jpg"))));
        assert!(!is_held(Action::DeleteRemote(at(
            "Shared with me/inbox/old.jpg"
        ))));
        assert!(!is_held(Action::Upload(at("Shared with me/report.pdf"))));
    }

    #[test]
    fn neither_the_shelf_nor_a_share_itself_is_removed() {
        assert!(is_held(Action::RemoveRemoteDirectory(at("Shared with me"))));
        assert!(is_held(Action::RemoveRemoteDirectory(at(
            "Shared with me/inbox"
        ))));
        assert!(is_held(Action::DeleteRemote(at(
            "Shared with me/report.pdf"
        ))));
    }

    #[test]
    fn removing_a_write_share_takes_nothing_of_the_owners_with_it() {
        let plan = held().apply(Plan {
            actions: vec![
                Action::DeleteRemote(at("Shared with me/inbox/a.txt")),
                Action::RemoveRemoteDirectory(at("Shared with me/inbox/drafts")),
                Action::RemoveRemoteDirectory(at("Shared with me/inbox")),
            ],
            ..Plan::default()
        });
        assert!(plan.actions.is_empty(), "{:?}", plan.actions);
        assert_eq!(plan.held.len(), 3);
    }

    #[test]
    fn removing_the_shelf_takes_nothing_from_any_share() {
        let plan = held().apply(Plan {
            actions: vec![
                Action::DeleteRemote(at("Shared with me/inbox/a.txt")),
                Action::RemoveRemoteDirectory(at("Shared with me/inbox")),
                Action::RemoveRemoteDirectory(at("Shared with me")),
            ],
            ..Plan::default()
        });
        assert!(plan.actions.is_empty(), "{:?}", plan.actions);
    }

    #[test]
    fn deleting_files_inside_a_write_share_still_goes_through() {
        let plan = held().apply(Plan {
            actions: vec![Action::DeleteRemote(at("Shared with me/inbox/a.txt"))],
            ..Plan::default()
        });
        assert_eq!(
            plan.actions,
            [Action::DeleteRemote(at("Shared with me/inbox/a.txt"))]
        );
    }

    #[test]
    fn a_conflict_in_a_read_only_share_still_brings_the_owners_version_down() {
        let plan = outcome(Action::KeepBoth {
            path: at("Shared with me/archive/old.jpg"),
            local_copy: at("Shared with me/archive/old (conflict).jpg"),
        });
        assert_eq!(
            plan.actions,
            [Action::SetAside {
                path: at("Shared with me/archive/old.jpg"),
                local_copy: at("Shared with me/archive/old (conflict).jpg"),
            }]
        );
        assert_eq!(plan.held, [at("Shared with me/archive/old (conflict).jpg")]);
    }

    #[test]
    fn a_conflict_on_a_shared_file_keeps_its_copy_off_the_shelf() {
        let plan = outcome(Action::KeepBoth {
            path: at("Shared with me/report.pdf"),
            local_copy: at("Shared with me/report (conflict).pdf"),
        });
        assert!(matches!(plan.actions.as_slice(), [Action::SetAside { .. }]));
        assert_eq!(plan.held, [at("Shared with me/report (conflict).pdf")]);
    }

    #[test]
    fn a_conflict_in_a_write_share_keeps_both_as_anywhere_else() {
        let conflict = Action::KeepBoth {
            path: at("Shared with me/inbox/todo.txt"),
            local_copy: at("Shared with me/inbox/todo (conflict).txt"),
        };
        let plan = outcome(conflict.clone());
        assert_eq!(plan.actions, [conflict]);
        assert!(plan.held.is_empty());
    }

    #[test]
    fn a_file_dropped_loose_on_the_shelf_is_not_uploaded() {
        assert!(is_held(Action::Upload(at("Shared with me/stray.txt"))));
    }

    #[test]
    fn reading_is_never_held() {
        assert!(!is_held(Action::Download(at(
            "Shared with me/archive/old.jpg"
        ))));
        assert!(!is_held(Action::DeleteLocal(at(
            "Shared with me/archive/old.jpg"
        ))));
    }

    #[test]
    fn a_server_that_knows_nothing_of_shares_holds_nothing_back() {
        let plan = Held::default().apply(Plan {
            actions: vec![Action::Upload(at("Shared with me/anything.txt"))],
            ..Plan::default()
        });
        assert_eq!(plan.actions.len(), 1);
    }

    #[test]
    fn what_is_held_back_is_told_apart_from_a_clash() {
        let plan = held().apply(Plan {
            actions: vec![
                Action::Upload(at("Shared with me/archive/new.jpg")),
                Action::Upload(at("notes.txt")),
            ],
            blocked: vec![at("clash")],
            held: Vec::new(),
        });
        assert_eq!(plan.actions, [Action::Upload(at("notes.txt"))]);
        assert_eq!(plan.blocked, [at("clash")]);
        assert_eq!(plan.held, [at("Shared with me/archive/new.jpg")]);
    }
}
