//! A deliberately trivial in-memory filesystem used as the oracle for the model test.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MNode {
    File(Vec<u8>),
    Dir(BTreeMap<String, MNode>),
    Link(String),
}

/// Error kinds shared with the real implementation's `FsError` (payloads dropped).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    NotFound,
    Exists,
    NotDir,
    IsDir,
    NotEmpty,
    Invalid,
}

pub type MResult<T> = Result<T, Kind>;

pub struct Model {
    pub root: BTreeMap<String, MNode>,
}

impl Model {
    pub fn new() -> Self {
        Model {
            root: BTreeMap::new(),
        }
    }

    /// Walk to the directory at `path`; every component must be an existing directory.
    fn dir_mut(&mut self, path: &[String]) -> MResult<&mut BTreeMap<String, MNode>> {
        let mut cur = &mut self.root;
        for part in path {
            match cur.get_mut(part) {
                None => return Err(Kind::NotFound),
                Some(MNode::Dir(d)) => cur = d,
                Some(_) => return Err(Kind::NotDir),
            }
        }
        Ok(cur)
    }

    pub fn mkdir(&mut self, parent: &[String], name: &str) -> MResult<()> {
        self.insert_new(parent, name, MNode::Dir(BTreeMap::new()))
    }

    pub fn create(&mut self, parent: &[String], name: &str) -> MResult<()> {
        self.insert_new(parent, name, MNode::File(Vec::new()))
    }

    pub fn symlink(&mut self, parent: &[String], name: &str, target: &str) -> MResult<()> {
        self.insert_new(parent, name, MNode::Link(target.to_string()))
    }

    fn insert_new(&mut self, parent: &[String], name: &str, node: MNode) -> MResult<()> {
        let dir = self.dir_mut(parent)?;
        if dir.contains_key(name) {
            return Err(Kind::Exists);
        }
        dir.insert(name.to_string(), node);
        Ok(())
    }

    pub fn write(&mut self, parent: &[String], name: &str, off: usize, data: &[u8]) -> MResult<()> {
        let data_vec = self.file_mut(parent, name)?;
        if data.is_empty() {
            return Ok(());
        }
        if data_vec.len() < off + data.len() {
            data_vec.resize(off + data.len(), 0);
        }
        data_vec[off..off + data.len()].copy_from_slice(data);
        Ok(())
    }

    pub fn truncate(&mut self, parent: &[String], name: &str, size: usize) -> MResult<()> {
        self.file_mut(parent, name)?.resize(size, 0);
        Ok(())
    }

    fn file_mut(&mut self, parent: &[String], name: &str) -> MResult<&mut Vec<u8>> {
        match self.dir_mut(parent)?.get_mut(name) {
            None => Err(Kind::NotFound),
            Some(MNode::File(d)) => Ok(d),
            Some(MNode::Dir(_)) => Err(Kind::IsDir),
            Some(MNode::Link(_)) => Err(Kind::Invalid),
        }
    }

    /// Mode is not modelled; only whether the entry exists matters.
    pub fn chmod(&mut self, parent: &[String], name: &str) -> MResult<()> {
        match self.dir_mut(parent)?.get(name) {
            Some(_) => Ok(()),
            None => Err(Kind::NotFound),
        }
    }

    pub fn unlink(&mut self, parent: &[String], name: &str) -> MResult<()> {
        let dir = self.dir_mut(parent)?;
        match dir.get(name) {
            None => Err(Kind::NotFound),
            Some(MNode::Dir(_)) => Err(Kind::IsDir),
            Some(_) => {
                dir.remove(name);
                Ok(())
            }
        }
    }

    pub fn rmdir(&mut self, parent: &[String], name: &str) -> MResult<()> {
        let dir = self.dir_mut(parent)?;
        match dir.get(name) {
            None => Err(Kind::NotFound),
            Some(MNode::Dir(d)) if !d.is_empty() => Err(Kind::NotEmpty),
            Some(MNode::Dir(_)) => {
                dir.remove(name);
                Ok(())
            }
            Some(_) => Err(Kind::NotDir),
        }
    }

    pub fn rename(
        &mut self,
        parent: &[String],
        name: &str,
        new_parent: &[String],
        new_name: &str,
    ) -> MResult<()> {
        // Same order of checks as the real implementation.
        self.dir_mut(parent)?;
        self.dir_mut(new_parent)?;
        let src = self
            .dir_mut(parent)?
            .get(name)
            .cloned()
            .ok_or(Kind::NotFound)?;
        let same_entry = parent == new_parent && name == new_name;
        if same_entry {
            return Ok(());
        }
        let src_is_dir = matches!(src, MNode::Dir(_));
        if src_is_dir && is_prefix(&[parent, &[name.to_string()]].concat(), new_parent) {
            return Err(Kind::Invalid);
        }
        if let Some(dst) = self.dir_mut(new_parent)?.get(new_name) {
            match (src_is_dir, dst) {
                (true, MNode::Dir(d)) if !d.is_empty() => return Err(Kind::NotEmpty),
                (true, MNode::Dir(_)) => {}
                (true, _) => return Err(Kind::NotDir),
                (false, MNode::Dir(_)) => return Err(Kind::IsDir),
                (false, _) => {}
            }
        }
        self.dir_mut(parent)?.remove(name);
        self.dir_mut(new_parent)?.insert(new_name.to_string(), src);
        Ok(())
    }
}

fn is_prefix(prefix: &[String], path: &[String]) -> bool {
    path.len() >= prefix.len() && path[..prefix.len()] == *prefix
}
