use std::path::Path;

use timefs_core::repo::StoreKind;
use timefs_core::Repository;

use super::CmdResult;
use crate::cli::{InitArgs, StoreArg};

impl From<StoreArg> for StoreKind {
    fn from(s: StoreArg) -> Self {
        match s {
            StoreArg::Pack => StoreKind::Pack,
            StoreArg::Loose => StoreKind::Loose,
        }
    }
}

pub fn run(path: &Path, args: InitArgs) -> CmdResult {
    let kind = StoreKind::from(args.store);
    Repository::init_with(path, kind)?;
    println!(
        "Initialized empty TimeFS repository ({} store) in {}",
        store_name(kind),
        path.display()
    );
    Ok(())
}

pub fn store_name(kind: StoreKind) -> &'static str {
    match kind {
        StoreKind::Pack => "pack",
        StoreKind::Loose => "loose",
    }
}
