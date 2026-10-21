//! Q9: what does the loose-object layout cost in inodes, disk space and verification time?

use timefs_core::chunk::Chunker;
use timefs_core::fsck::fsck;
use timefs_core::Repository;

use super::Res;
use crate::env::{disk_bytes, import_commit, inodes_used, Config};
use crate::gen::{build_tree, TreeSpec};
use crate::report::{human_bytes, ms, timed, Table};

pub fn run(cfg: &Config) -> Res {
    let mut table = Table::new(&[
        "files",
        "logical size",
        "objects",
        "stored bytes",
        "disk usage (du)",
        "inodes added",
        "import ms",
        "sync ms",
        "fsck ms",
    ]);
    for &n in &cfg.sizes {
        let work = cfg.scratch(&format!("store-{n}"))?;
        let src = work.join("src");
        let info = build_tree(
            &src,
            &TreeSpec {
                files: n,
                file_size: cfg.file_size,
                seed: 1,
            },
        )?;
        let repo = Repository::init_with(work.join("repo"), cfg.store)?;

        let inodes_before = inodes_used(&work);
        let (_, import) = timed(|| import_commit(&repo, &src, &Chunker::default(), "base"));
        let (_, sync) = timed(|| repo.store().sync().unwrap());
        let inodes_after = inodes_used(&work);
        let (report, check) = timed(|| fsck(&repo));
        assert!(report?.is_clean());

        let objects = repo.store().list()?;
        let stored: u64 = objects.iter().map(|o| o.size).sum();
        table.row(vec![
            n.to_string(),
            human_bytes(info.bytes),
            objects.len().to_string(),
            human_bytes(stored),
            disk_bytes(&work.join("repo")).map_or("n/a".into(), human_bytes),
            inodes_before
                .zip(inodes_after)
                .map_or("n/a".into(), |(a, b)| b.saturating_sub(a).to_string()),
            format!("{:.0}", ms(import)),
            format!("{:.0}", ms(sync)),
            format!("{:.0}", ms(check)),
        ]);
        std::fs::remove_dir_all(&work)?;
    }
    table.print("Loose object store overhead (Q9)");
    Ok(())
}
