//! Black-box tests of the `timefs` binary for everything that works without a mount.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Cli {
    dir: tempfile::TempDir,
}

impl Cli {
    fn new() -> Self {
        let cli = Cli {
            dir: tempfile::tempdir().unwrap(),
        };
        assert!(cli.run(&["init"]).status.success());
        cli
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_timefs"))
            .env("TIMEFS_REPO", self.repo())
            .args(args)
            .output()
            .unwrap()
    }

    /// Run and require success; returns stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Run and require failure; returns stderr.
    fn fails(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
        String::from_utf8(out.stderr).unwrap()
    }

    /// Replace the contents of a scratch directory and import it as a commit.
    fn commit(&self, files: &[(&str, &str)], message: &str) -> String {
        let src = self.dir.path().join("src");
        let _ = fs::remove_dir_all(&src);
        for (name, content) in files {
            let p = src.join(name);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
        fs::create_dir_all(&src).unwrap();
        self.ok(&["import", src.to_str().unwrap(), "-m", message])
    }
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap()
}

#[test]
fn import_log_diff_cat_ls_and_export() {
    let cli = Cli::new();
    cli.commit(&[("a.txt", "one"), ("dir/b.txt", "bee")], "first");
    cli.commit(
        &[
            ("a.txt", "one edited"),
            ("dir/b.txt", "bee"),
            ("c.txt", "see"),
        ],
        "second",
    );

    let log = cli.ok(&["log"]);
    assert!(log.contains("second") && log.contains("first"));

    let diff = cli.ok(&["diff", "HEAD~1", "HEAD"]);
    assert!(
        diff.contains("M a.txt") && diff.contains("A c.txt"),
        "{diff}"
    );
    assert!(!diff.contains("b.txt"));

    assert_eq!(cli.ok(&["cat", "HEAD~1", "a.txt"]), "one");
    assert_eq!(cli.ok(&["cat", "HEAD", "a.txt"]), "one edited");
    assert!(cli.ok(&["ls", "HEAD"]).contains("dir/"));

    let out = cli.dir.path().join("out");
    cli.ok(&["export", "HEAD~1", out.to_str().unwrap()]);
    assert_eq!(read(&out.join("a.txt")), "one");
    assert!(!out.join("c.txt").exists());

    assert!(cli.ok(&["fsck"]).contains("healthy"));
    assert!(cli.ok(&["stats"]).contains("objects:"));
}

#[test]
fn branch_checkout_and_clean_merge() {
    let cli = Cli::new();
    cli.commit(&[("base.txt", "b"), ("shared.txt", "s")], "base");
    cli.ok(&["branch", "feature"]);
    cli.ok(&["checkout", "feature"]);
    cli.commit(
        &[("base.txt", "b"), ("shared.txt", "s"), ("feature.txt", "f")],
        "feature work",
    );
    cli.ok(&["checkout", "main"]);
    cli.commit(
        &[("base.txt", "b"), ("shared.txt", "s"), ("main.txt", "m")],
        "main work",
    );

    let merged = cli.ok(&["merge", "feature"]);
    assert!(merged.contains("Merged into main"), "{merged}");
    let ls = cli.ok(&["ls", "HEAD"]);
    assert!(
        ls.contains("feature.txt") && ls.contains("main.txt") && ls.contains("base.txt"),
        "{ls}"
    );
    assert!(cli.ok(&["log"]).contains("(merge)"));
    assert!(cli.ok(&["merge", "feature"]).contains("Already up to date"));
}

#[test]
fn conflicting_merge_aborts_untouched_and_can_be_resolved() {
    let cli = Cli::new();
    cli.commit(&[("f.txt", "base")], "base");
    cli.ok(&["branch", "other"]);
    cli.ok(&["checkout", "other"]);
    cli.commit(&[("f.txt", "theirs")], "theirs");
    cli.ok(&["checkout", "main"]);
    cli.commit(&[("f.txt", "ours")], "ours");
    let tip_before = cli.ok(&["log", "-n", "1"]);

    let err = cli.fails(&["merge", "other"]);
    assert!(err.contains("conflict"), "{err}");
    assert_eq!(
        cli.ok(&["log", "-n", "1"]),
        tip_before,
        "an aborted merge must change nothing"
    );
    assert_eq!(cli.ok(&["cat", "HEAD", "f.txt"]), "ours");

    cli.ok(&["merge", "other", "--resolve", "theirs"]);
    assert_eq!(cli.ok(&["cat", "HEAD", "f.txt"]), "theirs");
}

#[test]
fn reset_rolls_back_and_gc_reclaims_the_abandoned_history() {
    let cli = Cli::new();
    cli.commit(&[("small", "keep")], "keep");
    let big = "x".repeat(300_000);
    cli.commit(&[("small", "keep"), ("big", big.as_str())], "big");
    cli.ok(&["reset", "HEAD~1"]);
    assert!(cli.ok(&["log"]).contains("keep") && !cli.ok(&["log"]).contains("big"));

    let dry = cli.ok(&["gc", "--grace-secs", "0", "--dry-run"]);
    assert!(dry.contains("Would delete"), "{dry}");
    let real = cli.ok(&["gc", "--grace-secs", "0"]);
    assert!(real.contains("Deleted"), "{real}");
    assert!(cli.ok(&["fsck"]).contains("healthy"));
    assert!(cli.ok(&["gc", "--grace-secs", "0"]).contains("Deleted 0"));
}

#[test]
fn user_errors_exit_nonzero_with_a_message() {
    let cli = Cli::new();
    assert!(
        cli.fails(&["log"]).contains("error"),
        "HEAD has no commits yet"
    );
    cli.commit(&[("f", "1")], "one");
    assert!(cli.fails(&["cat", "HEAD", "missing"]).contains("not found"));
    assert!(cli.fails(&["checkout", "nope"]).contains("does not exist"));
    assert!(cli.fails(&["branch", "main"]).contains("exists"));
    assert!(cli
        .fails(&["import", "/definitely/not/a/dir"])
        .contains("error"));
    assert!(cli
        .fails(&["--repo", "/tmp/not-a-repo-xyz", "log"])
        .contains("not a TimeFS repository"));
    assert!(cli.fails(&["status"]).contains("no running mount"));
}

#[test]
fn snap_without_a_mount_reports_nothing_to_snapshot() {
    let cli = Cli::new();
    cli.commit(&[("f", "1")], "one");
    assert!(cli.ok(&["snap"]).contains("nothing to snapshot"));
}

#[test]
fn import_refuses_while_a_checkpoint_is_pending() {
    let cli = Cli::new();
    cli.commit(&[("f", "1")], "one");
    let repo = timefs_core::Repository::open(cli.repo()).unwrap();
    let tip = repo.branch_tip("main").unwrap().unwrap();
    repo.refs()
        .write(&timefs_core::repo::wip_ref("main"), &tip)
        .unwrap();
    let log_before = cli.ok(&["log"]);

    let src = cli.dir.path().join("other");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("g"), "2").unwrap();
    let err = cli.fails(&["import", src.to_str().unwrap(), "-m", "two"]);
    assert!(err.contains("unsnapshotted"), "{err}");
    assert_eq!(
        cli.ok(&["log"]),
        log_before,
        "a refused import changes nothing"
    );
    assert!(repo.has_checkpoint("main").unwrap());
}

#[test]
fn import_and_branch_delete_refuse_while_the_repository_is_locked() {
    let cli = Cli::new();
    cli.commit(&[("f", "1")], "one");
    cli.ok(&["branch", "feature"]);
    let repo = timefs_core::Repository::open(cli.repo()).unwrap();
    let lock = repo.lock_exclusive().unwrap();

    let src = cli.dir.path().join("other");
    fs::create_dir_all(&src).unwrap();
    let err = cli.fails(&["import", src.to_str().unwrap(), "-m", "two"]);
    assert!(err.contains("mounted"), "{err}");
    let err = cli.fails(&["branch", "-d", "feature"]);
    assert!(err.contains("mounted"), "{err}");
    assert!(
        cli.ok(&["branch", "created-while-locked"])
            .contains("Created"),
        "branch creation moves no existing ref and stays unlocked"
    );

    drop(lock);
    assert!(cli.ok(&["branch", "-d", "feature"]).contains("Deleted"));
    cli.ok(&["import", src.to_str().unwrap(), "-m", "two"]);
}

#[test]
fn migrate_copies_a_repository_into_the_other_layout() {
    let cli = Cli::new();
    cli.commit(&[("f.txt", "one")], "first");
    cli.commit(&[("f.txt", "two")], "second");
    let dest = cli.dir.path().join("migrated");
    let out = cli.ok(&["migrate", dest.to_str().unwrap(), "--store", "loose"]);
    assert!(out.contains("Verified with fsck"), "{out}");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_timefs"))
            .env("TIMEFS_REPO", &dest)
            .args(args)
            .output()
            .unwrap()
    };
    assert_eq!(
        String::from_utf8(run(&["cat", "HEAD~1", "f.txt"]).stdout).unwrap(),
        "one"
    );
    assert!(String::from_utf8(run(&["stats"]).stdout)
        .unwrap()
        .contains("store:    loose"));
}
