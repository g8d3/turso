# Join optimizer study: 2026-09-06

Current runtime sweeps use release builds and the production hash memory budget.
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
| 20 | 109,156,533 | 30,916,254 |
| 21 | 124,631,788 | 6,697,731 |
| 22 | 18,967,954 | 1,800,000 |

Query 15 contains three statements that create, read, and drop a view.
The runner prepares one statement, so it cannot measure query 15 correctly.
The sweep does not count query 15 as measured.

A final release sweep measured the same 21 single-statement queries.
It used the production 64 MB hash memory budget.

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

### Materialize an IN-driven hash build

TPC-H 20 built a temporary B-tree over 6 million `lineitem` rows.
The outer `partsupp` read used an `IN` seek and produced about 894 estimated rows.

A global grouped table used 51,024,156 VM steps in an isolated query.
That form can run `sum` for unused keys and expose an overflow that the source query avoids.
The result rejected a general group-first rewrite.

A general hash rule changed TPC-H 14 from 27,791,557 to 103,693,634 VM steps.
Elapsed time rose from 14.78 seconds to 134.55 seconds.
The final rule keeps that guard.
It only considers this hash form when an `IN` seek filters the build input.

The optimizer now stores the selected build rows before it builds the hash table.
It stores the join keys and payload together.
This avoids a base-table seek for each hash match.

Release builds use the production 64 MB hash budget.
Three measured runs reduced mean elapsed time from 6.18 seconds to 2.65 seconds.
This is a 57.1% reduction.
Mean VM steps rose from 109,156,541 to 118,489,525.
B-tree seeks fell from 938,602 to 39,997.
Both plans returned 184 rows.

The change affected only query 20 across all 22 TPC-H plan files.
All eight graph query plans stayed unchanged.

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

### Use a hash anti-join for a large NOT EXISTS

The release sweep found that TPC-H 22 was the slowest supported query.
It built a temporary B-tree over 1.5 million `orders` rows.
The query then made 1,519,031 index seeks.

The new plan builds a hash table from the filtered `customer` rows.
It scans `orders` once and marks matching customer rows.
It then emits the customer rows that have no match.

The implementation uses the existing outer hash join match bits.
It also uses the existing spill path for large build inputs.
Anti-join match filters do not run again when unmatched rows are emitted.

Three release runs reduced mean time from 12.84 seconds to 0.54 seconds.
This is a 95.8% reduction.
VM steps fell from 18,964,160 to 16,489,945.
Total B-tree seeks fell from 1,519,031 to 6,434.
Both plans returned seven rows.

Only TPC-H 22 changed across all 22 TPC-H plan files.
All eight graph query plans stayed unchanged.

### Hash an indexed prefix before an unfiltered probe

The release sweep found that TPC-H 9 took the most time after the earlier changes.
It scanned 6 million `lineitem` rows and made 7,654,290 B-tree seeks.

A release profile collected 10,980 samples.
The system spent 39.2% of the samples in `pread`.
B-tree move and seek functions were the hottest Turso functions.

The first test allowed every indexed build input to use a hash join.
That rule made TPC-H 14 increase from 1.14 seconds to 4.50 seconds.
It also made TPC-H 10, 5, and 7 slower.
The results rejected the broad rule.

The final rule requires an unfiltered probe input.
It also keeps the earlier exception for an `IN` seek on the build input.
This rule lets TPC-H 9 hash its filtered `part` and `partsupp` prefix.
It then scans `lineitem` once and probes the hash table.

Three release runs reduced mean time from 11.76 seconds to 2.75 seconds.
This is a 76.6% reduction.
B-tree seeks fell from 7,654,290 to 1,046,815.
This is an 86.3% reduction.
VM steps rose by 12.4% because each scanned row now probes the hash table.
Both plans returned 175 rows.

The final release sweep ran all 21 supported single-statement TPC-H queries.
Only TPC-H 9 and 11 changed plan shape.
TPC-H 11 changed by 0.65%, which is not a meaningful runtime change.
All eight graph query plans stayed unchanged.

### Estimate complete unique hash keys together

The old model multiplied the estimates for each column in a composite hash key.
That calculation treated related key columns as independent values.
For TPC-H 9, it estimated less than one output row from 6 million probe rows.

The new model detects a complete unique key on the hash build input.
It divides the probe rows by the build table rows for that key.
This estimate is still conservative for filtered data, but it is much closer.

### Count the probe scan in hash join cost

The old hash cost counted each hash probe but did not count the probe table scan.
The new cost includes the scan pages and row work.
This change did not alter any measured TPC-H or graph plan shape.

## Final results

| Query | Baseline VM | Final VM | VM change | Baseline rows | Final rows | Row change |
|---|---:|---:|---:|---:|---:|---:|
| TPC-H 5 | 19,424,408 | 12,447,165 | -35.9% | 3,555,780 | 1,898,579 | -46.6% |
| TPC-H 9 | 58,993,989 | 66,301,166 | +12.4% | 6,649,413 | 6,650,707 | +0.0% |
| TPC-H 17 | 157,048,512 | 113,333,747 | -27.8% | 18,209,416 | 12,208,396 | -33.0% |
| TPC-H 20 | 109,156,541 | 118,489,525 | +8.5% | 30,916,254 | 31,123,606 | +0.7% |
| TPC-H 21 | 124,631,788 | 41,034,425 | -67.1% | 6,697,731 | 4,485,662 | -33.0% |
| TPC-H 22 | 18,964,160 | 16,489,945 | -13.0% | 1,800,000 | 1,819,031 | +1.1% |
| Graph `a_cooccurrence` | 427,457 | 427,457 | 0.0% | 40,171 | 40,171 | 0.0% |
| Graph `c_edge_counts` | 2,396,740 | 101,243 | -95.8% | 161,336 | 8,077 | -95.0% |

TPC-H 5 measured time fell from 26.37 seconds to 9.82 seconds.
TPC-H 9 release time fell from 11.76 seconds to 2.75 seconds.
TPC-H 20 release time fell from 6.18 seconds to 2.65 seconds.

All other graph queries kept the same VM steps and rows read.
The graph suite found no deterministic work regression.

## Commands

Print plans:

```bash
cargo run --release -p turso-join-benchmark -- \
  --database perf/tpc-h/TPC-H.db \
  --query-dir perf/tpc-h/queries \
  --query 5 --query 9 --plans
```

Measure TPC-H execution:

```bash
cargo run --release -p turso-join-benchmark -- \
  --database perf/tpc-h/TPC-H.db \
  --query-dir perf/tpc-h/queries \
  --query 5 --query 9 \
  --warmups 1 --repetitions 3 \
  --timeout-seconds 300
```

Measure the analyzed graph suite:

```bash
cargo run --release -p turso-join-benchmark -- \
  --database perf/graph-queries/graph-queries-analyzed.db \
  --query-dir perf/graph-queries/queries \
  --warmups 1 --repetitions 3
```
