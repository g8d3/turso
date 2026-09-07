# Turso FST fork

Imported from https://github.com/quickwit-inc/fst at commit
`29e16b3ec1b62fa21bcf4ba65d28088e67a5663f`, the source commit recorded in
the published `tantivy-fst` 0.5.0 package. Its `src` tree was compared byte
for byte with the registry package before import. MIT and Unlicense notices
are retained. Rustfmt-only normalization is recorded separately.

The import makes no format or behavior changes. Both local Tantivy and its
SSTable companion use this path dependency.

The next layer adds `raw::Node::from_range`: validated decoding from a window
ending at a node's original file address. Node windows need at most 4,619
bytes, including the largest 256-transition node. Transition targets retain
global addresses even when they are outside the window. This preserves
format versions 1 and 2 and does not require rebuilding existing indexes.

`asynchronous::Fst` opens with two 16-byte metadata reads, then performs
lookup and automaton streaming through an injected `RangeReader`. No runtime
is used. Node read requests are at most 4,619 bytes. Only one returned node
window is retained; stream ancestors keep addresses and automaton state,
so traversal state grows with key depth. Reader cache/backing allocations
are outside this accounting.

Delayed-reader tests compare all 20,001 keys with the resident reader,
track retained range bytes, repeatedly poll pending operations, and check
errors and cancellation without skipped results. These APIs alone do not
page Tantivy dictionaries or impose a query memory cap: callers still need
to use suspendible traversal and page the separately encoded term information.
