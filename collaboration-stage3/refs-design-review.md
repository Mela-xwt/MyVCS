# Stage 3 ref-state architecture review

Read-only source review; no code changes or tests were run. The proposed single authoritative `refs-state.json` is a sound commit point for refs, HEAD, retained reflog, and successful mutation audit events. Keep format 1 behavior and require explicit upgrade for format 2 features.

## Findings that affect correctness

1. **Upgrade cannot make already-open old binaries safe.** `Repo::open` checks `format.json` once (`core.rs:44-58`); the existing `Repo` handle has no later version guard. A stage 2 process that opened before upgrade can acquire the lock afterward, publish a loose ref the new reader ignores, or GC objects reachable only from format 2 state. New code must reload the format under the mutation lock, and compound readers must select their backend from a current snapshot. Document that the one-time upgrade requires old clients/processes to be stopped; a format marker only rejects old clients that open afterward. An already-open new-version handle must switch correctly when another process upgrades.

2. **A state file is atomic; multiple independent reads of it are not a snapshot.** `head_commit_id` currently obtains HEAD in `current_ref`, then reads the named ref, or reads HEAD again (`core.rs:354-383`). If these become independent state loads, checkout/receive can interleave them and produce a result that never existed. Introduce `RefSnapshot` with HEAD resolution, ref lookup, and root enumeration performed against the same value. Reuse that snapshot for policy evaluation, old OID capture, and transaction preparation while holding the lock. Network advertisement already holds a lock; preserve that interval.

3. **Filesystem existence is an accidental second source of truth today.** Branch deletion tests/removes a loose ref (`branch_merge.rs:8-17`), checkout uses `branch_path.exists()` (`branch_merge.rs:52-65`), detached checkout directly writes HEAD (`branch_merge.rs:77`), `update_head_to_commit` directly writes detached HEAD (`core.rs:381`), and clone directly writes HEAD (`remote/mod.rs:413-415`). Replace all such decisions and writes through the backend/snapshot interface. Stale legacy HEAD and refs may remain as inert upgrade artifacts but must never influence format 2 behavior, even if manually changed or corrupted. Avoid requiring legacy refs directory contents to validate a format 2 repository.

4. **Reflog must record HEAD transitions as well as ref transitions.** Branch checkout can change HEAD from A to B without changing any ref; detached checkout/reset can otherwise lose the only root for A. A record needs optional old/new commit OIDs and enough symbolic HEAD information to explain changes. Ref create/delete need `None` endpoints, not an all-zero synthetic OID. A transaction updating the attached branch should derive the HEAD movement from the same old/new snapshot; do not append a second independent state write. Any no-op logging policy should be explicit. Missing symbolic target is valid only for an unborn branch; detached HEAD must be a validated commit OID.

5. **GC, expiry, and recovery must share the writer lock.** `gc` currently assumes its caller acquired the lock; it is nevertheless public (`gc_verify.rs:9`). CLI does lock it, but a new public expiry/recovery API should lock internally and call private locked helpers. Mark every retained old and new reflog commit OID plus existing refs, detached/symbolic HEAD, MERGE_HEAD, FETCH_HEAD, and index roots. Preserve physical delta dependencies using current `mark_object`. Deduplicate roots and apply graph limits to the aggregate. Expiry must atomically remove entries under lock; recovery must locate a still-retained record, validate its closure, and create the recovery branch in the same lock interval. Otherwise expiry+GC can delete the target between lookup and publication.

6. **Preparation must precede publication side effects.** Validate the whole candidate map/HEAD/reflog, allocate sequence numbers, and serialize within the byte cap before receive installs objects or reset/checkout restores the working tree. In `receive`, validate every CAS and policy against one locked snapshot, validate the union closure in the overlay, prepare the candidate state, install/fsync the union of reachable staged objects, then replace the state once. A second-ref CAS or state-size failure must leave both refs untouched and install no newly received objects. Failed I/O during installation may leave unreachable objects, which is safe. State atomicity does not make worktree, index, or MERGE_HEAD updates atomic; preserve and document the narrower guarantee.

7. **Sequence number persistence must survive complete expiry.** Store `next_seq` or a transaction generation separately from retained entries; checked increment must fail before publication on overflow. Deriving the next value from the last surviving entry permits sequence reuse after expiry. Use sequence/generation for ordering; wall clocks can repeat or move backward. All entries for an atomic multi-ref operation should share a transaction identifier, with deterministic per-entry sequencing.

8. **Errors after rename can mean the mutation committed.** `atomic_write` renames before directory sync (`core.rs:759-780`). A final sync failure or response loss cannot safely be reported as definitely rolled back, and retrying can duplicate audit/reflog entries. Return a receipt containing committed generation/transaction ID when possible, and make the CLI explain ambiguous failure by requesting a ref/state reread. Do not append a contradictory denied event after the state replacement. Existing Windows `sync_directory` is a no-op; the design should retain the current platform durability qualification rather than promise identical power-loss durability everywhere.

9. **Reflog growth is bounded operationally, not by silent deletion.** An unbounded lifetime plus a 16 MiB state cap intentionally makes later mutations fail until explicit expiry. Check byte size of the exact serialized UTF-8 bytes and count/length limits, including actor/reason strings and JSON escaping. Bound the writer while serializing to avoid huge temporary allocations; bound upgrade accumulation so a million individually valid loose refs cannot consume several GiB before the final size check. Expiry must still be able to load and compact a state at the cap; do not append an ordinary reflog entry per expired entry. Preserve the generation counter when all logs are removed.

10. **The state parser is part of the repository integrity boundary.** Validate schema version, map names/OIDs, HEAD grammar, optional reflog OIDs, sequence uniqueness/order and counter consistency, transaction grouping limits, and bounded strings/counts. Reject malformed state; never fall back to legacy loose refs when format is 2. Consider a custom map visitor to reject duplicate JSON ref keys rather than silently keeping the last one. Retain portable case and file/directory namespace rules when moving from a filesystem namespace to an in-memory map, or explicitly decide/test the new semantics. Duplicate commands for the same ref in one transaction must be rejected.

## Small robust interface

Keep the current public single-ref wrapper and avoid exposing arbitrary state edits:

```rust
struct RefSnapshot {
    generation: u64,
    next_seq: u64,
    refs: BTreeMap<String, String>,
    head: Head, // Symbolic(local_branch) | Detached(validated_oid)
    reflog: Vec<ReflogEntry>,
}

enum Expected<T> { Any, Missing, Value(T) }
struct RefEdit {
    name: String,
    expected: Expected<String>,
    new: Option<String>, // None means deletion
}
struct HeadEdit { expected: Head, new: Head }
struct ChangeContext { actor: String, reason: String, time: i64 }
struct TransactionReceipt { generation: u64, transaction_id: String }

fn refs_snapshot(&self) -> Result<RefSnapshot>;
fn upgrade(&self) -> Result<()>; // internally acquires lock
fn transact_refs(&self, request: RefTransaction) -> Result<TransactionReceipt>;
fn reflog_expire(&self, selector: ExpirySelector) -> Result<TransactionReceipt>;
fn recover(&self, retained_sequence: u64, branch: &str) -> Result<TransactionReceipt>;
```

`Expected::Any` should be internal or tightly scoped: normal CLI and receive changes require exact CAS. Keep `load_snapshot_locked`, `prepare_transaction_locked`, and `publish_prepared_locked` private; one lock interval covers load through publication. A prepared transaction owns serialized bounded bytes and old generation, and cannot be published against a different generation. It is acceptable to omit `Any` entirely initially.

For receive, use `commit_many(updates, context, authorize)` where the authorization callback receives an immutable snapshot, immutable requested edits, and validated incoming view; it cannot independently publish refs or reacquire the repository lock. Run it before installing any object, then commit through the common prepared-state path. Bound the union closure, not only each individual closure. Return one receipt for the atomic batch.

For audit, successful events belong in the state transaction if their atomic association matters. Denied attempt events are a separate append-only journal: bounded event encoding, lock around each append, file sync, repair-or-reject behavior for a crash-truncated final record, and identifiers linking attempts to successful transactions. Explicitly decide behavior when denied-event persistence fails. Never claim tamper resistance against a repository owner who can edit these files.

## Upgrade protocol

Under the repository lock, reread format, read and validate legacy HEAD/ref snapshot and required closure roots, prepare a bounded complete state, atomically write/sync state, then atomically write/sync format 2 as the final compatibility marker. A leftover state with format 1 is ignored by ordinary operation and overwritten from the current loose snapshot by the next upgrade attempt. Format 2 with absent/invalid state fails closed. Repeating an already-completed upgrade validates and returns success without resetting generation or dropping retained history. Do not delete or mirror loose refs in the correctness path.

If the upgrade records a baseline, label it as an upgrade snapshot; historical deleted/detached commits from before upgrade cannot be reconstructed or protected automatically. Missing version metadata in supported legacy worktree layouts remains format 1.

## Focused tests

- Empty/bare/legacy missing-format and ordinary repositories upgrade; old-format tests continue to pass; repeated completed upgrade preserves counters/logs. Failure/crash before state rename, after state rename, before/after format rename produces either usable legacy state or complete format 2. A loose-ref change between failed upgrade and retry is included on retry.
- A new-version handle opened before another process upgrades reads/writes format 2 correctly afterward. Document the deliberately unsupported already-open old-binary case. If a previous binary fixture is available, confirm it rejects format 2 on reopen.
- Mutate stale loose HEAD/ref files after upgrade; list/checkout/delete/clone/update/reset/verify/GC consistently use the authoritative state. Corrupt state cannot trigger loose fallback.
- Checkout away from a detached commit, reset an attached branch, delete a branch, and force-update a ref. GC preserves every retained old/new closure. Expire selected entries and GC; objects are removed only when no ref, HEAD, other log, index, MERGE_HEAD, FETCH_HEAD, or delta dependency retains them. Recovery succeeds from each retained endpoint and fails after expiry without creating a branch.
- Multi-ref create/update/delete commits one generation. Wrong CAS or policy denial on the final command publishes none; duplicate names and invalid final namespace/HEAD are rejected. Crash at each object installation and before/after state rename preserves all-old or all-new refs and complete closures. Enforce union object/byte/graph caps across many small tips.
- Actor/reason/control characters and JSON escaping near the state limit: exact-cap state succeeds, one-byte overflow fails before object/worktree side effects. Full log remains readable and explicit expiry restores capacity. Expire all entries and verify subsequent sequence is strictly larger; checked sequence overflow changes nothing.
- Concurrent writer versus writer/expiry/recovery/GC, and a compound HEAD reader while symbolic/detached HEAD changes. Verify coherent snapshot generation and no missing reachable objects.
- Denied audit append interruption leaves prior complete events readable; successful state publication never appears as a denied mutation after injected post-rename failure. Audit/state write failure behavior matches the documented result semantics.

No source implementation edits or test execution were performed for this review.
