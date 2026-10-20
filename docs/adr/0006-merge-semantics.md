# ADR-0006: Tree-level 3-way merge, explicit conflicts

**Status:** Accepted

## Context
Branching is only useful if divergent branches can be combined, and silent data loss during
merge is unacceptable.

## Decision
Merge is a 3-way merge over trees using the lowest common ancestor in the commit DAG as the
base. Identical hashes short-circuit. Per path: one-sided changes apply; both-sided
changes to *directories* recurse; both-sided changes to anything else are **conflicts**
(add/add, modify/modify, modify/delete, file↔dir). By default a conflicting merge **aborts
and leaves the repository untouched**, printing all conflicts. `--resolve ours|theirs`
picks a side for every conflict. "Equal" means same kind, same content hash, same
mode/uid/gid; mtime is ignored (the newer wins) so two sides that merely `touch`ed a file
do not conflict.

## Alternatives
* **Write conflict markers / `.ours` `.theirs` files into the tree.** Convenient but commits
  a half-resolved state into history.
* **Line-level text merge (diff3).** Useful for source trees but needs encoding/binary
  detection; deferred to a later milestone, designed to slot in at the "both modified a
  regular file" case.
* **Fast-forward only.** Too weak to demonstrate the branching research question.

## Consequences
* Merge cost ∝ changed paths.
* Users must resolve conflicts explicitly; v1 resolution is whole-file, per side.
* Criss-cross merges (multiple merge bases) pick one deterministically (lowest hash among LCAs); documented limitation.
