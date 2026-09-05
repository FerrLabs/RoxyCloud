use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;

use super::xml::Property;

/// A property this server does not keep, remembered with the namespace it was asked under so the
/// answer names the same thing the client did. Explorer asks for `Win32LastModifiedTime` under
/// `urn:schemas-microsoft-com:`, and a bare local name is a different property to a client that
/// matches on both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unknown {
    pub namespace: String,
    pub name: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Requested {
    pub properties: Vec<Property>,
    pub unknown: Vec<Unknown>,
    pub names_only: bool,
}

impl Requested {
    fn everything() -> Self {
        Self {
            properties: Property::ALL.to_vec(),
            unknown: Vec::new(),
            names_only: false,
        }
    }
}

const DAV: &str = "DAV:";

/// An empty body means `allprop`, which is what the clients that send nothing expect. A body we
/// cannot parse is treated the same way: answering with every live property is what a client that
/// asked badly can still read, where a 400 leaves it showing an empty folder.
#[must_use]
pub fn parse(body: &[u8]) -> Requested {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Requested::everything();
    }

    let mut reader = NsReader::from_reader(body);
    reader.config_mut().trim_text(true);

    let mut inside_prop = false;
    let mut requested = Requested {
        properties: Vec::new(),
        unknown: Vec::new(),
        names_only: false,
    };

    loop {
        match reader.read_resolved_event() {
            Ok((resolved, Event::Start(element) | Event::Empty(element))) => {
                let name = String::from_utf8_lossy(element.local_name().into_inner()).into_owned();
                let namespace = match resolved {
                    ResolveResult::Bound(namespace) => {
                        String::from_utf8_lossy(namespace.into_inner()).into_owned()
                    }
                    _ => String::new(),
                };

                match name.as_str() {
                    "allprop" => return Requested::everything(),
                    "propname" => {
                        return Requested {
                            names_only: true,
                            ..Requested::everything()
                        };
                    }
                    "prop" => inside_prop = true,
                    _ if inside_prop => match Property::parse(&name).filter(|_| namespace == DAV) {
                        Some(property) => requested.properties.push(property),
                        None if is_name(&name) => {
                            requested.unknown.push(Unknown { namespace, name });
                        }
                        None => {}
                    },
                    _ => {}
                }
            }
            Ok((_, Event::End(element))) if element.local_name().into_inner() == b"prop" => {
                inside_prop = false;
            }
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(_) => return Requested::everything(),
        }
    }

    if requested.properties.is_empty() && requested.unknown.is_empty() {
        return Requested::everything();
    }
    requested
}

/// Whatever comes back goes into the answer as an element name, so anything that would not parse
/// as one is dropped rather than allowed to break the whole document.
fn is_name(candidate: &str) -> bool {
    let mut characters = candidate.chars();
    characters
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_')
        && characters.all(|rest| rest.is_alphanumeric() || matches!(rest, '.' | '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unknown(namespace: &str, name: &str) -> Unknown {
        Unknown {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn an_empty_body_asks_for_everything() {
        assert_eq!(parse(b""), Requested::everything());
        assert_eq!(parse(b"  \n"), Requested::everything());
    }

    #[test]
    fn allprop_asks_for_everything() {
        assert_eq!(
            parse(br#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>"#),
            Requested::everything()
        );
    }

    #[test]
    fn a_named_list_is_taken_literally() {
        let requested = parse(
            br#"<propfind xmlns="DAV:"><prop><getetag/><getcontentlength/></prop></propfind>"#,
        );

        assert_eq!(
            requested.properties,
            vec![Property::Etag, Property::ContentLength]
        );
        assert!(requested.unknown.is_empty());
    }

    #[test]
    fn a_property_we_do_not_keep_is_remembered_with_the_namespace_it_was_asked_under() {
        let requested = parse(
            br#"<D:propfind xmlns:D="DAV:" xmlns:Z="urn:schemas-microsoft-com:">
                  <D:prop><D:getetag/><Z:Win32LastModifiedTime/></D:prop>
                </D:propfind>"#,
        );

        assert_eq!(requested.properties, vec![Property::Etag]);
        assert_eq!(
            requested.unknown,
            vec![unknown(
                "urn:schemas-microsoft-com:",
                "Win32LastModifiedTime"
            )],
            "a bare local name is a different property to a client that matches on both"
        );
    }

    #[test]
    fn a_live_property_name_under_someone_elses_namespace_is_not_ours() {
        let requested = parse(
            br#"<D:propfind xmlns:D="DAV:" xmlns:Z="urn:example:"><D:prop><Z:getetag/></D:prop></D:propfind>"#,
        );

        assert!(requested.properties.is_empty());
        assert_eq!(requested.unknown, vec![unknown("urn:example:", "getetag")]);
    }

    #[test]
    fn any_prefix_works_because_clients_pick_their_own() {
        let requested =
            parse(br#"<x:propfind xmlns:x="DAV:"><x:prop><x:getetag/></x:prop></x:propfind>"#);
        assert_eq!(requested.properties, vec![Property::Etag]);
    }

    #[test]
    fn propname_asks_for_the_names_alone() {
        let requested = parse(br#"<D:propfind xmlns:D="DAV:"><D:propname/></D:propfind>"#);
        assert!(requested.names_only);
        assert_eq!(requested.properties, Property::ALL.to_vec());
    }

    #[test]
    fn a_body_that_is_not_xml_still_answers_something_a_client_can_read() {
        assert_eq!(parse(b"<<<not xml"), Requested::everything());
    }

    #[test]
    fn a_property_whose_name_is_not_a_name_is_dropped_rather_than_echoed() {
        assert!(is_name("Win32LastModifiedTime"));
        assert!(is_name("_private"));
        assert!(!is_name("1st"));
        assert!(!is_name("pr op"));
        assert!(!is_name(""));
    }
}
