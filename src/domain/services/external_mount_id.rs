//! External mount identifiers (domain value objects).
//!
//! Files and folders *below* an external mount root have no database row. They
//! are addressed by a synthetic id that wraps a provider-owned node identity:
//!
//! ```text
//! ext:<mount_id>:<base64url(node_id)>
//! ```
//!
//! * `<mount_id>` is the mount-root folder's UUID (simple form, no hyphens) — the
//!   only part the system interprets, used to find the provider in the registry.
//! * `<node_id>` is **assigned and owned by the provider** and is **opaque** to the
//!   rest of the system. Most providers use the entry's path (`local_fs`, `sftp`,
//!   `webdav`); a provider with a stronger stable handle (inode, object id, href)
//!   may use that. It is base64url-encoded (no padding) so it survives URLs and
//!   WebDAV hrefs and never collides with the `:` / `/` separators.
//!
//! The system NEVER parses or validates a `node_id` — it only splits the envelope
//! and hands the decoded bytes back to the provider verbatim.

use std::fmt;
use std::str::FromStr;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use uuid::Uuid;

/// The `ext:` scheme prefix that marks a synthetic external-mount id.
pub const EXTERNAL_ID_PREFIX: &str = "ext:";

/// A provider-owned, opaque node identity for one entry inside a mount.
///
/// The system treats this as opaque bytes. For path-based providers it is the
/// POSIX path relative to the mount root (no leading `/`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct NodeId(pub String);

impl NodeId {
    /// Borrow the inner string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume into the inner string.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<String> for NodeId {
    fn from(s: String) -> Self {
        NodeId(s)
    }
}

impl From<&str> for NodeId {
    fn from(s: &str) -> Self {
        NodeId(s.to_owned())
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The decoded parts of a synthetic external-mount child id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountChildId {
    /// The mount-root folder UUID (envelope; system-interpreted).
    pub mount_id: Uuid,
    /// The provider-owned node identity (opaque payload).
    pub node_id: NodeId,
}

impl MountChildId {
    /// Build a child id from a mount root and a provider node id.
    pub fn new(mount_id: Uuid, node_id: impl Into<NodeId>) -> Self {
        Self {
            mount_id,
            node_id: node_id.into(),
        }
    }
}

/// Renders as `ext:<mount_id>:<base64url(node_id)>`.
impl fmt::Display for MountChildId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{EXTERNAL_ID_PREFIX}{}:{}",
            self.mount_id.simple(),
            URL_SAFE_NO_PAD.encode(self.node_id.0.as_bytes())
        )
    }
}

/// Error returned when a string is not a well-formed external-mount child id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseMountChildIdError;

impl fmt::Display for ParseMountChildIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a valid external mount id (expected ext:<mount_id>:<token>)")
    }
}

impl std::error::Error for ParseMountChildIdError {}

impl FromStr for MountChildId {
    type Err = ParseMountChildIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // ext:<mount_id>:<token>  — split into exactly three logical parts.
        let rest = s
            .strip_prefix(EXTERNAL_ID_PREFIX)
            .ok_or(ParseMountChildIdError)?;
        let (mount_part, token) = rest.split_once(':').ok_or(ParseMountChildIdError)?;
        let mount_id = Uuid::parse_str(mount_part).map_err(|_| ParseMountChildIdError)?;
        let bytes = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| ParseMountChildIdError)?;
        let node = String::from_utf8(bytes).map_err(|_| ParseMountChildIdError)?;
        Ok(MountChildId {
            mount_id,
            node_id: NodeId(node),
        })
    }
}

/// Cheap check: does this id use the external-mount scheme?
///
/// Used at the top of service methods to route `ext:` ids away from
/// `Uuid::parse_str` and the PostgreSQL repositories.
pub fn is_external_id(id: &str) -> bool {
    id.starts_with(EXTERNAL_ID_PREFIX)
}

/// Encode a child id string from its parts.
pub fn encode_child_id(mount_id: Uuid, node_id: impl Into<NodeId>) -> String {
    MountChildId::new(mount_id, node_id).to_string()
}

/// Parse a child id string into its parts, or `None` when not `ext:`-shaped.
pub fn parse_child_id(id: &str) -> Option<MountChildId> {
    id.parse().ok()
}

/// ETag for a virtual file (no blob hash): `ext-{size:x}-{modified_at}`.
///
/// Combines size and mtime so it changes on any content edit without reading
/// the file. Mirrors the native `{blob_hash[..16]}-{modified_at}` shape closely
/// enough for conditional requests.
pub fn virtual_file_etag(size: u64, modified_at: u64) -> String {
    format!("ext-{size:x}-{modified_at}")
}

/// ETag for a virtual folder: `ext-{modified_at}` (directory mtime).
pub fn virtual_folder_etag(modified_at: u64) -> String {
    format!("ext-{modified_at}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_simple_path() {
        let mount = Uuid::new_v4();
        let id = encode_child_id(mount, "docs/report.txt");
        assert!(is_external_id(&id));
        let parsed = parse_child_id(&id).expect("parse");
        assert_eq!(parsed.mount_id, mount);
        assert_eq!(parsed.node_id.as_str(), "docs/report.txt");
    }

    #[test]
    fn round_trips_via_fromstr_display() {
        let mount = Uuid::new_v4();
        let child = MountChildId::new(mount, "a/b/c.bin");
        let rendered = child.to_string();
        let parsed: MountChildId = rendered.parse().expect("parse");
        assert_eq!(child, parsed);
    }

    #[test]
    fn token_survives_separators_and_unicode() {
        // node ids containing ':' '/' and non-ascii must survive the envelope.
        let mount = Uuid::new_v4();
        let tricky = "weird: name/with:colons/café.txt";
        let id = encode_child_id(mount, tricky);
        // The encoded form must not be ambiguous to the splitter: exactly two
        // colons (the `ext:` scheme and the `<mount_id>:` separator); the
        // base64url token never contains `:` or `/`.
        assert_eq!(id.matches(':').count(), 2, "only scheme + mount separators");
        let parsed = parse_child_id(&id).expect("parse");
        assert_eq!(parsed.node_id.as_str(), tricky);
    }

    #[test]
    fn rejects_non_external_ids() {
        assert!(parse_child_id("not-an-ext-id").is_none());
        assert!(parse_child_id(&Uuid::new_v4().to_string()).is_none());
        assert!(parse_child_id("ext:not-a-uuid:dG9rZW4").is_none());
        assert!(!is_external_id(&Uuid::new_v4().to_string()));
    }

    #[test]
    fn rejects_malformed_envelopes() {
        // Missing the second colon (no token separator).
        assert!(parse_child_id("ext:abc").is_none());
        // `ext:` with a valid uuid but a non-base64url token.
        let u = Uuid::new_v4().simple().to_string();
        assert!(parse_child_id(&format!("ext:{u}:!!!not-base64!!!")).is_none());
        // Empty string / bare scheme.
        assert!(parse_child_id("").is_none());
        assert!(parse_child_id("ext:").is_none());
        // is_external_id is a pure prefix check.
        assert!(is_external_id("ext:anything"));
        assert!(!is_external_id("EXT:upper"));
    }

    #[test]
    fn round_trips_empty_node_id() {
        // The mount root's own node id is empty; it must still round-trip
        // (e.g. if ever encoded), encoding to a token-less-but-present form.
        let mount = Uuid::new_v4();
        let id = encode_child_id(mount, "");
        let parsed = parse_child_id(&id).expect("parse empty node");
        assert_eq!(parsed.mount_id, mount);
        assert_eq!(parsed.node_id.as_str(), "");
    }

    #[test]
    fn round_trips_long_and_nested_paths() {
        let mount = Uuid::new_v4();
        let deep = "a/".repeat(64) + "leaf.bin";
        let id = encode_child_id(mount, deep.clone());
        assert_eq!(parse_child_id(&id).unwrap().node_id.as_str(), deep);
    }

    #[test]
    fn node_id_conversions() {
        assert_eq!(NodeId::from("x").as_str(), "x");
        assert_eq!(NodeId::from(String::from("y")).into_string(), "y");
        assert_eq!(NodeId::default().as_str(), "");
    }

    #[test]
    fn etags_change_with_inputs() {
        assert_ne!(virtual_file_etag(10, 100), virtual_file_etag(11, 100));
        assert_ne!(virtual_file_etag(10, 100), virtual_file_etag(10, 101));
        assert_ne!(virtual_folder_etag(100), virtual_folder_etag(101));
        // Format is stable and documented.
        assert_eq!(virtual_file_etag(255, 16), "ext-ff-16");
        assert_eq!(virtual_folder_etag(42), "ext-42");
    }
}
