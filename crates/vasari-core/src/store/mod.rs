use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::error::VasariError;
use crate::schema::{Attribution, AttributionTarget, Node, NodeId};

/// Git-like content-addressed object store at `<repo>/.vasari/`.
///
/// Layout:
///   objects/<sha[0..2]>/<sha[2..]>   canonical JSON nodes, gzip-compressed
///   index/targets/<encoded-path>/<start>-<end>  → attribution node IDs (newline-separated)
///   refs/                             human-readable refs
///   HEAD                              current intent context
pub struct ObjectStore {
    root: PathBuf,
}

impl ObjectStore {
    /// Open (or initialize) the store at `<repo_root>/.vasari/`.
    pub fn open(repo_root: &Path) -> Result<Self, VasariError> {
        let root = repo_root.join(".vasari");
        std::fs::create_dir_all(root.join("objects"))?;
        std::fs::create_dir_all(root.join("index").join("targets"))?;
        std::fs::create_dir_all(root.join("refs"))?;
        if !root.join("HEAD").exists() {
            std::fs::write(root.join("HEAD"), "")?;
        }
        Ok(Self { root })
    }

    /// Write a node to the object store. Idempotent: re-writing the same ID is a no-op.
    pub fn put(&self, node: &Node) -> Result<NodeId, VasariError> {
        let id = node.id();
        let (dir, file) = self.object_path(id);
        if file.exists() {
            return Ok(id.clone());
        }
        std::fs::create_dir_all(&dir)?;
        let json = serde_json::to_vec(node)?;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&json)?;
        let compressed = encoder.finish()?;
        std::fs::write(&file, compressed)?;

        // Update the derivable index for Attribution nodes.
        if let Node::Attribution(attr) = node {
            self.index_attribution(attr)?;
        }

        Ok(id.clone())
    }

    /// Read a node by ID. Returns None if not present.
    pub fn get(&self, id: &NodeId) -> Result<Option<Node>, VasariError> {
        let (_, file) = self.object_path(id);
        if !file.exists() {
            return Ok(None);
        }
        let compressed = std::fs::read(&file)?;
        let mut decoder = GzDecoder::new(&compressed[..]);
        let mut json = Vec::new();
        decoder.read_to_end(&mut json)?;
        let node: Node = serde_json::from_slice(&json)?;
        Ok(Some(node))
    }

    /// Look up attribution node IDs for a specific file:line target.
    /// The index is rebuilt by `vasari fsck` if stale.
    pub fn lookup_attributions(
        &self,
        path: &str,
        line: u32,
    ) -> Result<Vec<NodeId>, VasariError> {
        // Scan all index entries for this path and find ranges covering `line`.
        let path_dir = self.root.join("index").join("targets").join(encode_path(path));
        if !path_dir.exists() {
            return Ok(vec![]);
        }
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&path_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Index file names: "<start>-<end>"
            if let Some((start_s, end_s)) = name_str.split_once('-') {
                if let (Ok(start), Ok(end)) =
                    (start_s.parse::<u32>(), end_s.parse::<u32>())
                {
                    if line >= start && line <= end {
                        let content = std::fs::read_to_string(entry.path())?;
                        for id_str in content.lines() {
                            if !id_str.is_empty() {
                                ids.push(NodeId(id_str.to_string()));
                            }
                        }
                    }
                }
            }
        }
        Ok(ids)
    }

    /// Rebuild all indexes from the object store. Called by `vasari fsck`.
    pub fn rebuild_index(&self) -> Result<usize, VasariError> {
        let objects_dir = self.root.join("objects");
        let index_targets = self.root.join("index").join("targets");
        if index_targets.exists() {
            std::fs::remove_dir_all(&index_targets)?;
        }
        std::fs::create_dir_all(&index_targets)?;

        let mut count = 0;
        for prefix_entry in std::fs::read_dir(&objects_dir)? {
            let prefix_entry = prefix_entry?;
            for obj_entry in std::fs::read_dir(prefix_entry.path())? {
                let obj_entry = obj_entry?;
                let prefix = prefix_entry.file_name();
                let suffix = obj_entry.file_name();
                let id = NodeId(format!(
                    "{}{}",
                    prefix.to_string_lossy(),
                    suffix.to_string_lossy()
                ));
                if let Some(Node::Attribution(attr)) = self.get(&id)? {
                    self.index_attribution(&attr)?;
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    fn object_path(&self, id: &NodeId) -> (PathBuf, PathBuf) {
        let s = id.as_str();
        let dir = self.root.join("objects").join(&s[..2]);
        let file = dir.join(&s[2..]);
        (dir, file)
    }

    fn index_attribution(&self, attr: &Attribution) -> Result<(), VasariError> {
        if let AttributionTarget::LineRange { path, start, end } = &attr.target {
            let path_dir = self
                .root
                .join("index")
                .join("targets")
                .join(encode_path(path));
            std::fs::create_dir_all(&path_dir)?;
            let index_file = path_dir.join(format!("{start}-{end}"));
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&index_file)?;
            writeln!(f, "{}", attr.id.as_str())?;
        }
        Ok(())
    }
}

/// Encode a file path as a safe directory name for the index.
/// Slashes become `%2F`, percent signs become `%25`.
fn encode_path(path: &str) -> String {
    path.replace('%', "%25").replace('/', "%2F")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Attribution, AttributionTarget, Evidence, Intent, Node};

    #[test]
    fn round_trip_intent() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let intent = Intent::new("ACME-411".into(), "Add JWT verification".into(), vec![]);
        let id = store.put(&Node::Intent(intent.clone())).unwrap();
        let got = store.get(&id).unwrap().unwrap();
        match got {
            Node::Intent(i) => assert_eq!(i.id, intent.id),
            _ => panic!("wrong node type"),
        }
    }

    #[test]
    fn put_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let intent = Intent::new("src".into(), "test".into(), vec![]);
        let node = Node::Intent(intent);
        let id1 = store.put(&node).unwrap();
        let id2 = store.put(&node).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn attribution_index_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let action_id = NodeId("deadbeef".repeat(8));
        let attr = Attribution::new(
            action_id,
            AttributionTarget::LineRange {
                path: "src/auth.ts".into(),
                start: 40,
                end: 55,
            },
            1.0,
            vec![],
            vec![],
        );
        let attr_id = attr.id.clone();
        store.put(&Node::Attribution(attr)).unwrap();
        let found = store.lookup_attributions("src/auth.ts", 47).unwrap();
        assert!(found.contains(&attr_id));
        let not_found = store.lookup_attributions("src/auth.ts", 60).unwrap();
        assert!(not_found.is_empty());
    }
}
