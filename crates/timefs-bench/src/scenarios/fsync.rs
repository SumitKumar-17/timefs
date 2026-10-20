//! Where does an fsync go? Times each phase of a checkpoint for the "append 4 KiB, fsync" pattern.

use std::time::Duration;

use timefs_core::repo::wip_ref;
use timefs_core::worktree::{Options, WorkTree, ROOT_INO};

use super::Res;
use crate::env::{new_repo, Config};
use crate::gen::Rng;
use crate::report::{ms, timed, Summary, Table};

const ROUNDS: usize = 40;

pub fn run(cfg: &Config) -> Res {
    let repo = new_repo(cfg, "fsync")?;
    let store = std::sync::Arc::clone(repo.store());
    let mut wt = WorkTree::new(store, None, Options::default());
    let ino = wt.create(ROOT_INO, b"journal", 0o644, 0, 0)?.ino;
    let mut rng = Rng::new(9);

    let mut phases: [Vec<f64>; 5] = Default::default();
    let mut new_objects = Vec::new();
    for round in 0..ROUNDS {
        wt.write(ino, (round * 4096) as u64, &rng.bytes(4096))?;
        let before = repo.store().stats().new_objects;

        let (tree, persist) = timed(|| wt.checkpoint().unwrap());
        let (commit, make_commit) =
            timed(|| repo.create_commit(tree, vec![], "b", "checkpoint").unwrap());
        let (_, sync) = timed(|| repo.store().sync().unwrap());
        let (_, write_ref) = timed(|| repo.refs().write(&wip_ref("main"), &commit).unwrap());
        let total: Duration = persist + make_commit + sync + write_ref;

        for (slot, d) in phases
            .iter_mut()
            .zip([persist, make_commit, sync, write_ref, total])
        {
            slot.push(ms(d));
        }
        new_objects.push((repo.store().stats().new_objects - before) as f64);
    }

    let mut table = Table::new(&["phase", "ms median (min..max)"]);
    let names = [
        "persist objects (no sync)",
        "create commit object",
        "sync objects (fsync batch)",
        "write ref (tmp, fsync, rename, fsync dir)",
        "total",
    ];
    for (name, samples) in names.iter().zip(&phases) {
        table.row(vec![name.to_string(), Summary::of(samples).show(2)]);
    }
    table.row(vec![
        "new objects per checkpoint".into(),
        format!("{:.0}", Summary::of(&new_objects).median),
    ]);
    table.print("Checkpoint phase breakdown for append + fsync");
    Ok(())
}
