//! A minimal XML tree for SOAP answers: local names only (namespaces are
//! ignored, cameras use many prefixes), entities resolved.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// An element with its attributes, text and children.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<Element>,
}

fn start(e: &BytesStart<'_>) -> Element {
    Element {
        name: e.local_name().as_ref().to_owned(),
        attrs: e
            .attributes()
            .flatten()
            .map(|a| {
                (
                    a.key.local_name().as_ref().to_owned(),
                    a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map(|v| v.into_owned())
                        .unwrap_or_default(),
                )
            })
            .collect(),
        ..Element::default()
    }
}

/// `&name;` / `&#n;` / `&#xn;` as a character.
fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let n = name.strip_prefix('#')?;
            let code = match n.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => n.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// The root element of `xml`, `None` if it is not well-formed.
pub(crate) fn parse(xml: &str) -> Option<Element> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    // stack[0] holds the root once read.
    let mut stack = vec![Element::default()];
    loop {
        match reader.read_event().ok()? {
            Event::Start(e) => stack.push(start(&e)),
            Event::Empty(e) => {
                let el = start(&e);
                stack.last_mut()?.children.push(el);
            }
            Event::End(_) => {
                let el = stack.pop()?;
                stack.last_mut()?.children.push(el);
            }
            Event::Text(t) => stack.last_mut()?.text.push_str(&t.xml10_content()),
            Event::CData(t) => stack.last_mut()?.text.push_str(&t.xml10_content()),
            Event::GeneralRef(r) => {
                if let Some(c) = entity(&r.xml10_content()) {
                    stack.last_mut()?.text.push(c);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if stack.len() != 1 {
        return None;
    }
    stack.pop()?.children.into_iter().next()
}

impl Element {
    /// First child named `name`.
    pub(crate) fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    /// First descendant (or itself) named `name`, depth first.
    pub(crate) fn find(&self, name: &str) -> Option<&Element> {
        if self.name == name {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(name))
    }

    /// Every descendant named `name` (not looking inside them).
    pub(crate) fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Element>) {
        for c in &self.children {
            if c.name == name {
                out.push(c);
            } else {
                c.find_all(name, out);
            }
        }
    }

    /// Attribute `name`.
    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Text of the descendant at `path` (child names), if not empty.
    pub(crate) fn text_at(&self, path: &[&str]) -> Option<&str> {
        let mut el = self;
        for name in path {
            el = el.child(name)?;
        }
        Some(el.text.trim()).filter(|t| !t.is_empty())
    }
}

/// Escapes `s` for XML text.
pub(crate) fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_soap_with_any_prefix_and_entities() {
        let xml = r#"<?xml version="1.0"?>
            <SOAP-ENV:Envelope xmlns:SOAP-ENV="x" xmlns:trt="y" xmlns:tt="z">
              <SOAP-ENV:Body><trt:GetStreamUriResponse><trt:MediaUri>
                <tt:Uri>rtsp://192.168.1.21:554/cam/realmonitor?channel=1&amp;subtype=0</tt:Uri>
              </trt:MediaUri></trt:GetStreamUriResponse>
              <trt:Profiles token="Profile_1" fixed="true"><tt:Name>main</tt:Name></trt:Profiles>
              <tt:Empty a="1"/></SOAP-ENV:Body>
            </SOAP-ENV:Envelope>"#;
        let root = parse(xml).expect("parse");
        assert_eq!(root.name, "Envelope");
        assert_eq!(
            root.find("MediaUri").and_then(|m| m.text_at(&["Uri"])),
            Some("rtsp://192.168.1.21:554/cam/realmonitor?channel=1&subtype=0")
        );
        let p = root.find("Profiles").expect("profiles");
        assert_eq!(p.attr("token"), Some("Profile_1"));
        assert_eq!(p.text_at(&["Name"]), Some("main"));
        assert_eq!(root.find("Empty").and_then(|e| e.attr("a")), Some("1"));
        assert!(parse("<a><b></a>").is_none());
        assert_eq!(entity("#233"), Some('é'));
        assert_eq!(escape("a<b&\"c\""), "a&lt;b&amp;&quot;c&quot;");
    }
}
