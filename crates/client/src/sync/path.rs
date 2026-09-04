use std::fmt;
use std::path::{Path, PathBuf};

use roxycloud_core::name::{InvalidNodeName, NodeName, parse_path};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RelPath(String);

#[derive(Debug, thiserror::Error)]
pub enum InvalidRelPath {
    #[error("a relative path needs at least one segment")]
    Empty,
    #[error(transparent)]
    Segment(#[from] InvalidNodeName),
}

impl RelPath {
    pub fn parse(input: &str) -> Result<Self, InvalidRelPath> {
        let segments = parse_path(input)?;
        if segments.is_empty() {
            return Err(InvalidRelPath::Empty);
        }
        let joined = segments
            .iter()
            .map(NodeName::as_str)
            .collect::<Vec<_>>()
            .join("/");
        Ok(Self(joined))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.0.split('/').count()
    }

    #[must_use]
    pub fn file_name(&self) -> &str {
        match self.0.rsplit_once('/') {
            Some((_, name)) => name,
            None => &self.0,
        }
    }

    #[must_use]
    pub fn is_inside(&self, directory: &Self) -> bool {
        self.0
            .strip_prefix(&directory.0)
            .is_some_and(|rest| rest.starts_with('/'))
    }

    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        self.0
            .rsplit_once('/')
            .map(|(parent, _)| Self(parent.to_owned()))
    }

    pub fn child(&self, name: &str) -> Result<Self, InvalidRelPath> {
        Self::parse(&format!("{}/{name}", self.0))
    }

    pub fn with_file_name(&self, name: &str) -> Result<Self, InvalidRelPath> {
        match self.parent() {
            Some(parent) => parent.child(name),
            None => Self::parse(name),
        }
    }

    #[must_use]
    pub fn to_path(&self, root: &Path) -> PathBuf {
        self.0
            .split('/')
            .fold(root.to_path_buf(), |path, segment| path.join(segment))
    }
}

impl TryFrom<String> for RelPath {
    type Error = InvalidRelPath;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<RelPath> for String {
    fn from(value: RelPath) -> Self {
        value.0
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_path_is_inside_the_directory_above_it() {
        let directory = RelPath::parse("photos").expect("valid path");
        assert!(
            RelPath::parse("photos/x.jpg")
                .expect("valid path")
                .is_inside(&directory)
        );
        assert!(
            RelPath::parse("photos/summer/x.jpg")
                .expect("valid path")
                .is_inside(&directory)
        );
    }

    #[test]
    fn a_name_that_merely_starts_the_same_is_not_inside() {
        let directory = RelPath::parse("photos").expect("valid path");
        assert!(
            !RelPath::parse("photos!notes")
                .expect("valid path")
                .is_inside(&directory)
        );
        assert!(
            !RelPath::parse("photoshoot/x.jpg")
                .expect("valid path")
                .is_inside(&directory)
        );
        assert!(
            !directory.is_inside(&directory),
            "nor is the directory itself"
        );
    }

    use super::*;

    fn path(input: &str) -> RelPath {
        RelPath::parse(input).expect("valid path")
    }

    #[test]
    fn normalises_redundant_separators() {
        assert_eq!(path("//photos///summer/").as_str(), "photos/summer");
    }

    #[test]
    fn the_root_is_not_a_relative_path() {
        assert!(matches!(RelPath::parse("/"), Err(InvalidRelPath::Empty)));
        assert!(matches!(RelPath::parse(""), Err(InvalidRelPath::Empty)));
    }

    #[test]
    fn traversal_is_refused() {
        assert!(matches!(
            RelPath::parse("photos/../../etc/passwd"),
            Err(InvalidRelPath::Segment(_))
        ));
    }

    #[test]
    fn a_child_cannot_smuggle_a_separator() {
        assert!(matches!(
            path("photos").child("../etc"),
            Err(InvalidRelPath::Segment(_))
        ));
    }

    #[test]
    fn parents_and_names_split_at_the_last_separator() {
        let deep = path("a/b/c.txt");
        assert_eq!(deep.file_name(), "c.txt");
        assert_eq!(deep.parent().expect("has a parent").as_str(), "a/b");
        assert_eq!(deep.depth(), 3);

        let shallow = path("c.txt");
        assert_eq!(shallow.file_name(), "c.txt");
        assert!(shallow.parent().is_none());
    }

    #[test]
    fn renaming_keeps_the_directory() {
        let renamed = path("a/b/c.txt")
            .with_file_name("c (conflict).txt")
            .expect("valid name");
        assert_eq!(renamed.as_str(), "a/b/c (conflict).txt");
    }

    #[test]
    fn local_paths_are_built_from_segments_not_from_the_string() {
        let root = Path::new("/tmp/roxy");
        assert_eq!(
            path("a/b/c.txt").to_path(root),
            root.join("a").join("b").join("c.txt")
        );
    }

    #[test]
    fn round_trips_through_serde() {
        let original = path("photos/summer/x.jpg");
        let json = serde_json::to_string(&original).expect("serialises");
        assert_eq!(json, "\"photos/summer/x.jpg\"");
        let parsed: RelPath = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(parsed, original);
    }

    #[test]
    fn serde_rejects_a_traversal_payload() {
        assert!(serde_json::from_str::<RelPath>("\"a/../b\"").is_err());
    }
}
