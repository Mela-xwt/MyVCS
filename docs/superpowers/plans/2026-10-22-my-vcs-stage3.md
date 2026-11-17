# MyVCS Stage 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Deliver sustainable SSH team workflows, recoverable reference updates and clearly scoped compatibility.

**Architecture:** Keep logical object IDs and pack v1/v2 unchanged. Introduce versioned, durable reference transaction metadata and recovery before operations; build team commands and SSH policy around the same locked publication path. Line merge operates on bounded text and keeps explicit unresolved state and an abort snapshot.

**Tech Stack:** Rust, existing serde/similar/flate2/fs2 dependencies, OpenSSH; Linux WSL and openEuler validation.

## Global Constraints

- User selected SSH only for this stage, no HTTPS.
- Authorization is operator-configured SSH identity, never commit author or a client-provided protocol identity.
- Keep previous logical object bytes and IDs; old formats remain readable; unsupported upgrades must fail clearly.
- Full pack fallback and stage 2 resource limits remain available.
- Workspace has no Git metadata. Preserve stage 2 baseline, per-task diff snapshots and final source archive instead of Git commits/worktrees.
- Tests must cover concurrent CAS, interruption recovery, GC retention, dirty worktree protection and explicit limits. No production key replacement or existing repository overwrite.

## Task 1: Line merge and abort

Files: `src/repo/branch_merge.rs`, new `src/repo/line_merge.rs`, `src/repo/core.rs`, `src/repo/mod.rs`, `src/cli.rs`, `src/main.rs`, `tests/team_merge.rs`.
Interfaces: preserve `merge_fast_forward_only(&self, target: &str) -> Result<()>`; add `merge_abort(&self) -> Result<()>`, invoked under repository lock.

- [x] Add behavior tests for disjoint lines, overlapping lines, CRLF/no-final-newline, binary modify/delete, dirty staging/working/untracked collisions, abort restoring original HEAD/index/files, double-parent resolution, multiple merge bases.
- [x] Run `cargo test --test team_merge` and retain the initial failure evidence.
- [x] Use base-relative line hunks to combine non-overlapping edits; identical edits deduplicate; overlaps produce conflict markers and structured path state. Bound text size/line complexity; binary data preserves ours and records conflict. Check clean worktree before any mutation. Persist abort state before restore; abort discards merge-produced edits only, retains unrelated untracked files, validates unchanged original HEAD and uses saved touched paths. Repeated abort after interruption must be safe.
- [x] Compute best common ancestors, refuse ambiguous multiple-base merge before mutation with explicit diagnostic (recursive virtual-base merge is out of scope).
- [x] Run new tests and relevant original merge/pull regressions, produce report/diff and obtain task review.

## Task 2: Reference transactions, reflog, recovery and compatibility

Files: new `src/repo/ref_state.rs`, core/safety/receive/gc_verify/branch_merge/history, new `tests/team_recovery.rs`.
Interfaces: public serializable `RefChange { name: String, expected: Option<String>, new: Option<String> }`; `Repo::transact_refs(&self, changes: &[RefChange], reason: &str) -> Result<()>`; locked variant for callers already holding lock; `ReceiveTransaction::commit_many` for object publication then one reference transaction.

- [x] Add tests for all-or-none multiple refs, stale one cancels all, deletion, concurrent writers, kill points before/after durable commit, old/new format handling, reset/delete/reflog GC/recover.
- [x] Implement a recoverable transaction journal, durable decision and locked roll-forward; reads/open/maintenance must not observe unrecovered partial state. Store reflog old/new records durably in the same decision. Use immutable transaction records or single atomic state publication to avoid ref/log skew. Reject wrong schema, duplicate/aliased names and missing closure before publication.
- [x] Preserve legacy ref layout through explicit upgrade with format guard old clients reject; never silently introduce unsafe legacy writer coexistence. Supply `upgrade` and `recover` entrypoints and documented retry semantics.
- [x] Reflog keeps old/new closures alive until explicit retention expiration; bounded metadata, explicit expiry command, recovery creates a new branch rather than destroying current state. Route branch creation/deletion/reset/receive through common update path; protect detached HEAD movements too.
- [x] Run targeted fault/concurrency/GC tests, review and fix before integration.

## Task 3: Team configuration, history and remote policies

Files: new `src/repo/daily.rs`, `src/repo/remote/team.rs`, `src/repo/policy.rs`, cli/main, history/core/remote wire/pack, new `tests/team_remote.rs`.

- [x] Add repository identity configuration (name/email), upstream set/unset/default routing, lightweight commit tags create/list/delete/resolve, topological DAG log with parent edges and decorations. Preserve public stage 1 methods as wrappers where tests rely on signatures.
- [x] Add explicit `fetch --prune` removing only absent refs from the chosen remote under CAS; protect other remotes/local refs/tags. Clone sets upstream. Tag fetch/push explicit, no silent tag overwrite.
- [x] Extend negotiated protocol for generic atomic ref updates; snapshots cover union of target histories. Explicit lease OID is mandatory for non-fast-forward updates, server policy must allow it, every update rechecks CAS under one lock. Support ref deletion with protected HEAD branch checks and audit.
- [x] Operator-managed repository policy maps trusted SSH principal to read and branch/tag write/force/delete patterns, default deny when policy present. Apply on advertisement/read and on final publication (not just handshake). Audit successful/denied operations with actor/ref/old/new/outcome, never passwords; fail closed if required audit cannot be recorded.
- [x] Test forged commit author, read-only/branch policy, force lease stale/current, multi-ref reject-all and concurrent pushes, prune/tag/upstream, DAG, identity and legacy protocol matrix.
- [x] Final integration review, WSL/openEuler full tests and real SSH identities in isolated directories; document limitations, commands, measured evidence and delivery hashes.
