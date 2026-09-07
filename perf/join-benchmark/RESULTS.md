# Join optimizer study: 2026-09-06

This study uses debug builds because repository rules prohibit release builds.
Execution work is the primary measure.
Planning time is outside the study.

## Measures

The runner consumes every result row and records statement metrics.
The main measures are VM steps and rows read.
These counters are more stable than elapsed time on a shared machine.

Elapsed time remains useful for large changes.
Each TPC-H query uses the same database and warm cache unless noted.

The study also saves the JSON query plan.
Each access node reports input rows, output rows, local cost, and total prefix cost.

## Baseline

The baseline used commit `b04d329547` and the analyzed benchmark databases.

| Query | VM steps | Rows read | Time |
|---|---:|---:|---:|
| TPC-H 5 | 19,424,408 | 3,555,780 | 26.37 s |
| TPC-H 9 | 58,993,989 | 6,649,413 | 107.76 s |
| Graph `a_cooccurrence` | 427,457 | 40,171 | 0.88 s |
| Graph `c_edge_counts` | 2,396,740 | 161,336 | 0.43 s |

The other six graph queries provide guard coverage.
Their baseline VM counts range from 1,060 to 289,758.

### Full TPC-H work sweep

This sweep used commit `0b61011880` after the first optimizer changes.
It measured all 21 single-statement TPC-H query files.

| Query | VM steps | Rows read |
|---|---:|---:|
| 1 | 204,236,651 | 6,001,215 |
| 2 | 37,023,962 | 9,177,720 |
| 3 | 12,482,022 | 2,087,813 |
| 4 | 6,274,158 | 1,530,882 |
| 5 | 12,448,655 | 1,898,579 |
| 6 | 30,186,092 | 6,001,215 |
| 7 | 77,004,157 | 9,512,865 |
| 8 | 17,677,931 | 2,308,497 |
| 9 | 58,996,167 | 6,649,413 |
| 10 | 12,671,395 | 1,834,164 |
| 11 | 8,060,382 | 1,621,704 |
| 12 | 30,940,235 | 6,001,215 |
| 13 | 55,776,321 | 1,800,000 |
| 14 | 27,791,644 | 6,001,215 |
| 16 | 7,829,268 | 803,084 |
| 17 | 157,048,512 | 18,209,416 |
| 18 | 75,230,762 | 6,001,287 |
| 19 | 27,079,598 | 6,001,215 |
| 20 | 37,804,191 | 7,159,641 |
| 21 | 124,631,788 | 6,697,731 |
| 22 | 18,967,954 | 1,800,000 |

Query 15 contains three statements that create, read, and drop a view.
The runner prepares one statement, so it cannot measure query 15 correctly.
The sweep does not count query 15 as measured.

## Hypotheses

### Finish connected components first

TPC-H 5 placed `region` after `orders` without a join condition.
This order scanned the small table once for each order row.

A forced connected order reduced VM steps to 16,100,394.
It reduced rows read to 2,445,401.
The result validated connected-prefix pruning.

### Build equality classes

TPC-H 5 contains `customer.nation = supplier.nation` and `supplier.nation = nation.id`.
The old optimizer could not infer `customer.nation = nation.id`.

A forced query with that implied equality used 12,447,183 VM steps.
It read 1,898,579 rows.
The result justified equality classes.

The implementation uses one representative column for each class.
It adds missing links in a star shape.
This shape avoids unnecessary links and unstable symmetric plans.

### Count constant filters inside compound lookups

The `region` lookup used both its join key and `region.name`.
The old estimate treated every joined key as a hit.
It therefore missed the tenfold reduction from the name filter.

The new estimate applies a consumed constant filter when a lookup also uses a join key.
This estimate moved `nation` and `region` before `lineitem`.

### Count unmatched outer rows

The old model counted matches for a left join but did not count preserved unmatched rows.
It also mispriced `WHERE right.key IS NULL` anti-joins.

The new model estimates the chance of no match from the average match count.
It uses that value for null tests on non-null right-side columns.

### Keep looking after an unavailable duplicate

An early equality-class version exposed a compound-index bug in TPC-H 9.
An unavailable duplicate on one index column stopped the next usable column.

The search now skips the unavailable duplicate.
It only stops when a later index column creates a real prefix gap.

### Replace a large automatic index with a hash join

TPC-H 17 built an automatic index over 6 million `lineitem` rows.
The filtered `part` input had an estimated 2,000 rows.

The optimizer now compares a hash join with that automatic-index build.
It keeps the index guard when the probe has a constant filter.
This guard prevents the slower hash plan found for TPC-H 14.

The TPC-H 17 result reduced VM steps from 157,048,512 to 113,333,747.
Rows read fell from 18,209,416 to 12,208,396.
Elapsed time fell from 85.21 seconds to 58.42 seconds.

### Run correlated EXISTS filters after selective joins

TPC-H 21 ran two correlated `EXISTS` filters after its first `lineitem` loop.
The later `supplier` and `nation` joins rejected most of those rows.
The old schedule still ran both filters for every earlier row.

Removing both filters reduced VM work to 37,525,287 steps.
The full query used 113,569,744 steps before this change.
Swapping the filter order did not change the read counters.
This result showed that both filters ran before either result was tested.

A forced `lineitem`-first plan used 209,150,140 VM steps.
That result rejected a join-order change.
An early version also moved scalar subqueries.
Exact parent comparisons found no gain and a small TPC-H 2 regression.

The optimizer now moves a correlated `EXISTS` filter to the valid join prefix
with the lowest estimated row count.
It does not move a filter from an outer join condition.
Scalar subqueries keep their old schedule.

TPC-H 21 VM steps fell from 113,569,744 to 41,034,425.
Rows read fell from 6,500,065 to 4,485,662.
B-tree seeks fell from 12,915,717 to 5,899,973.
Elapsed time fell from 155.47 seconds to 57.50 seconds.

## Final results

| Query | Baseline VM | Final VM | VM change | Baseline rows | Final rows | Row change |
|---|---:|---:|---:|---:|---:|---:|
| TPC-H 5 | 19,424,408 | 12,447,165 | -35.9% | 3,555,780 | 1,898,579 | -46.6% |
| TPC-H 9 | 58,993,989 | 58,993,419 | 0.0% | 6,649,413 | 6,649,413 | 0.0% |
| TPC-H 17 | 157,048,512 | 113,333,747 | -27.8% | 18,209,416 | 12,208,396 | -33.0% |
| TPC-H 21 | 124,631,788 | 41,034,425 | -67.1% | 6,697,731 | 4,485,662 | -33.0% |
| Graph `a_cooccurrence` | 427,457 | 427,457 | 0.0% | 40,171 | 40,171 | 0.0% |
| Graph `c_edge_counts` | 2,396,740 | 101,243 | -95.8% | 161,336 | 8,077 | -95.0% |

TPC-H 5 measured time fell from 26.37 seconds to 9.82 seconds.
TPC-H 9 kept the same work and completed in 100.63 seconds.

All other graph queries kept the same VM steps and rows read.
The graph suite found no deterministic work regression.

## Commands

Print plans:

```bash
cargo run -p turso-join-benchmark -- \
  --database perf/tpc-h/TPC-H.db \
  --query-dir perf/tpc-h/queries \
  --query 5 --query 9 --plans
```

Measure TPC-H execution:

```bash
cargo run -p turso-join-benchmark -- \
  --database perf/tpc-h/TPC-H.db \
  --query-dir perf/tpc-h/queries \
  --query 5 --query 9 \
  --warmups 1 --repetitions 3 \
  --timeout-seconds 300
```

Measure the analyzed graph suite:

```bash
cargo run -p turso-join-benchmark -- \
  --database perf/graph-queries/graph-queries-analyzed.db \
  --query-dir perf/graph-queries/queries \
  --warmups 1 --repetitions 3
```
