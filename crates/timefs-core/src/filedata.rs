//! Storing and loading a file's extent list, paged when it is long.
//!
//! A file with at most [`PAGE_EXTENTS`] extents is one `File` object, exactly as before.
//! A longer one is split into `File` pages of that many extents plus a `FileIndex` naming
//! them. Unchanged pages keep their hashes, so a checkpoint after a small edit to a huge file
//! writes one page and the index instead of the whole list (ADR-0009, part 2).

use crate::error::{Error, Result};
use crate::hash::Hash;
use crate::object::{FileIndex, FileObj, ObjectKind, Page, PAGE_EXTENTS};
use crate::store::ObjectStore;

/// Store `file`, paging it if needed, and return the hash a tree entry should name.
pub fn store_file<S: ObjectStore + ?Sized>(store: &S, file: &FileObj) -> Result<Hash> {
    if file.extents.len() <= PAGE_EXTENTS {
        return store.put(ObjectKind::File, &file.encode());
    }
    let mut pages = Vec::with_capacity(file.extents.len().div_ceil(PAGE_EXTENTS));
    for chunk in file.extents.chunks(PAGE_EXTENTS) {
        let page = FileObj {
            extents: chunk.to_vec(),
        };
        pages.push(Page {
            hash: store.put(ObjectKind::File, &page.encode())?,
            size: page.size(),
            extents: chunk.len() as u64,
        });
    }
    store.put(ObjectKind::FileIndex, &FileIndex { pages }.encode())
}

/// Load the full extent list named by a tree entry (a `File` or a `FileIndex`).
pub fn load_file<S: ObjectStore + ?Sized>(store: &S, hash: &Hash) -> Result<FileObj> {
    let (kind, payload) = store.get(hash)?;
    match kind {
        ObjectKind::File => FileObj::decode(&payload),
        ObjectKind::FileIndex => {
            let index = FileIndex::decode(&payload)?;
            let mut extents = Vec::new();
            for page in &index.pages {
                let (page_kind, bytes) = store.get(&page.hash)?;
                if page_kind != ObjectKind::File {
                    return Err(wrong_kind(&page.hash, page_kind));
                }
                let obj = FileObj::decode(&bytes)?;
                if obj.extents.len() as u64 != page.extents || obj.size() != page.size {
                    return Err(Error::corrupt(format!(
                        "page {} does not match its index entry",
                        page.hash
                    )));
                }
                extents.extend(obj.extents);
            }
            Ok(FileObj { extents })
        }
        other => Err(wrong_kind(hash, other)),
    }
}

/// What a file object references: `(hash, kind)` of each page or data blob. Holes reference
/// nothing. Used by reachability (fsck and gc) without materialising whole extent lists.
pub fn file_children<S: ObjectStore + ?Sized>(
    store: &S,
    hash: &Hash,
) -> Result<Vec<(Hash, ObjectKind)>> {
    let (kind, payload) = store.get(hash)?;
    match kind {
        ObjectKind::File => Ok(FileObj::decode(&payload)?
            .extents
            .iter()
            .filter(|e| !e.is_hole())
            .map(|e| (e.hash, ObjectKind::Blob))
            .collect()),
        ObjectKind::FileIndex => Ok(FileIndex::decode(&payload)?
            .pages
            .iter()
            .map(|p| (p.hash, ObjectKind::File))
            .collect()),
        other => Err(wrong_kind(hash, other)),
    }
}

fn wrong_kind(hash: &Hash, actual: ObjectKind) -> Error {
    Error::WrongKind {
        hash: *hash,
        expected: "file",
        actual: actual.name(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::Extent;
    use crate::store::{LooseStore, ObjectStoreExt};

    fn store() -> (tempfile::TempDir, LooseStore) {
        let d = tempfile::tempdir().unwrap();
        let s = LooseStore::open(d.path().join("objects")).unwrap();
        (d, s)
    }

    fn file_with(store: &LooseStore, extents: usize) -> FileObj {
        FileObj {
            extents: (0..extents)
                .map(|i| Extent {
                    hash: store.put_blob(&(i as u32).to_le_bytes()).unwrap(),
                    len: 4,
                })
                .collect(),
        }
    }

    #[test]
    fn short_files_stay_one_flat_object() {
        let (_d, s) = store();
        let f = file_with(&s, 10);
        let h = store_file(&s, &f).unwrap();
        assert_eq!(s.get(&h).unwrap().0, ObjectKind::File);
        assert_eq!(load_file(&s, &h).unwrap(), f);
    }

    #[test]
    fn long_files_are_paged_and_round_trip() {
        let (_d, s) = store();
        let f = file_with(&s, PAGE_EXTENTS * 3 + 7);
        let h = store_file(&s, &f).unwrap();
        assert_eq!(s.get(&h).unwrap().0, ObjectKind::FileIndex);
        assert_eq!(load_file(&s, &h).unwrap(), f);
        let children = file_children(&s, &h).unwrap();
        assert_eq!(children.len(), 4);
        assert!(children.iter().all(|(_, k)| *k == ObjectKind::File));
    }

    #[test]
    fn editing_one_extent_rewrites_one_page_and_the_index() {
        let (_d, s) = store();
        let mut f = file_with(&s, PAGE_EXTENTS * 8);
        store_file(&s, &f).unwrap();
        f.extents[5000].hash = s.put_blob(b"edit").unwrap();
        let before = s.stats().new_objects;
        store_file(&s, &f).unwrap();
        assert_eq!(s.stats().new_objects - before, 2, "one page plus the index");
    }

    #[test]
    fn holes_reference_no_blob() {
        let (_d, s) = store();
        let f = FileObj {
            extents: vec![
                Extent::hole(1 << 40),
                Extent {
                    hash: s.put_blob(b"x").unwrap(),
                    len: 1,
                },
            ],
        };
        let h = store_file(&s, &f).unwrap();
        assert_eq!(file_children(&s, &h).unwrap().len(), 1);
        assert_eq!(load_file(&s, &h).unwrap().size(), (1 << 40) + 1);
    }

    #[test]
    fn a_page_that_disagrees_with_its_index_is_corrupt() {
        let (_d, s) = store();
        let page = s
            .put_file(&FileObj {
                extents: vec![Extent::hole(10)],
            })
            .unwrap();
        let bad = FileIndex {
            pages: vec![Page {
                hash: page,
                size: 11,
                extents: 1,
            }],
        };
        let h = s.put(ObjectKind::FileIndex, &bad.encode()).unwrap();
        assert!(load_file(&s, &h).is_err());
    }
}
