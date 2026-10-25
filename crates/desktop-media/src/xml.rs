use crate::MediaError;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::BTreeMap;

#[derive(Debug)]
pub(crate) struct Node {
    pub name: String,
    pub attributes: BTreeMap<String, String>,
    pub text: String,
    pub children: Vec<Node>,
}

impl Node {
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|node| node.name == name)
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.child(name).map(|node| node.text.trim())
    }

    pub fn descendants<'a>(&'a self, name: &str, output: &mut Vec<&'a Node>) {
        for child in &self.children {
            if child.name == name {
                output.push(child);
            }
            child.descendants(name, output);
        }
    }
}

pub(crate) fn decode(bytes: &[u8]) -> Result<String, MediaError> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(MediaError::InvalidMetadata("XML size limit"));
    }
    let utf16 = if bytes.starts_with(&[0xff, 0xfe]) {
        Some((&bytes[2..], true))
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        Some((&bytes[2..], false))
    } else if bytes.starts_with(&[b'<', 0]) {
        Some((bytes, true))
    } else if bytes.starts_with(&[0, b'<']) {
        Some((bytes, false))
    } else {
        None
    };
    match utf16 {
        Some((bytes, little)) => {
            if bytes.len() % 2 != 0 {
                return Err(MediaError::InvalidMetadata("UTF-16 XML"));
            }
            let units: Vec<_> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| {
                    if little {
                        u16::from_le_bytes([c[0], c[1]])
                    } else {
                        u16::from_be_bytes([c[0], c[1]])
                    }
                })
                .collect();
            String::from_utf16(&units).map_err(|_| MediaError::InvalidMetadata("UTF-16 XML"))
        }
        None => std::str::from_utf8(bytes)
            .map(|s| s.trim_start_matches('\u{feff}').to_owned())
            .map_err(|_| MediaError::InvalidMetadata("UTF-8 XML")),
    }
}

pub(crate) fn parse(bytes: &[u8]) -> Result<Node, MediaError> {
    let document = decode(bytes)?;
    let mut reader = Reader::from_str(&document);
    reader.config_mut().check_end_names = true;
    let mut stack = vec![Node {
        name: "#document".into(),
        attributes: BTreeMap::new(),
        text: String::new(),
        children: Vec::new(),
    }];
    let mut count = 0usize;
    loop {
        let event = reader
            .read_event()
            .map_err(|_| MediaError::InvalidMetadata("XML document"))?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(start) | Event::Empty(start) => {
                count += 1;
                if count > 300_000 || stack.len() > 64 {
                    return Err(MediaError::InvalidMetadata("XML structural limits"));
                }
                let name = std::str::from_utf8(start.name().as_ref())
                    .map_err(|_| MediaError::InvalidMetadata("XML name"))?
                    .to_owned();
                let mut attributes = BTreeMap::new();
                for attribute in start.attributes() {
                    let attribute =
                        attribute.map_err(|_| MediaError::InvalidMetadata("XML attributes"))?;
                    let name = std::str::from_utf8(attribute.key.as_ref())
                        .map_err(|_| MediaError::InvalidMetadata("XML attribute name"))?
                        .to_owned();
                    let value = attribute
                        .decode_and_unescape_value(reader.decoder())
                        .map_err(|_| MediaError::InvalidMetadata("XML attribute value"))?;
                    if attributes.insert(name, value.into_owned()).is_some() {
                        return Err(MediaError::InvalidMetadata("duplicate XML attribute"));
                    }
                }
                let node = Node {
                    name,
                    attributes,
                    text: String::new(),
                    children: Vec::new(),
                };
                if empty {
                    stack.last_mut().unwrap().children.push(node);
                } else {
                    stack.push(node);
                }
            }
            Event::End(_) => {
                if stack.len() < 2 {
                    return Err(MediaError::InvalidMetadata("unbalanced XML document"));
                }
                let node = stack.pop().unwrap();
                stack.last_mut().unwrap().children.push(node);
            }
            Event::Text(text) => {
                let text = text
                    .xml_content()
                    .map_err(|_| MediaError::InvalidMetadata("XML text"))?;
                let text = quick_xml::escape::unescape(&text)
                    .map_err(|_| MediaError::InvalidMetadata("XML entity"))?;
                stack.last_mut().unwrap().text.push_str(&text);
            }
            Event::CData(text) => {
                let text = text
                    .decode()
                    .map_err(|_| MediaError::InvalidMetadata("XML CDATA"))?;
                stack.last_mut().unwrap().text.push_str(&text);
            }
            Event::GeneralRef(reference) => {
                let reference = reference
                    .decode()
                    .map_err(|_| MediaError::InvalidMetadata("XML entity"))?;
                let escaped = format!("&{reference};");
                let value = quick_xml::escape::unescape(&escaped)
                    .map_err(|_| MediaError::InvalidMetadata("XML entity"))?;
                stack.last_mut().unwrap().text.push_str(&value);
            }
            Event::DocType(_) => {
                return Err(MediaError::InvalidMetadata(
                    "XML document type is not allowed",
                ));
            }
            Event::Eof => break,
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
        }
    }
    if stack.len() != 1 {
        return Err(MediaError::InvalidMetadata("incomplete XML document"));
    }
    let document = stack.pop().unwrap();
    if document.children.len() != 1 || !document.text.trim().is_empty() {
        return Err(MediaError::InvalidMetadata("XML root"));
    }
    Ok(document.children.into_iter().next().unwrap())
}
