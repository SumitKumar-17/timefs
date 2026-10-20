# ADR-0006: Tree-level 3-way merge, explicit conflicts

**Status:** Accepted (implemented)

## Context
* Branching needs a way to combine divergent branches.
* Silent data loss during merge is unacceptable.

## Decision
* 3-way merge over trees, using a lowest common ancestor in the commit DAG as the base.
* Identical hashes short-circuit.
* Per path:
  * one-sided change: apply it,
  * both sides changed a directory: recurse,
  * anything else: conflict (add/add, modify/modify, modify/delete, file vs dir).
* Default: a conflicting merge aborts, leaves the repo untouched and lists all conflicts.
* `--resolve ours|theirs` picks a side for every conflict.
* "Equal" means same kind, content hash, mode, uid and gid; mtime is ignored (the newer wins).
* Needs the repo unmounted.

## Alternatives
* **Conflict markers or `.ours`/`.theirs` files in the tree:** commits a half-resolved state into history.
* **Line-level text merge (diff3):** needs encoding and binary detection; would slot in at the "both modified a regular file" case.
* **Fast-forward only:** too weak for the branching research question.

## Consequences
* Merge cost follows changed paths.
* Resolution is whole-file, per side.
* Criss-cross merges (several merge bases) pick one deterministically; documented limitation.
