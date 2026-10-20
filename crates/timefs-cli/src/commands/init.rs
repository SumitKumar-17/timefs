use std::path::Path;

use timefs_core::Repository;

use super::CmdResult;

pub fn run(path: &Path) -> CmdResult {
    Repository::init(path)?;
    println!("Initialized empty TimeFS repository in {}", path.display());
    Ok(())
}
