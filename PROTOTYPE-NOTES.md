# PROTOTYPE NOTES — full ptrmap coverage for autovacuum DBs (PR2 prep)

Branch `bounty/ptrmap-full-coverage` (local only — never push, never open PRs).
Goal: a multi-table `auto_vacuum=full` DB passes stock SQLite `PRAGMA
integrity_check`. Builds on `main` (`cca14b3f`); deliberately does NOT duplicate
the freelist-side fixes of open PR #8812.

Provenance note: this working state was resumed mid-flight after a prior agent
session stalled; its uncommitted tree changes were recovered from this
workspace's git stash (`git stash show -p`) plus this session's additions
(stale-buffer clears at fresh-balance entry points) and committed together.

## Design (as implemented)

Single choke points, buffer-then-drain everywhere blocking IO is possible, every
drain point re-entrant. All code is `#[cfg(feature = "autovacuum")]` AND
runtime-gated on `AutoVacuumMode::Full` (`ptrmap_put` must never run on
non-autovacuum DBs).

### 1. Overflow chains — `fill_cell_payload` (core/storage/btree.rs)

`fill_cell_payload` is the single place overflow pages are allocated (real
callers: `WriteState::Insert` and `OverwriteCellState::FillPayload`; other
callers are `#[cfg(test)]` via `run_until_done`, which drives the same
persistent state machine, so they are compatible).

- `FillCellPayloadState::CopyData` gained `pending_ptrmap:
  Vec<(u32, PtrmapType, u32)>` (buffer lives in the persisted enum, so re-entry
  across IO yields neither loses nor duplicates entries).
- In the `AllocateOverflowPage` arm, after linking a new overflow page:
  `(page, Overflow1, btree_page)` for the first page of a chain, or
  `(page, Overflow2, prev_overflow_page)` for later pages. Pure push, no IO.
- When the copy completes, instead of returning `Done` the state transitions to
  `WritePtrmap { entries, idx }`, which drains via
  `return_if_io!(pager.ptrmap_put(..))`, advancing the persisted `idx` only
  after each put completes. Re-entering re-issues the in-flight entry; the put
  resumes via the pager's own `PtrMapPutState` and rewrites the same bytes
  (idempotent). No overflow allocation can ever re-run after a ptrmap yield
  (no double allocation).
- If balancing later moves the cell, the overflow entries are re-asserted by
  the balance-side walks below (later writes win; drain order = push order).

### 2. B-tree node entries + reparenting — balance paths

Every non-root b-tree page needs `(BTreeNode, parent)`. Pages are first linked
in exactly three places, plus cells/children move during balancing:

- `balance_root` (root split): the fresh child gets `(BTreeNode, root)`, then
  `queue_page_ptrmap_refs(child)` re-asserts the child's references (their
  parent changed from root to child, including first-overflow parents of moved
  cells).
- `balance_quick` (append fast-path): the fresh rightmost leaf gets
  `(BTreeNode, parent)`, then `queue_page_ptrmap_refs(new_leaf)` re-asserts the
  moved overflow cell's chain with the leaf as `Overflow1` parent.
- `balance_nonroot` (`NonRootDoBalancingFinish`), AFTER the page-number
  reassignment so ids are final: every surviving sibling page gets its own
  `(BTreeNode, parent)` self-entry plus `queue_page_ptrmap_refs(page)`
  (children, rightmost pointer, and `Overflow1` parents of cells that landed on
  it); finally `queue_page_ptrmap_refs(parent)` re-asserts divider-cell
  overflow chains now hanging off the parent and, in the balance-shallower
  case, the children absorbed by the root.

`queue_page_ptrmap_refs` is a pure walk over the already-loaded, dirty page
buffer (parses cells via `cell_get_raw_region` + `read_btree_cell`); it emits
`(BTreeNode, page)` for each direct child and `(Overflow1, page)` for each
cell-overflow chain start. Rewriting an unchanged entry is a harmless idempotent
overwrite; leaves produce no child entries.

Correctness notes:
- The reassignment block permutes page ids only among pages that all require
  the identical entry `(BTreeNode, parent)`, so old entries stay correct;
  genuinely new pages (possibly freelist-reused ids with stale entries) get
  overwritten by their self-entry.
- Divider cells deferred into the parent's `overflow_cells` are not walked by
  the parent pass, but they can never terminate a balance: any level with
  overflow cells is not "already balanced", so the next balancing round
  collects them and the page they land on re-asserts their chains.
- Balance-shallower: the absorbed page's own entry is simply not re-queued;
  with PR #8812 its free will write the `FreePage` entry.

### 3. IO placement (the resolved design question)

Blocking IO (`ptrmap_put` → `read_page`) happens in exactly two states, both
re-entrant by construction:

1. `FillCellPayloadState::WritePtrmap` — after all payload copying is done; the
   copy/allocation loop can never be re-entered.
2. `BalanceSubState::WritePtrmap { idx }` — a dedicated state in the
   `balance()` driver, reached only at balance()'s single completion point
   (the `Start` arm's "already balanced" exit), after all structural work.
   The structural states (`NonRootDoBalancing*`, `FreePages`) are not
   re-entrant (a yield inside would redo allocations), which is why the
   previous prototype's blocking-IO placement was unresolved: it is resolved
   here by never putting blocking IO inside them — only pure buffer pushes.
   On IO, `balance()` re-enters `WritePtrmap` directly with the persisted
   `idx` (the balanced-check is pure and re-runnable, the stack is stable
   during the drain).

Stale-buffer hygiene: entries buffered by an aborted balance (IO error path)
are dropped at the three fresh-balance construction sites (`WriteState::Insert`
balancing transition, overwrite transition, `DeleteState::Balancing`), so a
later balance can never flush entries from an earlier, aborted one.

## Validation status (honest)

Validated:
- `cargo check -p turso_core` clean (default features include `autovacuum`, so
  all new code is compiled and exercised).
- `cargo fmt --check` clean (CI's cargo-fmt-check).
- `cargo check -p turso_core --no-default-features` produces exactly the same
  15 pre-existing errors as `main` (`uuid`, io/encryption-gated modules; none
  in `btree.rs`/`pager.rs`) — that feature combination is broken on `main`
  already; this change adds zero new errors there.
- Design-level yield-safety reasoning above (state persisted in enums,
  idempotent retries, single re-entrant drain points).

NOT validated (explicitly):
- NO runtime test was executed: no DB was created, no `PRAGMA integrity_check`
  was run. This is compile-checked, design-reviewed prototype state, not a
  green-lighted fix. (An earlier ~270-line prototype from session history with
  equivalent coverage turned 101/8 integrity errors into `ok` on 3 of 4
  multi-table churn scenarios — evidence the approach works, but not evidence
  THIS code passes.)
- Freelist-side gaps remain by design (PR #8812 covers `FreePage` entries in
  `free_page` + the database-size persistence fix): stock SQLite
  integrity_check will still report `Freelist: Failed to read ptrmap key=N`
  for freed pages until #8812 lands. The two PRs are complementary.
- `VACUUM` / incremental-vacuum page-relocation paths untouched.
- No fuzz/simulator exposure.

## Suggested validation plan (when maintainer feedback arrives)

1. Port the repro harness from PR #8812's tests (page_size=512; ptrmap pages
   at 2, 105, ...): create a DB with `auto_vacuum=full`, several tables,
   oversized rows (multi-page overflow chains), interleaved inserts/deletes to
   force splits, rebalances, balance_quick appends and balance-shallower
   shrinks; then `PRAGMA integrity_check` via stock SQLite CLI.
2. Assertion set: zero `Failed to read ptrmap key=N` errors for live (non-
   freelist) pages; `integrity_check` = `ok` once #8812's FreePage entries are
   also present; spot-check `ptrmap_get` parent/type for pages involved in a
   forced `balance_nonroot` (log page ids).
3. Yield coverage: run the same scenario through the injected-yield test
   harness (`injected_yields`) and under the async path so every
   `return_if_io!` edge in the two `WritePtrmap` states is actually hit;
   assert no duplicated/lost entries (entry count per page id).
4. Simulator: enable autovacuum in a simulator scenario for a soak run.
