# Stage 3 final whole-change review

Reviewed 2026-10-22 against `docs/superpowers/plans/2026-10-22-my-vcs-stage3.md` and `task-{1,2,3}-brief.md`, using the code-reviewer template. Read-only source review; only this report was written in the workspace. No Git commands or source/test edits.

## Scope and evidence

- Verified byte-for-byte that current source matches `candidate-source.tar.gz`, SHA-256 `20d6ec3c7749912074132d574cab3d331a188cfa75b3bd582f0cc0b260fc8d61`; independently regenerated the baseline-to-candidate diff in memory and confirmed equality with `final.diff`.
- Reviewed the complete production change, the task reports and new test coverage, with focused inspection of surrounding receive/pack, worktree, lock, and GC integration. Cross-feature checks covered state authority, CAS/closure checks, object installation, successful audit publication, retained recovery roots, merge phases, v1/v2 compatibility, upstream/tag/prune routing, and forced-command identity.
- Inspected final WSL/openEuler test logs (each 130 passing), compatibility, gate and real SSH acceptance evidence. Did not rerun the full suite. Existing evidence does not claim native Windows or power-loss validation, and this review does not extend those claims.
- Ran one focused executable reproduction in a fresh WSL `/tmp` directory for the finding below. No server or existing repository was modified.

## Strengths

- The single authoritative format-2 state makes references, HEAD endpoints and successful remote update audit share one commit point. Candidate validation and final authorization occur under the same lock before object installation/publication; stale-CAS vectors fail as a whole.
- GC retains reflog endpoints and merge-journal roots, including the window before MERGE_HEAD publication. Receive overlay validation and private transfer snapshots preserve the stage-2 object-integrity boundaries.
- Merge readiness is explicit and persisted only after materialization; abort resets readiness before restoration and retains a retryable journal. Text bounds and conservative structural/multiple-base rejection are clearly documented.
- Both legacy and extended network writes use trusted service identity and final policy checks. The SSH gate parses a narrow command grammar and constructs a fresh environment; the docs clearly distinguish this boundary from file transport and OS-account authority.
- Operational documentation accurately limits atomicity, success-acknowledgment certainty, upgrade coexistence and platform durability claims.

## Issues

### Critical (must fix)

None identified.

### Important (should fix)

1. **[P2] Keep format-1 attempt audit inspectable without upgrading** — `src/repo/policy.rs:225-227`.

   `audit_list` loads the attempt journal and then unconditionally calls `r.reflog(None)?`. Reflog requires format 2, so every `myvcs audit` invocation on a format-1 repository fails, even when its valid attempt journal already contains recorded remote reads/failures. The service deliberately records these attempts for compatibility repositories, caps them at 10,000 records, and tells operators to export before expiry. The matching `audit expire` command works on format 1, leaving the supported CLI able to discard those entries but unable to inspect/export them. This is distinct from the intentionally unavailable atomic success audit on format 1.

   Reproduction with the current WSL debug binary:

   ```text
   myvcs init --bare /tmp/myvcs-final-audit-review-ck7ms_mk/server
   myvcs clone file:///tmp/myvcs-final-audit-review-ck7ms_mk/server /tmp/myvcs-final-audit-review-ck7ms_mk/client
   # audit-attempts.json now contains two valid records; both commands succeeded.
   myvcs --repo /tmp/myvcs-final-audit-review-ck7ms_mk/server audit
   # exit 1: team references require format 2; stop all old myvcs processes,
   # then run `myvcs upgrade --quiescent`
   myvcs --repo /tmp/myvcs-final-audit-review-ck7ms_mk/server audit expire --keep 0
   # exit 0; format remains 1.
   ```

   Fix by including success records only when the current repository format supports reflog, while always returning validated attempt records. Preserve fail-closed parsing and unsupported-format errors; do not blanket-ignore reflog errors. Add a regression using a real format-1 service session, inspect its attempt records through `audit`, expire a subset, and confirm continued listing without upgrade or fabricated success entries. Existing format-2 combined-audit coverage should continue passing.

### Minor (nice to have)

None identified that warrants a separate finding.

## Recommendations

Make the narrow audit compatibility fix, run its focused regression plus the existing format-2 audit maintenance test, and refresh the final source/diff/hash artifacts and affected validation evidence. No second functional finding was confirmed in this whole-change review.

## Assessment

**Ready to merge? With fixes.**

The reference/receive/GC and SSH authorization architecture meets the stated scope and has substantial failure/concurrency evidence. The format-1 audit listing regression should be corrected before delivery because it breaks the maintenance path for a deliberately supported repository mode.
