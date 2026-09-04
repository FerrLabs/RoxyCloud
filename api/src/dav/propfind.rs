use quick_xml::Reader;
use quick_xml::events::Event;

use super::xml::Property;

#[derive(Debug, PartialEq, Eq)]
pub struct Requested {
    pub properties: Vec<Property>,
    pub unknown: Vec<String>,
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

/// An empty body means `allprop`, which is what the clients that send nothing expect. A body we
/// cannot parse is treated the same way: answering with every live property is what a client that
/// asked badly can still read, where a 400 leaves it showing an empty folder.
#[must_use]
pub fn parse(body: &[u8]) -> Requested {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Requested::everything();
    }

    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);

    let mut inside_prop = false;
    let mut requested = Requested {
        properties: Vec::new(),
        unknown: Vec::new(),
        names_only: false,
    };

    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                let name = local_name(element.name().as_ref());
                match name.as_str() {
                    "allprop" => return Requested::everything(),
                    "propname" => {
                        return Requested {
                            names_only: true,
                            ..Requested::everything()
                        };
                    }
                    "prop" => inside_prop = true,
                    _ if inside_prop => match Property::parse(&name) {
                        Some(property) => requested.properties.push(property),
                        None => requested.unknown.push(name),
                    },
                    _ => {}
                }
            }
            Ok(Event::End(element)) if local_name(element.name().as_ref()) == "prop" => {
                inside_prop = false;
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Requested::everything(),
        }
    }

    if requested.properties.is_empty() && requested.unknown.is_empty() {
        return Requested::everything();
    }
    requested
}

fn local_name(raw: &[u8]) -> String {
    let name = String::from_utf8_lossy(raw);
    match name.split_once(':') {
        Some((_, local)) => local.to_owned(),
        None => name.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_property_we_do_not_keep_is_remembered_so_it_can_be_reported_missing() {
        let requested = parse(
            br#"<D:propfind xmlns:D="DAV:"><D:prop><D:getetag/><D:author/></D:prop></D:propfind>"#,
        );

        assert_eq!(requested.properties, vec![Property::Etag]);
        assert_eq!(requested.unknown, vec!["author".to_owned()]);
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
}
