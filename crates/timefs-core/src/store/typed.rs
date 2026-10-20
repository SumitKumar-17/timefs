//! Typed convenience layer over the raw [`ObjectStore`] interface.

use crate::error::{Error, Result};
use crate::hash::Hash;
use crate::object::{Commit, FileObj, ObjectKind, Tree};
use crate::store::ObjectStore;

/// Typed put/get helpers, available on every [`ObjectStore`] (including `dyn`).
pub trait ObjectStoreExt: ObjectStore {
    fn put_blob(&self, data: &[u8]) -> Result<Hash> {
        self.put(ObjectKind::Blob, data)
    }

    fn put_file(&self, file: &FileObj) -> Result<Hash> {
        self.put(ObjectKind::File, &file.encode())
    }

    fn put_tree(&self, tree: &Tree) -> Result<Hash> {
        self.put(ObjectKind::Tree, &tree.encode())
    }

    fn put_commit(&self, commit: &Commit) -> Result<Hash> {
        self.put(ObjectKind::Commit, &commit.encode())
    }

    fn get_blob(&self, hash: &Hash) -> Result<Vec<u8>> {
        self.get_kind(hash, ObjectKind::Blob)
    }

    fn get_file(&self, hash: &Hash) -> Result<FileObj> {
        FileObj::decode(&self.get_kind(hash, ObjectKind::File)?)
    }

    fn get_tree(&self, hash: &Hash) -> Result<Tree> {
        Tree::decode(&self.get_kind(hash, ObjectKind::Tree)?)
    }

    fn get_commit(&self, hash: &Hash) -> Result<Commit> {
        Commit::decode(&self.get_kind(hash, ObjectKind::Commit)?)
    }

    /// Fetch an object and fail unless it has the expected kind.
    fn get_kind(&self, hash: &Hash, expected: ObjectKind) -> Result<Vec<u8>> {
        let (kind, payload) = self.get(hash)?;
        if kind != expected {
            return Err(Error::WrongKind {
                hash: *hash,
                expected: expected.name(),
                actual: kind.name(),
            });
        }
        Ok(payload)
    }
}

impl<T: ObjectStore + ?Sized> ObjectStoreExt for T {}
