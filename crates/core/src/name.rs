use std::fmt;
use std::str::FromStr;

pub const MAX_NAME_LEN: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeName(String);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InvalidNodeName {
    #[error("name is empty")]
    Empty,
    #[error("name contains a path separator")]
    Separator,
    #[error("`.` and `..` are not names")]
    Relative,
    #[error("name is longer than {MAX_NAME_LEN} bytes")]
    TooLong,
    #[error("name contains a control character")]
    Control,
}

impl NodeName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for NodeName {
    type Err = InvalidNodeName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(InvalidNodeName::Empty);
        }
        if s.len() > MAX_NAME_LEN {
            return Err(InvalidNodeName::TooLong);
        }
        if s == "." || s == ".." {
            return Err(InvalidNodeName::Relative);
        }
        if s.contains('/') || s.contains('\\') {
            return Err(InvalidNodeName::Separator);
        }
        if s.chars().any(char::is_control) {
            return Err(InvalidNodeName::Control);
        }
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for NodeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn parse_path(path: &str) -> Result<Vec<NodeName>, InvalidNodeName> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(NodeName::from_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names() {
        for name in ["photo.jpg", "Mon Dossier", "a.b.c", "..hidden", "..."] {
            assert!(name.parse::<NodeName>().is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn rejects_traversal_segments() {
        assert_eq!("..".parse::<NodeName>(), Err(InvalidNodeName::Relative));
        assert_eq!(".".parse::<NodeName>(), Err(InvalidNodeName::Relative));
    }

    #[test]
    fn rejects_separators_including_backslash() {
        assert_eq!("a/b".parse::<NodeName>(), Err(InvalidNodeName::Separator));
        assert_eq!(
            "..\\..\\etc".parse::<NodeName>(),
            Err(InvalidNodeName::Separator)
        );
    }

    #[test]
    fn rejects_nul_and_newline() {
        assert_eq!("a\0b".parse::<NodeName>(), Err(InvalidNodeName::Control));
        assert_eq!("a\nb".parse::<NodeName>(), Err(InvalidNodeName::Control));
    }

    #[test]
    fn rejects_overlong_names() {
        let long = "x".repeat(MAX_NAME_LEN + 1);
        assert_eq!(long.parse::<NodeName>(), Err(InvalidNodeName::TooLong));
    }

    #[test]
    fn parse_path_drops_empty_segments() {
        let parsed = parse_path("//photos///summer/").expect("valid path");
        let names: Vec<_> = parsed.iter().map(NodeName::as_str).collect();
        assert_eq!(names, ["photos", "summer"]);
    }

    #[test]
    fn parse_path_rejects_traversal_anywhere() {
        assert_eq!(
            parse_path("photos/../../etc/passwd"),
            Err(InvalidNodeName::Relative)
        );
    }

    #[test]
    fn parse_path_of_root_is_empty() {
        assert_eq!(parse_path("/"), Ok(Vec::new()));
    }
}
