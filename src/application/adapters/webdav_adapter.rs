use crate::application::dtos::file_dto::FileDto;
use crate::application::dtos::folder_dto::FolderDto;
use chrono::Utc;
use quick_xml::{
    Reader, Writer,
    events::{BytesEnd, BytesStart, BytesText, Event},
};
/**
 * WebDAV Adapter Module
 *
 * This module provides conversion between WebDAV protocol XML structures and OxiCloud domain objects.
 * It handles parsing WebDAV request XML and generating WebDAV response XML according to RFC 4918.
 */
use std::io::{BufReader, Read, Write};

/// Result type for WebDAV operations
pub type Result<T> = std::result::Result<T, WebDavError>;

/// Error type for WebDAV operations
#[derive(Debug)]
pub enum WebDavError {
    XmlError(quick_xml::Error),
    IoError(std::io::Error),
    ParseError(String),
}

impl From<quick_xml::Error> for WebDavError {
    fn from(err: quick_xml::Error) -> Self {
        WebDavError::XmlError(err)
    }
}

impl From<std::io::Error> for WebDavError {
    fn from(err: std::io::Error) -> Self {
        WebDavError::IoError(err)
    }
}

impl std::fmt::Display for WebDavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebDavError::XmlError(e) => write!(f, "XML error: {}", e),
            WebDavError::IoError(e) => write!(f, "IO error: {}", e),
            WebDavError::ParseError(msg) => write!(f, "Parse error: {}", msg),
        }
    }
}

/// Qualified name with namespace and local name
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct QualifiedName {
    pub namespace: String,
    pub name: String,
}

impl QualifiedName {
    pub fn new<S: Into<String>>(namespace: S, name: S) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }
}

impl std::fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.namespace.is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{{{}}}{}", self.namespace, self.name)
        }
    }
}

/// PROPFIND request type
#[derive(Debug, PartialEq)]
pub enum PropFindType {
    /// Request all properties
    AllProp,
    /// Request property names only
    PropName,
    /// Request specific properties
    Prop(Vec<QualifiedName>),
}

/// PROPFIND request
#[derive(Debug)]
pub struct PropFindRequest {
    pub prop_find_type: PropFindType,
}

/// WebDAV property value
#[derive(Debug, Clone)]
pub struct PropValue {
    pub name: QualifiedName,
    pub value: Option<String>,
}

/// A single PROPPATCH operation (preserves document order per RFC 4918 §9.2).
#[derive(Debug, Clone)]
pub enum PropPatchOp {
    Set(PropValue),
    Remove(QualifiedName),
}

/// WebDAV lock information
#[derive(Debug, Clone)]
pub struct LockInfo {
    pub token: String,
    pub owner: Option<String>,
    pub depth: String,
    pub timeout: Option<String>,
    pub scope: LockScope,
    pub type_: LockType,
}

/// Lock scope (exclusive or shared)
#[derive(Debug, Clone, PartialEq)]
pub enum LockScope {
    Exclusive,
    Shared,
}

/// Lock type (currently only write)
#[derive(Debug, Clone, PartialEq)]
pub enum LockType {
    Write,
}

/// Extra property context for Nextcloud/ownCloud WebDAV extensions.
#[derive(Debug, Clone)]
pub struct NextcloudPropContext {
    pub file_id: Option<i64>,
    pub oc_id: Option<String>,
    pub owner_id: Option<String>,
    pub owner_display_name: Option<String>,
    pub permissions: String,
    pub size: u64,
    pub has_preview: bool,
    pub is_encrypted: bool,
    pub mount_type: String,
    pub contained_file_count: u64,
    pub contained_folder_count: u64,
}

impl NextcloudPropContext {
    pub fn for_folder(
        file_id: Option<i64>,
        oc_id: Option<String>,
        owner: &str,
        contained_files: u64,
        contained_folders: u64,
    ) -> Self {
        Self {
            file_id,
            oc_id,
            owner_id: Some(owner.to_string()),
            owner_display_name: Some(owner.to_string()),
            permissions: "RGDNVCK".to_string(),
            size: 0,
            has_preview: false,
            is_encrypted: false,
            mount_type: "dir".to_string(),
            contained_file_count: contained_files,
            contained_folder_count: contained_folders,
        }
    }

    pub fn for_file(file_id: Option<i64>, oc_id: Option<String>, owner: &str, size: u64) -> Self {
        Self {
            file_id,
            oc_id,
            owner_id: Some(owner.to_string()),
            owner_display_name: Some(owner.to_string()),
            permissions: "RGDNVW".to_string(),
            size,
            has_preview: false,
            is_encrypted: false,
            mount_type: "file".to_string(),
            contained_file_count: 0,
            contained_folder_count: 0,
        }
    }
}

/// WebDAV adapter for converting between XML and domain objects
pub struct WebDavAdapter;

impl WebDavAdapter {
    /// Collect namespace prefix → URI mappings from element attributes.
    /// E.g. `xmlns:D="DAV:"` maps prefix `"D"` to `"DAV:"`.
    pub fn collect_ns_decls(
        e: &BytesStart,
        ns_map: &mut std::collections::HashMap<String, String>,
    ) {
        for attr in e.attributes().flatten() {
            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
            if let Some(prefix) = key.strip_prefix("xmlns:") {
                let uri = attr.unescape_value().unwrap_or_default().to_string();
                ns_map.insert(prefix.to_string(), uri);
            } else if key == "xmlns" {
                // Default namespace declaration: xmlns="uri"
                let uri = attr.unescape_value().unwrap_or_default().to_string();
                ns_map.insert(String::new(), uri);
            }
        }
    }

    /// Reject `xmlns:prefix=""` declarations — binding a prefix to an empty URI
    /// is forbidden by the XML Namespaces 1.0 spec (RFC 4918 §8.1 requires 400).
    fn check_ns_decls_valid(e: &BytesStart) -> Result<()> {
        for attr in e.attributes().flatten() {
            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
            if key.starts_with("xmlns:") {
                let uri = attr.unescape_value().unwrap_or_default();
                if uri.is_empty() {
                    return Err(WebDavError::ParseError(
                        "Invalid namespace declaration: prefix bound to empty URI".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Resolve a prefixed element name (e.g. `D:resourcetype`) to a
    /// `QualifiedName` using the accumulated namespace declarations.
    pub fn resolve_name(
        name_str: &str,
        ns_map: &std::collections::HashMap<String, String>,
    ) -> QualifiedName {
        if let Some(idx) = name_str.find(':') {
            let prefix = &name_str[..idx];
            let local = &name_str[idx + 1..];
            if let Some(uri) = ns_map.get(prefix) {
                return QualifiedName::new(uri.clone(), local.to_string());
            }
        }
        // No prefix: check for a default namespace (xmlns="...").
        // An empty string means xmlns="" — null namespace override, which is valid.
        if let Some(default_ns) = ns_map.get("") {
            return QualifiedName::new(default_ns.clone(), name_str.to_string());
        }
        // Fallback: no prefix or unknown prefix → use legacy extraction
        QualifiedName::new(
            Self::extract_namespace(name_str),
            Self::extract_local_name(name_str),
        )
    }

    /// Parse a PROPFIND XML request
    pub fn parse_propfind<R: Read>(reader: R) -> Result<PropFindRequest> {
        let mut xml_reader = Reader::from_reader(BufReader::new(reader));
        xml_reader.config_mut().trim_text(true);

        let mut buffer = Vec::new();
        let mut in_propfind = false;
        let mut saw_propfind_close = false;
        let mut in_prop = false;
        let mut in_allprop = false;
        let mut in_propname = false;
        let mut props = Vec::new();
        let mut ns_map = std::collections::HashMap::<String, String>::new();

        loop {
            match xml_reader.read_event_into(&mut buffer) {
                Ok(Event::Start(ref e)) => {
                    Self::collect_ns_decls(e, &mut ns_map);
                    Self::check_ns_decls_valid(e)?;
                    let name = e.name();
                    let name_str = std::str::from_utf8(name.as_ref()).unwrap_or("");

                    if name_str == "propfind" || name_str.ends_with(":propfind") {
                        in_propfind = true;
                    } else if in_propfind && (name_str == "prop" || name_str.ends_with(":prop")) {
                        in_prop = true;
                    } else if in_propfind
                        && (name_str == "allprop" || name_str.ends_with(":allprop"))
                    {
                        in_allprop = true;
                    } else if in_propfind
                        && (name_str == "propname" || name_str.ends_with(":propname"))
                    {
                        in_propname = true;
                    } else if in_prop {
                        let qname = Self::resolve_name(name_str, &ns_map);
                        props.push(qname);
                    }
                }
                Ok(Event::End(ref e)) => {
                    let name = e.name();
                    let name_str = std::str::from_utf8(name.as_ref()).unwrap_or("");

                    if name_str == "propfind" || name_str.ends_with(":propfind") {
                        in_propfind = false;
                        saw_propfind_close = true;
                    } else if name_str == "prop" || name_str.ends_with(":prop") {
                        in_prop = false;
                    } else if name_str == "allprop" || name_str.ends_with(":allprop") {
                        in_allprop = false;
                    } else if name_str == "propname" || name_str.ends_with(":propname") {
                        in_propname = false;
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    Self::collect_ns_decls(e, &mut ns_map);
                    Self::check_ns_decls_valid(e)?;
                    let name = e.name();
                    let name_str = std::str::from_utf8(name.as_ref()).unwrap_or("");

                    if in_propfind && (name_str == "allprop" || name_str.ends_with(":allprop")) {
                        in_allprop = true;
                    } else if in_propfind
                        && (name_str == "propname" || name_str.ends_with(":propname"))
                    {
                        in_propname = true;
                    } else if in_prop {
                        let qname = Self::resolve_name(name_str, &ns_map);
                        props.push(qname);
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(WebDavError::XmlError(e)),
                _ => (),
            }

            buffer.clear();
        }

        // RFC 4918 §8.1: non-well-formed XML MUST produce 400. quick-xml is
        // lenient about EOF-inside-element (no XmlError on unclosed tags), so
        // check explicitly: body must contain a complete <propfind>…</propfind>.
        if !saw_propfind_close {
            return Err(WebDavError::ParseError(
                "PROPFIND body is not well-formed XML: missing or unclosed <propfind> element"
                    .to_string(),
            ));
        }

        let prop_find_type = if in_allprop {
            PropFindType::AllProp
        } else if in_propname {
            PropFindType::PropName
        } else {
            PropFindType::Prop(props)
        };

        Ok(PropFindRequest { prop_find_type })
    }

    fn folder_prop_is_known(prop: &QualifiedName) -> bool {
        prop.namespace == "DAV:"
            && matches!(
                prop.name.as_str(),
                "resourcetype"
                    | "displayname"
                    | "creationdate"
                    | "getlastmodified"
                    | "getetag"
                    | "getcontentlength"
                    | "getcontenttype"
            )
    }

    fn file_prop_is_known(prop: &QualifiedName) -> bool {
        prop.namespace == "DAV:"
            && matches!(
                prop.name.as_str(),
                "resourcetype"
                    | "displayname"
                    | "getcontenttype"
                    | "getcontentlength"
                    | "creationdate"
                    | "getlastmodified"
                    | "getetag"
            )
    }

    /// Write a single qualified name as an empty XML element with proper namespace declaration.
    ///
    /// DAV: props use the `D:` prefix (already declared on the root element).
    /// All other namespaces get a local `xmlns:X` declaration on the element itself.
    fn write_qname_empty<W: Write>(xml_writer: &mut Writer<W>, prop: &QualifiedName) -> Result<()> {
        if prop.namespace.is_empty() {
            xml_writer.write_event(Event::Empty(BytesStart::new(prop.name.as_str())))?;
        } else if prop.namespace == "DAV:" {
            xml_writer.write_event(Event::Empty(BytesStart::new(format!("D:{}", prop.name))))?;
        } else {
            let tag = format!("X:{}", prop.name);
            let mut start = BytesStart::new(tag.as_str());
            start.push_attribute(("xmlns:X", prop.namespace.as_str()));
            xml_writer.write_event(Event::Empty(start))?;
        }
        Ok(())
    }

    /// Write a 404 propstat block for unknown properties (RFC 4918 §9.2).
    fn write_unknown_props_404<W: Write>(
        xml_writer: &mut Writer<W>,
        unknown: &[&QualifiedName],
    ) -> Result<()> {
        if unknown.is_empty() {
            return Ok(());
        }
        xml_writer.write_event(Event::Start(BytesStart::new("D:propstat")))?;
        xml_writer.write_event(Event::Start(BytesStart::new("D:prop")))?;
        for prop in unknown {
            Self::write_qname_empty(xml_writer, prop)?;
        }
        xml_writer.write_event(Event::End(BytesEnd::new("D:prop")))?;
        xml_writer.write_event(Event::Start(BytesStart::new("D:status")))?;
        xml_writer.write_event(Event::Text(BytesText::new("HTTP/1.1 404 Not Found")))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:status")))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:propstat")))?;
        Ok(())
    }

    /// Write a dead-property propstat block (RFC 4918 §4.2).
    ///
    /// Written AFTER the live-property propstats inside a `<D:response>`.
    /// Only emitted when `dead_props` is non-empty.
    fn write_dead_props_propstat<W: Write>(
        xml_writer: &mut Writer<W>,
        dead_props: &[(QualifiedName, Option<String>)],
    ) -> Result<()> {
        if dead_props.is_empty() {
            return Ok(());
        }
        xml_writer.write_event(Event::Start(BytesStart::new("D:propstat")))?;
        xml_writer.write_event(Event::Start(BytesStart::new("D:prop")))?;
        for (name, value) in dead_props {
            let tag = if name.namespace.is_empty() {
                name.name.clone()
            } else {
                format!("X:{}", name.name)
            };
            let mut start = BytesStart::new(tag.as_str());
            if !name.namespace.is_empty() {
                start.push_attribute(("xmlns:X", name.namespace.as_str()));
            }
            match value {
                Some(v) if !v.is_empty() => {
                    xml_writer.write_event(Event::Start(start))?;
                    xml_writer.write_event(Event::Text(BytesText::new(v)))?;
                    xml_writer.write_event(Event::End(BytesEnd::new(tag.as_str())))?;
                }
                _ => {
                    xml_writer.write_event(Event::Empty(start))?;
                }
            }
        }
        xml_writer.write_event(Event::End(BytesEnd::new("D:prop")))?;
        xml_writer.write_event(Event::Start(BytesStart::new("D:status")))?;
        xml_writer.write_event(Event::Text(BytesText::new("HTTP/1.1 200 OK")))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:status")))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:propstat")))?;
        Ok(())
    }

    /// Write folder properties as a response
    fn write_folder_response<W: Write>(
        xml_writer: &mut Writer<W>,
        folder: &FolderDto,
        request: &PropFindRequest,
        href: &str,
    ) -> Result<()> {
        Self::write_folder_response_with_dead_props(xml_writer, folder, request, href, &[])
    }

    fn write_folder_response_with_dead_props<W: Write>(
        xml_writer: &mut Writer<W>,
        folder: &FolderDto,
        request: &PropFindRequest,
        href: &str,
        dead_props: &[(QualifiedName, Option<String>)],
    ) -> Result<()> {
        xml_writer.write_event(Event::Start(BytesStart::new("D:response")))?;

        xml_writer.write_event(Event::Start(BytesStart::new("D:href")))?;
        xml_writer.write_event(Event::Text(BytesText::new(href)))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:href")))?;

        // Compute dead props first so we can exclude them from the 404 propstat.
        let relevant_dead: Vec<_> = match &request.prop_find_type {
            PropFindType::Prop(requested) => dead_props
                .iter()
                .filter(|(name, _)| requested.iter().any(|r| r == name))
                .cloned()
                .collect(),
            PropFindType::AllProp => dead_props.to_vec(),
            PropFindType::PropName => vec![],
        };
        let dead_name_set: std::collections::HashSet<&QualifiedName> =
            relevant_dead.iter().map(|(n, _)| n).collect();

        match &request.prop_find_type {
            PropFindType::Prop(props) => {
                // RFC 4918 §9.2: known props → 200 propstat; unknown → 404 propstat.
                // Props found in the dead store are returned in the dead 200 propstat,
                // so exclude them from the 404 propstat to avoid duplicate reporting.
                let (known, unknown): (Vec<_>, Vec<_>) =
                    props.iter().partition(|p| Self::folder_prop_is_known(p));
                let truly_unknown: Vec<_> = unknown
                    .into_iter()
                    .filter(|p| !dead_name_set.contains(*p))
                    .collect();

                xml_writer.write_event(Event::Start(BytesStart::new("D:propstat")))?;
                xml_writer.write_event(Event::Start(BytesStart::new("D:prop")))?;
                Self::write_folder_requested_props(xml_writer, folder, &known)?;
                xml_writer.write_event(Event::End(BytesEnd::new("D:prop")))?;
                xml_writer.write_event(Event::Start(BytesStart::new("D:status")))?;
                xml_writer.write_event(Event::Text(BytesText::new("HTTP/1.1 200 OK")))?;
                xml_writer.write_event(Event::End(BytesEnd::new("D:status")))?;
                xml_writer.write_event(Event::End(BytesEnd::new("D:propstat")))?;

                Self::write_unknown_props_404(xml_writer, &truly_unknown)?;
            }
            other => {
                xml_writer.write_event(Event::Start(BytesStart::new("D:propstat")))?;
                xml_writer.write_event(Event::Start(BytesStart::new("D:prop")))?;
                match other {
                    PropFindType::AllProp => {
                        Self::write_folder_standard_props(xml_writer, folder)?;
                    }
                    PropFindType::PropName => {
                        Self::write_folder_prop_names(xml_writer)?;
                    }
                    PropFindType::Prop(_) => unreachable!(),
                }
                xml_writer.write_event(Event::End(BytesEnd::new("D:prop")))?;
                xml_writer.write_event(Event::Start(BytesStart::new("D:status")))?;
                xml_writer.write_event(Event::Text(BytesText::new("HTTP/1.1 200 OK")))?;
                xml_writer.write_event(Event::End(BytesEnd::new("D:status")))?;
                xml_writer.write_event(Event::End(BytesEnd::new("D:propstat")))?;
            }
        }

        // Dead properties — written as a separate 200 propstat (RFC 4918 §4.2).
        Self::write_dead_props_propstat(xml_writer, &relevant_dead)?;

        xml_writer.write_event(Event::End(BytesEnd::new("D:response")))?;
        Ok(())
    }

    /// Write file properties as a response
    fn write_file_response<W: Write>(
        xml_writer: &mut Writer<W>,
        file: &FileDto,
        request: &PropFindRequest,
        href: &str,
    ) -> Result<()> {
        Self::write_file_response_with_dead_props(xml_writer, file, request, href, &[])
    }

    fn write_file_response_with_dead_props<W: Write>(
        xml_writer: &mut Writer<W>,
        file: &FileDto,
        request: &PropFindRequest,
        href: &str,
        dead_props: &[(QualifiedName, Option<String>)],
    ) -> Result<()> {
        xml_writer.write_event(Event::Start(BytesStart::new("D:response")))?;

        xml_writer.write_event(Event::Start(BytesStart::new("D:href")))?;
        xml_writer.write_event(Event::Text(BytesText::new(href)))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:href")))?;

        // Compute dead props first so we can exclude them from the 404 propstat.
        let relevant_dead: Vec<_> = match &request.prop_find_type {
            PropFindType::Prop(requested) => dead_props
                .iter()
                .filter(|(name, _)| requested.iter().any(|r| r == name))
                .cloned()
                .collect(),
            PropFindType::AllProp => dead_props.to_vec(),
            PropFindType::PropName => vec![],
        };
        let dead_name_set: std::collections::HashSet<&QualifiedName> =
            relevant_dead.iter().map(|(n, _)| n).collect();

        match &request.prop_find_type {
            PropFindType::Prop(props) => {
                // RFC 4918 §9.2: known props → 200 propstat; unknown → 404 propstat.
                // Props found in the dead store are returned in the dead 200 propstat,
                // so exclude them from the 404 propstat to avoid duplicate reporting.
                let (known, unknown): (Vec<_>, Vec<_>) =
                    props.iter().partition(|p| Self::file_prop_is_known(p));
                let truly_unknown: Vec<_> = unknown
                    .into_iter()
                    .filter(|p| !dead_name_set.contains(*p))
                    .collect();

                xml_writer.write_event(Event::Start(BytesStart::new("D:propstat")))?;
                xml_writer.write_event(Event::Start(BytesStart::new("D:prop")))?;
                Self::write_file_requested_props(xml_writer, file, &known)?;
                xml_writer.write_event(Event::End(BytesEnd::new("D:prop")))?;
                xml_writer.write_event(Event::Start(BytesStart::new("D:status")))?;
                xml_writer.write_event(Event::Text(BytesText::new("HTTP/1.1 200 OK")))?;
                xml_writer.write_event(Event::End(BytesEnd::new("D:status")))?;
                xml_writer.write_event(Event::End(BytesEnd::new("D:propstat")))?;

                Self::write_unknown_props_404(xml_writer, &truly_unknown)?;
            }
            other => {
                xml_writer.write_event(Event::Start(BytesStart::new("D:propstat")))?;
                xml_writer.write_event(Event::Start(BytesStart::new("D:prop")))?;
                match other {
                    PropFindType::AllProp => {
                        Self::write_file_standard_props(xml_writer, file)?;
                    }
                    PropFindType::PropName => {
                        Self::write_file_prop_names(xml_writer)?;
                    }
                    PropFindType::Prop(_) => unreachable!(),
                }
                xml_writer.write_event(Event::End(BytesEnd::new("D:prop")))?;
                xml_writer.write_event(Event::Start(BytesStart::new("D:status")))?;
                xml_writer.write_event(Event::Text(BytesText::new("HTTP/1.1 200 OK")))?;
                xml_writer.write_event(Event::End(BytesEnd::new("D:status")))?;
                xml_writer.write_event(Event::End(BytesEnd::new("D:propstat")))?;
            }
        }

        // Dead properties (RFC 4918 §4.2).
        Self::write_dead_props_propstat(xml_writer, &relevant_dead)?;

        xml_writer.write_event(Event::End(BytesEnd::new("D:response")))?;
        Ok(())
    }

    /// Write standard folder properties
    fn write_folder_standard_props<W: Write>(
        xml_writer: &mut Writer<W>,
        folder: &FolderDto,
    ) -> Result<()> {
        // Resource type (collection)
        xml_writer.write_event(Event::Start(BytesStart::new("D:resourcetype")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:collection")))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:resourcetype")))?;

        // Display name
        xml_writer.write_event(Event::Start(BytesStart::new("D:displayname")))?;
        xml_writer.write_event(Event::Text(BytesText::new(&folder.name)))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:displayname")))?;

        // Creation date
        xml_writer.write_event(Event::Start(BytesStart::new("D:creationdate")))?;

        // Convert u64 timestamp to DateTime
        let created_at = chrono::DateTime::<Utc>::from_timestamp(folder.created_at as i64, 0)
            .unwrap_or_else(Utc::now);

        xml_writer.write_event(Event::Text(BytesText::new(&created_at.to_rfc3339())))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:creationdate")))?;

        // Last modified
        xml_writer.write_event(Event::Start(BytesStart::new("D:getlastmodified")))?;

        // Convert u64 timestamp to DateTime
        let modified_at = chrono::DateTime::<Utc>::from_timestamp(folder.modified_at as i64, 0)
            .unwrap_or_else(Utc::now);

        xml_writer.write_event(Event::Text(BytesText::new(&modified_at.to_rfc2822())))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:getlastmodified")))?;

        // ETag — routes through `FolderDto::etag` (= `Folder::etag()`)
        // so every WebDAV emitter and HEAD response agree on a single
        // value for the same folder.
        xml_writer.write_event(Event::Start(BytesStart::new("D:getetag")))?;
        xml_writer.write_event(Event::Text(BytesText::new(&format!("\"{}\"", folder.etag))))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:getetag")))?;

        // Content length (0 for directories)
        xml_writer.write_event(Event::Start(BytesStart::new("D:getcontentlength")))?;
        xml_writer.write_event(Event::Text(BytesText::new("0")))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:getcontentlength")))?;

        // Content type for directories
        xml_writer.write_event(Event::Start(BytesStart::new("D:getcontenttype")))?;
        xml_writer.write_event(Event::Text(BytesText::new("httpd/unix-directory")))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:getcontenttype")))?;

        Ok(())
    }

    /// Write standard file properties
    fn write_file_standard_props<W: Write>(
        xml_writer: &mut Writer<W>,
        file: &FileDto,
    ) -> Result<()> {
        // Resource type (empty for files)
        xml_writer.write_event(Event::Empty(BytesStart::new("D:resourcetype")))?;

        // Display name
        xml_writer.write_event(Event::Start(BytesStart::new("D:displayname")))?;
        xml_writer.write_event(Event::Text(BytesText::new(&file.name)))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:displayname")))?;

        // Content type
        xml_writer.write_event(Event::Start(BytesStart::new("D:getcontenttype")))?;
        xml_writer.write_event(Event::Text(BytesText::new(&file.mime_type)))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:getcontenttype")))?;

        // Content length
        xml_writer.write_event(Event::Start(BytesStart::new("D:getcontentlength")))?;
        xml_writer.write_event(Event::Text(BytesText::new(&file.size.to_string())))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:getcontentlength")))?;

        // Creation date
        xml_writer.write_event(Event::Start(BytesStart::new("D:creationdate")))?;

        // Convert u64 timestamp to DateTime
        let created_at = chrono::DateTime::<Utc>::from_timestamp(file.created_at as i64, 0)
            .unwrap_or_else(Utc::now);

        xml_writer.write_event(Event::Text(BytesText::new(&created_at.to_rfc3339())))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:creationdate")))?;

        // Last modified
        xml_writer.write_event(Event::Start(BytesStart::new("D:getlastmodified")))?;

        // Convert u64 timestamp to DateTime
        let modified_at = chrono::DateTime::<Utc>::from_timestamp(file.modified_at as i64, 0)
            .unwrap_or_else(Utc::now);

        xml_writer.write_event(Event::Text(BytesText::new(&modified_at.to_rfc2822())))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:getlastmodified")))?;

        // ETag — routes through `FileDto::etag` (= `File::etag()`) so
        // PROPFIND, GET, HEAD, PUT-response, and MOVE all emit
        // byte-identical values for the same file.
        xml_writer.write_event(Event::Start(BytesStart::new("D:getetag")))?;
        xml_writer.write_event(Event::Text(BytesText::new(&format!("\"{}\"", file.etag))))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:getetag")))?;

        Ok(())
    }

    /// Write folder property names
    fn write_folder_prop_names<W: Write>(xml_writer: &mut Writer<W>) -> Result<()> {
        // Write empty property elements for folders
        xml_writer.write_event(Event::Empty(BytesStart::new("D:resourcetype")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:displayname")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:creationdate")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:getlastmodified")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:getetag")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:getcontentlength")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:getcontenttype")))?;

        Ok(())
    }

    /// Write file property names
    fn write_file_prop_names<W: Write>(xml_writer: &mut Writer<W>) -> Result<()> {
        // Write empty property elements for files
        xml_writer.write_event(Event::Empty(BytesStart::new("D:resourcetype")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:displayname")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:getcontenttype")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:getcontentlength")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:creationdate")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:getlastmodified")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:getetag")))?;

        Ok(())
    }

    /// Write requested folder properties
    fn write_folder_requested_props<W: Write>(
        xml_writer: &mut Writer<W>,
        folder: &FolderDto,
        props: &[&QualifiedName],
    ) -> Result<()> {
        for prop in props {
            if prop.namespace == "DAV:" {
                match prop.name.as_str() {
                    "resourcetype" => {
                        xml_writer.write_event(Event::Start(BytesStart::new("D:resourcetype")))?;
                        xml_writer.write_event(Event::Empty(BytesStart::new("D:collection")))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:resourcetype")))?;
                    }
                    "displayname" => {
                        xml_writer.write_event(Event::Start(BytesStart::new("D:displayname")))?;
                        xml_writer.write_event(Event::Text(BytesText::new(&folder.name)))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:displayname")))?;
                    }
                    "creationdate" => {
                        xml_writer.write_event(Event::Start(BytesStart::new("D:creationdate")))?;

                        // Convert u64 timestamp to DateTime
                        let created_at =
                            chrono::DateTime::<Utc>::from_timestamp(folder.created_at as i64, 0)
                                .unwrap_or_else(Utc::now);

                        xml_writer
                            .write_event(Event::Text(BytesText::new(&created_at.to_rfc3339())))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:creationdate")))?;
                    }
                    "getlastmodified" => {
                        xml_writer
                            .write_event(Event::Start(BytesStart::new("D:getlastmodified")))?;

                        // Convert u64 timestamp to DateTime
                        let modified_at =
                            chrono::DateTime::<Utc>::from_timestamp(folder.modified_at as i64, 0)
                                .unwrap_or_else(Utc::now);

                        xml_writer
                            .write_event(Event::Text(BytesText::new(&modified_at.to_rfc2822())))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:getlastmodified")))?;
                    }
                    "getetag" => {
                        xml_writer.write_event(Event::Start(BytesStart::new("D:getetag")))?;
                        xml_writer.write_event(Event::Text(BytesText::new(&format!(
                            "\"{}\"",
                            folder.etag
                        ))))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:getetag")))?;
                    }
                    "getcontentlength" => {
                        xml_writer
                            .write_event(Event::Start(BytesStart::new("D:getcontentlength")))?;
                        xml_writer.write_event(Event::Text(BytesText::new("0")))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:getcontentlength")))?;
                    }
                    "getcontenttype" => {
                        xml_writer
                            .write_event(Event::Start(BytesStart::new("D:getcontenttype")))?;
                        xml_writer
                            .write_event(Event::Text(BytesText::new("httpd/unix-directory")))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:getcontenttype")))?;
                    }
                    _ => {
                        // Unknown prop — skipped here; caller writes 404 propstat.
                    }
                }
            }
            // Non-DAV namespace props are unknown — skipped; caller writes 404 propstat.
        }

        Ok(())
    }

    /// Write requested file properties
    fn write_file_requested_props<W: Write>(
        xml_writer: &mut Writer<W>,
        file: &FileDto,
        props: &[&QualifiedName],
    ) -> Result<()> {
        for prop in props {
            if prop.namespace == "DAV:" {
                match prop.name.as_str() {
                    "resourcetype" => {
                        xml_writer.write_event(Event::Empty(BytesStart::new("D:resourcetype")))?;
                    }
                    "displayname" => {
                        xml_writer.write_event(Event::Start(BytesStart::new("D:displayname")))?;
                        xml_writer.write_event(Event::Text(BytesText::new(&file.name)))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:displayname")))?;
                    }
                    "getcontenttype" => {
                        xml_writer
                            .write_event(Event::Start(BytesStart::new("D:getcontenttype")))?;
                        xml_writer.write_event(Event::Text(BytesText::new(&file.mime_type)))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:getcontenttype")))?;
                    }
                    "getcontentlength" => {
                        xml_writer
                            .write_event(Event::Start(BytesStart::new("D:getcontentlength")))?;
                        xml_writer
                            .write_event(Event::Text(BytesText::new(&file.size.to_string())))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:getcontentlength")))?;
                    }
                    "creationdate" => {
                        xml_writer.write_event(Event::Start(BytesStart::new("D:creationdate")))?;

                        // Convert u64 timestamp to DateTime
                        let created_at =
                            chrono::DateTime::<Utc>::from_timestamp(file.created_at as i64, 0)
                                .unwrap_or_else(Utc::now);

                        xml_writer
                            .write_event(Event::Text(BytesText::new(&created_at.to_rfc3339())))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:creationdate")))?;
                    }
                    "getlastmodified" => {
                        xml_writer
                            .write_event(Event::Start(BytesStart::new("D:getlastmodified")))?;

                        // Convert u64 timestamp to DateTime
                        let modified_at =
                            chrono::DateTime::<Utc>::from_timestamp(file.modified_at as i64, 0)
                                .unwrap_or_else(Utc::now);

                        xml_writer
                            .write_event(Event::Text(BytesText::new(&modified_at.to_rfc2822())))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:getlastmodified")))?;
                    }
                    "getetag" => {
                        xml_writer.write_event(Event::Start(BytesStart::new("D:getetag")))?;
                        xml_writer.write_event(Event::Text(BytesText::new(&format!(
                            "\"{}\"",
                            file.etag
                        ))))?;
                        xml_writer.write_event(Event::End(BytesEnd::new("D:getetag")))?;
                    }
                    _ => {
                        // Unknown prop — skipped here; caller writes 404 propstat.
                    }
                }
            }
            // Non-DAV namespace props are unknown — skipped; caller writes 404 propstat.
        }

        Ok(())
    }

    /// Parse a PROPPATCH XML request.
    ///
    /// Returns operations in document order (RFC 4918 §9.2 requires document-order
    /// processing so that remove-then-set and set-then-remove yield different results).
    pub fn parse_proppatch<R: Read>(reader: R) -> Result<Vec<PropPatchOp>> {
        let mut xml_reader = Reader::from_reader(BufReader::new(reader));
        xml_reader.config_mut().trim_text(true);

        let mut buffer = Vec::new();
        let mut in_propertyupdate = false;
        let mut in_set = false;
        let mut in_remove = false;
        let mut in_prop = false;
        let mut current_prop: Option<QualifiedName> = None;
        let mut ops: Vec<PropPatchOp> = Vec::new();
        let mut current_text = String::new();
        let mut ns_map = std::collections::HashMap::<String, String>::new();

        loop {
            match xml_reader.read_event_into(&mut buffer) {
                Ok(Event::Start(ref e)) => {
                    Self::collect_ns_decls(e, &mut ns_map);
                    let name = e.name();
                    let name_str = std::str::from_utf8(name.as_ref()).unwrap_or("");

                    match name_str {
                        s if s == "propertyupdate" || s.ends_with(":propertyupdate") => {
                            in_propertyupdate = true
                        }
                        s if (in_propertyupdate && (s == "set" || s.ends_with(":set"))) => {
                            in_set = true
                        }
                        s if (in_propertyupdate && (s == "remove" || s.ends_with(":remove"))) => {
                            in_remove = true
                        }
                        s if ((in_set || in_remove) && (s == "prop" || s.ends_with(":prop"))) => {
                            in_prop = true
                        }
                        _ if in_prop => {
                            current_prop = Some(Self::resolve_name(name_str, &ns_map));
                            current_text.clear();
                        }
                        _ => (),
                    }
                }
                Ok(Event::Text(e)) if current_prop.is_some() => {
                    let raw = e.decode().unwrap_or_default();
                    let unescaped =
                        quick_xml::escape::unescape(&raw).unwrap_or_else(|_| raw.clone());
                    current_text.push_str(&unescaped);
                }
                Ok(Event::GeneralRef(ref e)) if current_prop.is_some() => {
                    // quick-xml 0.39 emits GeneralRef for character references like &#65536;
                    // and named entity references like &amp;. Resolve them to actual chars.
                    match e.resolve_char_ref() {
                        Ok(Some(ch)) => current_text.push(ch),
                        Ok(None) => {
                            if let Ok(name) = e.decode() {
                                match name.as_ref() {
                                    "amp" => current_text.push('&'),
                                    "lt" => current_text.push('<'),
                                    "gt" => current_text.push('>'),
                                    "apos" => current_text.push('\''),
                                    "quot" => current_text.push('"'),
                                    _ => {}
                                }
                            }
                        }
                        Err(_) => {}
                    }
                }
                Ok(Event::End(ref e)) => {
                    let name = e.name();
                    let name_str = std::str::from_utf8(name.as_ref()).unwrap_or("");

                    match name_str {
                        s if s == "propertyupdate" || s.ends_with(":propertyupdate") => {
                            in_propertyupdate = false
                        }
                        s if s == "set" || s.ends_with(":set") => in_set = false,
                        s if s == "remove" || s.ends_with(":remove") => in_remove = false,
                        s if s == "prop" || s.ends_with(":prop") => in_prop = false,
                        _ if in_prop => {
                            if let Some(prop_name) = current_prop.take() {
                                if in_set {
                                    ops.push(PropPatchOp::Set(PropValue {
                                        name: prop_name,
                                        value: if current_text.is_empty() {
                                            None
                                        } else {
                                            Some(current_text.clone())
                                        },
                                    }));
                                } else if in_remove {
                                    ops.push(PropPatchOp::Remove(prop_name));
                                }
                            }
                            current_text.clear();
                        }
                        _ => (),
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    Self::collect_ns_decls(e, &mut ns_map);
                    let name = e.name();
                    let name_str = std::str::from_utf8(name.as_ref()).unwrap_or("");

                    if in_prop {
                        let qname = Self::resolve_name(name_str, &ns_map);

                        if in_set {
                            ops.push(PropPatchOp::Set(PropValue {
                                name: qname,
                                value: None,
                            }));
                        } else if in_remove {
                            ops.push(PropPatchOp::Remove(qname));
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(WebDavError::XmlError(e)),
                _ => (),
            }

            buffer.clear();
        }

        Ok(ops)
    }

    /// Generate a PROPPATCH response
    pub fn generate_proppatch_response<W: Write>(
        writer: W,
        href: &str,
        results: &[(&QualifiedName, bool)],
    ) -> Result<()> {
        let mut xml_writer = Writer::new(writer);

        // Start multistatus response
        xml_writer.write_event(Event::Start(
            BytesStart::new("D:multistatus").with_attributes([("xmlns:D", "DAV:")]),
        ))?;

        // Start response element
        xml_writer.write_event(Event::Start(BytesStart::new("D:response")))?;

        // Write href
        xml_writer.write_event(Event::Start(BytesStart::new("D:href")))?;
        xml_writer.write_event(Event::Text(BytesText::new(href)))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:href")))?;

        // Group results by status
        let mut success_props = Vec::new();
        let mut failed_props = Vec::new();

        for (prop, success) in results {
            if *success {
                success_props.push(prop);
            } else {
                failed_props.push(prop);
            }
        }

        // Write successful properties
        if !success_props.is_empty() {
            xml_writer.write_event(Event::Start(BytesStart::new("D:propstat")))?;

            // Start prop
            xml_writer.write_event(Event::Start(BytesStart::new("D:prop")))?;

            // Write property names
            for prop in success_props {
                Self::write_qname_empty(&mut xml_writer, prop)?;
            }

            // End prop
            xml_writer.write_event(Event::End(BytesEnd::new("D:prop")))?;

            // Write status
            xml_writer.write_event(Event::Start(BytesStart::new("D:status")))?;
            xml_writer.write_event(Event::Text(BytesText::new("HTTP/1.1 200 OK")))?;
            xml_writer.write_event(Event::End(BytesEnd::new("D:status")))?;

            // End propstat
            xml_writer.write_event(Event::End(BytesEnd::new("D:propstat")))?;
        }

        // Write failed properties
        if !failed_props.is_empty() {
            xml_writer.write_event(Event::Start(BytesStart::new("D:propstat")))?;

            // Start prop
            xml_writer.write_event(Event::Start(BytesStart::new("D:prop")))?;

            // Write property names
            for prop in failed_props {
                Self::write_qname_empty(&mut xml_writer, prop)?;
            }

            // End prop
            xml_writer.write_event(Event::End(BytesEnd::new("D:prop")))?;

            // Write status
            xml_writer.write_event(Event::Start(BytesStart::new("D:status")))?;
            xml_writer.write_event(Event::Text(BytesText::new("HTTP/1.1 403 Forbidden")))?;
            xml_writer.write_event(Event::End(BytesEnd::new("D:status")))?;

            // End propstat
            xml_writer.write_event(Event::End(BytesEnd::new("D:propstat")))?;
        }

        // End response
        xml_writer.write_event(Event::End(BytesEnd::new("D:response")))?;

        // End multistatus
        xml_writer.write_event(Event::End(BytesEnd::new("D:multistatus")))?;

        Ok(())
    }

    /// Parse a LOCK XML request
    pub fn parse_lockinfo<R: Read>(reader: R) -> Result<(LockScope, LockType, Option<String>)> {
        let mut xml_reader = Reader::from_reader(BufReader::new(reader));
        xml_reader.config_mut().trim_text(true);

        let mut buffer = Vec::new();
        let mut in_lockinfo = false;
        let mut in_lockscope = false;
        let mut in_locktype = false;
        let mut in_owner = false;
        let mut owner_text = String::new();
        let mut scope = LockScope::Exclusive; // Default to exclusive
        let mut type_ = LockType::Write; // Default to write (only supported type)

        loop {
            match xml_reader.read_event_into(&mut buffer) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let name_str = std::str::from_utf8(name.as_ref()).unwrap_or("");

                    match name_str {
                        s if s == "lockinfo" || s.ends_with(":lockinfo") => in_lockinfo = true,
                        s if in_lockinfo && (s == "lockscope" || s.ends_with(":lockscope")) => {
                            in_lockscope = true
                        }
                        s if in_lockinfo && (s == "locktype" || s.ends_with(":locktype")) => {
                            in_locktype = true
                        }
                        s if in_lockinfo && (s == "owner" || s.ends_with(":owner")) => {
                            in_owner = true
                        }
                        s if in_lockscope && (s == "exclusive" || s.ends_with(":exclusive")) => {
                            scope = LockScope::Exclusive
                        }
                        s if in_lockscope && (s == "shared" || s.ends_with(":shared")) => {
                            scope = LockScope::Shared
                        }
                        s if in_locktype && (s == "write" || s.ends_with(":write")) => {
                            type_ = LockType::Write
                        }
                        _ => (),
                    }
                }
                Ok(Event::Text(e)) if in_owner => {
                    owner_text.push_str(&e.decode().unwrap_or_default());
                }
                Ok(Event::End(ref e)) => {
                    let name = e.name();
                    let name_str = std::str::from_utf8(name.as_ref()).unwrap_or("");

                    match name_str {
                        s if s == "lockinfo" || s.ends_with(":lockinfo") => in_lockinfo = false,
                        s if s == "lockscope" || s.ends_with(":lockscope") => in_lockscope = false,
                        s if s == "locktype" || s.ends_with(":locktype") => in_locktype = false,
                        s if s == "owner" || s.ends_with(":owner") => in_owner = false,
                        _ => (),
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let name_str = std::str::from_utf8(name.as_ref()).unwrap_or("");

                    match name_str {
                        s if in_lockscope && (s == "exclusive" || s.ends_with(":exclusive")) => {
                            scope = LockScope::Exclusive
                        }
                        s if in_lockscope && (s == "shared" || s.ends_with(":shared")) => {
                            scope = LockScope::Shared
                        }
                        s if in_locktype && (s == "write" || s.ends_with(":write")) => {
                            type_ = LockType::Write
                        }
                        _ => (),
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(WebDavError::XmlError(e)),
                _ => (),
            }

            buffer.clear();
        }

        let owner = if owner_text.is_empty() {
            None
        } else {
            Some(owner_text)
        };

        Ok((scope, type_, owner))
    }

    /// Generate a LOCK response (lockdiscovery)
    pub fn generate_lock_response<W: Write>(
        writer: W,
        lock_info: &LockInfo,
        href: &str,
    ) -> Result<()> {
        let mut xml_writer = Writer::new(writer);

        // Start prop element (direct response, not multistatus)
        xml_writer.write_event(Event::Start(
            BytesStart::new("D:prop").with_attributes([("xmlns:D", "DAV:")]),
        ))?;

        // Start lockdiscovery
        xml_writer.write_event(Event::Start(BytesStart::new("D:lockdiscovery")))?;

        // Start activelock
        xml_writer.write_event(Event::Start(BytesStart::new("D:activelock")))?;

        // Write locktype
        xml_writer.write_event(Event::Start(BytesStart::new("D:locktype")))?;
        xml_writer.write_event(Event::Empty(BytesStart::new("D:write")))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:locktype")))?;

        // Write lockscope
        xml_writer.write_event(Event::Start(BytesStart::new("D:lockscope")))?;
        match lock_info.scope {
            LockScope::Exclusive => {
                xml_writer.write_event(Event::Empty(BytesStart::new("D:exclusive")))?;
            }
            LockScope::Shared => {
                xml_writer.write_event(Event::Empty(BytesStart::new("D:shared")))?;
            }
        }
        xml_writer.write_event(Event::End(BytesEnd::new("D:lockscope")))?;

        // Write depth
        xml_writer.write_event(Event::Start(BytesStart::new("D:depth")))?;
        xml_writer.write_event(Event::Text(BytesText::new(&lock_info.depth)))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:depth")))?;

        // Write owner (if provided)
        if let Some(owner) = &lock_info.owner {
            xml_writer.write_event(Event::Start(BytesStart::new("D:owner")))?;
            xml_writer.write_event(Event::Text(BytesText::new(owner)))?;
            xml_writer.write_event(Event::End(BytesEnd::new("D:owner")))?;
        }

        // Write timeout (if provided)
        if let Some(timeout) = &lock_info.timeout {
            xml_writer.write_event(Event::Start(BytesStart::new("D:timeout")))?;
            xml_writer.write_event(Event::Text(BytesText::new(timeout)))?;
            xml_writer.write_event(Event::End(BytesEnd::new("D:timeout")))?;
        }

        // Write locktoken
        xml_writer.write_event(Event::Start(BytesStart::new("D:locktoken")))?;
        xml_writer.write_event(Event::Start(BytesStart::new("D:href")))?;
        xml_writer.write_event(Event::Text(BytesText::new(&lock_info.token)))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:href")))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:locktoken")))?;

        // Write lockroot
        xml_writer.write_event(Event::Start(BytesStart::new("D:lockroot")))?;
        xml_writer.write_event(Event::Start(BytesStart::new("D:href")))?;
        xml_writer.write_event(Event::Text(BytesText::new(href)))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:href")))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:lockroot")))?;

        // End activelock, lockdiscovery, and prop
        xml_writer.write_event(Event::End(BytesEnd::new("D:activelock")))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:lockdiscovery")))?;
        xml_writer.write_event(Event::End(BytesEnd::new("D:prop")))?;

        Ok(())
    }

    /// Helper method to extract namespace from tag name
    pub fn extract_namespace(name: &str) -> String {
        if let Some(idx) = name.rfind(':')
            && idx > 0
        {
            return name[..idx].to_string();
        }
        // Default namespace for WebDAV
        "DAV:".to_string()
    }

    /// Helper method to extract local name from tag name
    pub fn extract_local_name(name: &str) -> String {
        if let Some(idx) = name.rfind(':')
            && idx > 0
            && idx < name.len() - 1
        {
            return name[idx + 1..].to_string();
        }
        name.to_string()
    }

    // ─────────────────────────────────────────────────────────────
    // Streaming PROPFIND helpers
    //
    // These methods write incremental XML fragments so the caller
    // can flush chunks to the HTTP body without buffering the whole
    // response in memory.
    // ─────────────────────────────────────────────────────────────

    /// Writes the opening `<D:multistatus>` tag.
    pub fn write_multistatus_start<W: Write>(writer: &mut Writer<W>) -> Result<()> {
        writer.write_event(Event::Start(
            BytesStart::new("D:multistatus").with_attributes([("xmlns:D", "DAV:")]),
        ))?;
        Ok(())
    }

    /// Writes the closing `</D:multistatus>` tag.
    pub fn write_multistatus_end<W: Write>(writer: &mut Writer<W>) -> Result<()> {
        writer.write_event(Event::End(BytesEnd::new("D:multistatus")))?;
        Ok(())
    }

    /// Writes a single `<D:response>` element for a folder.
    pub fn write_folder_entry<W: Write>(
        writer: &mut Writer<W>,
        folder: &FolderDto,
        request: &PropFindRequest,
        href: &str,
    ) -> Result<()> {
        Self::write_folder_response(writer, folder, request, href)
    }

    /// Writes a single `<D:response>` element for a file, including dead properties.
    pub fn write_file_entry<W: Write>(
        writer: &mut Writer<W>,
        file: &FileDto,
        request: &PropFindRequest,
        href: &str,
    ) -> Result<()> {
        Self::write_file_response(writer, file, request, href)
    }

    /// Writes a folder entry including dead (custom) properties.
    pub fn write_folder_entry_with_dead_props<W: Write>(
        writer: &mut Writer<W>,
        folder: &FolderDto,
        request: &PropFindRequest,
        href: &str,
        dead_props: &[(QualifiedName, Option<String>)],
    ) -> Result<()> {
        Self::write_folder_response_with_dead_props(writer, folder, request, href, dead_props)
    }

    /// Writes a file entry including dead (custom) properties.
    pub fn write_file_entry_with_dead_props<W: Write>(
        writer: &mut Writer<W>,
        file: &FileDto,
        request: &PropFindRequest,
        href: &str,
        dead_props: &[(QualifiedName, Option<String>)],
    ) -> Result<()> {
        Self::write_file_response_with_dead_props(writer, file, request, href, dead_props)
    }
}
