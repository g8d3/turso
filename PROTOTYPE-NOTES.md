# PR2 prep — full ptrmap coverage for autovacuum databases

Branch `bounty/ptrmap-full-coverage` (built on `main` @ `cca14b3f`, **local only**,
no push, no PR). Companion to PR #8812 (freelist-side `FreePage` entries +
`database_size` persistence) — this work deliberately does **not** duplicate that
scope; the two are complementary and a full integrity-check pass needs both.

Goal: a multi-table `auto_vacuum=full` database built by turso passes stock
SQLite `PRAGMA integrity_check` on the pointer-map side (issue #6774; prior art
#3894/#6193).

## Design implemented

Choke points, following the settled design:

1. **Overflow chains — `fill_cell_payload` (`core/storage/btree.rs`).** The
   single place overflow pages get allocated for real callers (`WriteState::Insert`
   and `OverwriteCellState::FillPayload`; `#[cfg(test)]` callers use the same
   state machine and stay compatible).

   - `FillCellPayloadState::CopyData` gained a `pending_ptrmap:
     Vec<(u32, PtrmapType, u32)>` buffer (behind `#[cfg(feature = "autovacuum")]`).
   - In the `CopyDataState::AllocateOverflowPage` arm, when the pager is in
     `AutoVacuumMode::Full`, linking a new overflow page pushes
     `(new_page_id, Overflow2, previous_overflow_page)` when the chain has a
     predecessor, else `(new_page_id, Overflow1, btree_page_holding_the_cell)`.
     This matches SQLite's ptrmap semantics (btree.c `ptrmapPutOvflPtr`).
   - When the payload copy completes, the state transitions to
     `FillCellPayloadState::WritePtrmap { entries, idx }` instead of returning
     `Done`. That arm drains the buffer via
     `return_if_io!(pager.ptrmap_put(..))`, advancing the persisted `idx` only
     after each put completes. On IO yield the state is re-entered unchanged,
     the same entry is re-issued, and `Pager::ptrmap_put` resumes via its own
     persisted `PtrMapPutState`; overwrite semantics make the retry idempotent.
     The buffer lives entirely inside the persisted state enum, so re-entry
     across yields neither loses nor duplicates entries.

2. **B-tree page entries + reparenting — balance paths.** Every non-root btree
   page needs a `(BTreeNode, parent)` entry, and pages referenced by cells that
   move between pages need their entries re-asserted (both `BTreeNode` parents
   of children and `Overflow1` parents of moved cells).

   - A pure-memory helper `queue_page_ptrmap_refs(page, usable_space, out)`
     walks a loaded page and buffers: each cell's left child (and the
     rightmost pointer for interior pages) as `(BTreeNode, page)`, each
     overflowing cell's first overflow page as `(Overflow1, page)`, and the
     same for deferred cells held in `overflow_cells` (full cell images parsed
     by `queue_cell_image_ptrmap_refs` — covers the unmaterialized divider
     cells and the cell `balance_quick` moves).
   - **Buffer, don't block:** the previous prototype's unresolved problem was
     blocking-IO placement inside non-reentrant balance states. Resolved by
     buffering entries into `BalanceState::pending_ptrmap` (all pushes are
     memory-only) and draining them in a new re-entrant
     `BalanceSubState::WritePtrmap { idx }` state at the **single completion
     point of `balance()`** (the `Start` arm's `return Done`, gated on
     `AutoVacuumMode::Full` and a non-empty buffer). `balance_root`,
     `balance_quick`, and `balance_nonroot` all funnel through that exit, so
     no blocking IO was added to any state that would redo work on re-entry.
   - Hook points:
     - `balance_root`: the fresh child (old root content) gets
       `(BTreeNode, root)` plus a walk of its references; the root itself
       keeps no entry (root pages are referenced only by the schema page;
       `PTRMAP_ROOTPAGE` entries only arise from vacuum relocation, out of
       scope).
     - `balance_quick` (append fast-path): the new rightmost leaf gets
       `(BTreeNode, parent)` plus a walk (the moved overflow cell may carry an
       `Overflow1` chain that now hangs off the new leaf).
     - `balance_nonroot` `NonRootDoBalancingFinish`: after page-number
       reassignment (so entries use final ids), after divider insertion and
       cell movement, and after the balance-shallower fold (the absorbed
       sibling is skipped — `sibling_count_new` is already decremented): each
       surviving sibling gets `(BTreeNode, parent)` plus a walk; then the
       parent is walked, which re-asserts entries for children moved between
       siblings, promoted divider cells and their overflow chains. Duplicate
       entries are harmless: `ptrmap_put` overwrites in place, and the walk
       ordering guarantees the last write reflects the final structure.

   This mirrors SQLite's `reparentChildPages` approach in `balance_nonroot`.

## IO-placement decisions (the part this design had to settle)

- **Why buffer+flush instead of inline `ptrmap_put` in balance states:**
  `balance_root`/`balance_quick` restart from their top on yield (their only
  IO — `do_allocate_page` — is at the top), so any `return_if_io!` placed
  after their mutations would re-run allocations and duplicate structural
  work. `balance_nonroot`'s `NonRootDoBalancingFinish` is likewise not
  re-entrant mid-body. The only re-entrant point is the `balance()` driver's
  `Start` arm, so that is where the drain state lives. Cost: entries are
  written after the structural work instead of interleaved — irrelevant for
  correctness because ptrmap entries are absolute overwrites and nothing reads
  them mid-balance.
- **fill_cell_payload drains its own entries before returning Done**, so the
  pager's single shared `ptrmap_put_state` is never used by two logical puts
  at once within one write operation (fill → insert → balance are strictly
  sequential on one cursor).

## Guards

- All new state fields/arms/helpers are behind
  `#[cfg(feature = "autovacuum")]`; the variant `BalanceSubState::WritePtrmap`
  itself is unconditional (with a `#[cfg(not(...))]` no-op transition in the
  driver) to avoid cfg-gating every match arm.
- Every push/drain is additionally gated at runtime on
  `pager.get_auto_vacuum_mode() == AutoVacuumMode::Full`, so non-autovacuum
  databases never touch `ptrmap_put`.

## Compile status

- `cargo check -p turso_core` (default features, includes `autovacuum`):
  **clean**; `cargo fmt --check`: clean; clippy shows no findings in
  `btree.rs`.
- `cargo check -p turso_core --no-default-features`: fails, but identically
  to pristine `main` (verified by stashing: same 8 errors, all in
  `incremental/dbsp.rs` — unresolved `uuid` — and `vdbe/execute.rs`). The
  baseline feature-combination build is broken on main independent of this
  change; zero errors/warnings originate from `btree.rs` in any configuration.

## What is validated vs NOT (honest)

Validated:

- Compiles clean (default config), fmt-clean, no clippy findings in the
  touched file; `--no-default-features` delta-vs-main is zero.
- The state machines are yield-safe by construction (buffers live in the
  persisted enums; drain indices only advance on completed puts; retries are
  idempotent overwrites) — by reasoning, not by crash-injection tests.

NOT validated (prototype hand-off to maintainer feedback):

- **No runtime test was run**: no `auto_vacuum=full` end-to-end scenario, no
  stock-SQLite `PRAGMA integrity_check` comparison, no simulator run. The
  prior prototype's signal (churn scenarios went from 101/8 integrity errors
  to `ok` on 3 of 4) applies to the same approach but not to this exact code.
- Freelist-side `FreePage` entries are intentionally absent (PR #8812's
  scope). Until combined with it, integrity checks on DBs with deleted pages
  will still report freelist ptrmap errors — expected.
- `PTRMAP_ROOTPAGE` entries (root relocation during vacuum) are out of scope.
- Known residual risks, documented for the maintainer discussion:
  - `Pager::ptrmap_put` state is a single shared slot; two cursors on the same
    pager whose write operations interleave across IO yields could trample
    each other's in-flight put. Same exposure pattern as PR #8812's
    `free_page` ptrmap write; within one cursor's sequential phases it cannot
    happen.
  - If a cursor is dropped mid-balance or an error aborts a balance, buffered
    entries are lost (page structure itself is also partially mutated in that
    case — pre-existing behavior class, not worsened here).
  - Perf: every balance in `auto_vacuum=full` now parses all cells of each
    balanced page and writes a handful of ptrmap entries (ptrmap pages are
    hot in cache; entries collapse onto few pages). Not benchmarked.

## Suggested validation plan (when maintainer feedback arrives)

1. Port the prior session's scenario harness: create DB with
   `PRAGMA page_size=512; PRAGMA auto_vacuum=FULL;`, 3+ tables, mixed
   insert/delete churn past several ptrmap boundaries, big blobs to force
   overflow chains and balances (including appends that hit `balance_quick`).
2. Compare against stock SQLite: open the file read-only, run
   `PRAGMA integrity_check`, assert `ok` once #8812 + this change are
   combined; without the BTreeNode/Overflow work it reports
   `Failed to read ptrmap key=N`.
3. Crash/yield injection: run the same scenarios with the injected-yield
   feature (`io_memory_yield` / simulator) to exercise re-entry through
   `FillCellPayloadState::WritePtrmap` and `BalanceSubState::WritePtrmap`.
4. Regression-test the non-autovacuum path (default mode) to confirm the
   runtime guard keeps ptrmap_put unreachable.

## Concurrency note (process, not code)

While this session was implementing, a concurrent agent session in this same
worktree applied `cargo fmt` and extended `queue_page_ptrmap_refs` with
deferred-`overflow_cells` coverage (`queue_cell_image_ptrmap_refs`). That
addition closes a real gap (unmaterialized cells during balancing) and was
absorbed and compile-verified rather than reverted. The final committed tree
is the merged result; anything that writer lands afterwards is theirs and
layers on top via git.
