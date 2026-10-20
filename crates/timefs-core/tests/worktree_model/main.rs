//! Model-based test: random POSIX operation sequences must behave identically on the
//! `WorkTree` and on a trivial in-memory reference filesystem, including across
//! checkpoint and reload.

mod harness;
mod model;

use harness::{Harness, Op};
use proptest::prelude::*;

fn name() -> impl Strategy<Value = String> {
    prop_oneof!["a", "b", "c"].prop_map(String::from)
}

fn path() -> impl Strategy<Value = Vec<String>> {
    proptest::collection::vec(name(), 1..=3)
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => path().prop_map(Op::Mkdir),
        4 => path().prop_map(Op::Create),
        1 => (path(), "[a-z/]{1,6}").prop_map(|(p, t)| Op::Symlink(p, t)),
        6 => (path(), 0usize..60, proptest::collection::vec(any::<u8>(), 0..30))
                .prop_map(|(p, o, d)| Op::Write(p, o, d)),
        2 => (path(), 0usize..80).prop_map(|(p, s)| Op::Truncate(p, s)),
        2 => (path(), 0u32..0o1000).prop_map(|(p, m)| Op::Chmod(p, m)),
        2 => path().prop_map(Op::Unlink),
        2 => path().prop_map(Op::Rmdir),
        3 => (path(), path()).prop_map(|(a, b)| Op::Rename(a, b)),
        2 => Just(Op::CheckpointReload),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, .. ProptestConfig::default() })]

    #[test]
    fn worktree_matches_reference_model(ops in proptest::collection::vec(op(), 1..60)) {
        let mut h = Harness::new();
        for o in &ops {
            h.apply(o).map_err(TestCaseError::fail)?;
        }
        h.assert_same_tree().map_err(TestCaseError::fail)?;
        // and once more after a final checkpoint and reload
        h.apply(&Op::CheckpointReload).map_err(TestCaseError::fail)?;
        h.assert_same_tree().map_err(TestCaseError::fail)?;
    }
}
