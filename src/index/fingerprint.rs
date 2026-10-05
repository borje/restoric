//! Content fingerprints of files, links and other non-folder entries
//! (PLAN.md §2.2). Modification time and the rest of restic's metadata
//! churn (atime, ctime, inode, device id) are left out; mode, owner and
//! group are in.
//!
//! Folders have no stored fingerprint: whether two folder trees differ in
//! content is found by diffing them (`folder.rs`), going only into subtrees
//! whose ids differ, so the cost follows what changed rather than the size
//! of the folder.

use sha2::{Digest, Sha256};

use crate::repo::{Node, NodeKind};

pub type Fp = [u8; 32];

pub fn leaf(node: &Node) -> Fp {
    let mut h = Sha256::new();
    match &node.kind {
        NodeKind::Symlink { target } => {
            h.update(b"link\0");
            h.update(target.as_encoded_bytes());
        }
        kind => {
            match kind {
                NodeKind::Other(t) => h.update(t.as_bytes()),
                NodeKind::Dir => h.update(b"dir"),
                _ => h.update(b"file"),
            }
            h.update([0]);
            h.update(node.size.to_le_bytes());
            h.update(node.mode.unwrap_or(0).to_le_bytes());
            h.update(node.uid.unwrap_or(0).to_le_bytes());
            h.update(node.gid.unwrap_or(0).to_le_bytes());
            for c in &node.content {
                h.update(c.0.0);
            }
        }
    }
    h.finalize().into()
}

/// The file's bytes, as a key: equal keys, equal content.
pub fn content(node: &Node) -> Fp {
    let mut h = Sha256::new();
    h.update(node.size.to_le_bytes());
    for c in &node.content {
        h.update(c.0.0);
    }
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{BlobId, Id};

    fn file() -> Node {
        Node {
            name: "a".into(),
            kind: NodeKind::File,
            size: 3,
            mode: Some(0o644),
            uid: Some(1000),
            gid: Some(1000),
            mtime: None,
            content: vec![BlobId(Id([7; 32]))],
            subtree: None,
            raw: Id([1; 32]),
        }
    }

    #[test]
    fn ignores_metadata_churn() {
        let mut b = file();
        b.mtime = Some(jiff::Timestamp::now());
        b.raw = Id([2; 32]);
        assert_eq!(leaf(&file()), leaf(&b));
    }

    #[test]
    fn catches_content_mode_and_owner() {
        let base = leaf(&file());
        let mut b = file();
        b.content = vec![BlobId(Id([8; 32]))];
        assert_ne!(base, leaf(&b));
        let mut b = file();
        b.mode = Some(0o600);
        assert_ne!(base, leaf(&b));
        let mut b = file();
        b.gid = Some(0);
        assert_ne!(base, leaf(&b));
    }
}
