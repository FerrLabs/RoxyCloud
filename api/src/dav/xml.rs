use std::fmt::Write;

use chrono::{DateTime, Utc};

use super::propfind::Requested;
use roxycloud_core::node::{Node, NodeKind};

pub const MULTISTATUS_OPEN: &str = concat!(
    r#"<?xml version="1.0" encoding="utf-8"?>"#,
    r#"<D:multistatus xmlns:D="DAV:">"#
);
pub const MULTISTATUS_CLOSE: &str = "</D:multistatus>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Property {
    ContentLength,
    LastModified,
    Etag,
    ResourceType,
    DisplayName,
    ContentType,
    QuotaAvailable,
    QuotaUsed,
}

impl Property {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "getcontentlength" => Some(Self::ContentLength),
            "getlastmodified" => Some(Self::LastModified),
            "getetag" => Some(Self::Etag),
            "resourcetype" => Some(Self::ResourceType),
            "displayname" => Some(Self::DisplayName),
            "getcontenttype" => Some(Self::ContentType),
            "quota-available-bytes" => Some(Self::QuotaAvailable),
            "quota-used-bytes" => Some(Self::QuotaUsed),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::ContentLength => "getcontentlength",
            Self::LastModified => "getlastmodified",
            Self::Etag => "getetag",
            Self::ResourceType => "resourcetype",
            Self::DisplayName => "displayname",
            Self::ContentType => "getcontenttype",
            Self::QuotaAvailable => "quota-available-bytes",
            Self::QuotaUsed => "quota-used-bytes",
        }
    }

    pub const ALL: [Self; 8] = [
        Self::ResourceType,
        Self::DisplayName,
        Self::ContentLength,
        Self::ContentType,
        Self::LastModified,
        Self::Etag,
        Self::QuotaAvailable,
        Self::QuotaUsed,
    ];
}

pub struct Quota {
    pub used: i64,
    pub available: i64,
}

pub fn response(href: &str, node: &Node, quota: &Quota, requested: &Requested) -> String {
    let mut found = String::new();
    let mut missing = String::new();

    for property in &requested.properties {
        if requested.names_only {
            let _ = write!(found, "<D:{}/>", property.name());
            continue;
        }
        match value(*property, node, quota) {
            Some(rendered) => found.push_str(&rendered),
            None => {
                let _ = write!(missing, "<D:{}/>", property.name());
            }
        }
    }
    for name in &requested.unknown {
        let _ = write!(missing, "<D:{}/>", escape(name));
    }

    let mut out = format!("<D:response><D:href>{}</D:href>", escape(href));
    let _ = write!(
        out,
        "<D:propstat><D:prop>{found}</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>"
    );
    if !missing.is_empty() {
        let _ = write!(
            out,
            "<D:propstat><D:prop>{missing}</D:prop>             <D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>"
        );
    }
    out.push_str("</D:response>");
    out
}

fn value(property: Property, node: &Node, quota: &Quota) -> Option<String> {
    let rendered = match property {
        Property::ResourceType => match node.kind {
            NodeKind::Directory => "<D:resourcetype><D:collection/></D:resourcetype>".to_owned(),
            NodeKind::File => "<D:resourcetype/>".to_owned(),
        },
        Property::DisplayName => format!("<D:displayname>{}</D:displayname>", escape(&node.name)),
        Property::Etag => format!("<D:getetag>{}</D:getetag>", escape(&node.etag)),
        Property::LastModified => format!(
            "<D:getlastmodified>{}</D:getlastmodified>",
            http_date(node.updated_at)
        ),
        Property::ContentLength => match node.kind {
            NodeKind::File => format!("<D:getcontentlength>{}</D:getcontentlength>", node.size),
            NodeKind::Directory => return None,
        },
        Property::ContentType => match node.kind {
            NodeKind::File => {
                "<D:getcontenttype>application/octet-stream</D:getcontenttype>".to_owned()
            }
            NodeKind::Directory => {
                "<D:getcontenttype>httpd/unix-directory</D:getcontenttype>".to_owned()
            }
        },
        Property::QuotaUsed => format!("<D:quota-used-bytes>{}</D:quota-used-bytes>", quota.used),
        Property::QuotaAvailable => format!(
            "<D:quota-available-bytes>{}</D:quota-available-bytes>",
            quota.available
        ),
    };
    Some(rendered)
}

/// RFC 1123, which is what `getlastmodified` is defined as and what clients parse.
fn http_date(at: DateTime<Utc>) -> String {
    at.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

pub fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for character in raw.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_looks_like_markup_cannot_close_a_tag() {
        assert_eq!(
            escape("</D:href><script>&"),
            "&lt;/D:href&gt;&lt;script&gt;&amp;"
        );
    }

    #[test]
    fn every_live_property_has_a_name_that_parses_back() {
        for property in Property::ALL {
            assert_eq!(Property::parse(property.name()), Some(property));
        }
    }

    #[test]
    fn a_property_nobody_defines_is_not_invented() {
        assert_eq!(Property::parse("author"), None);
    }
}
