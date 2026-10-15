# Ownership and memory safety (M6)

M6 is in progress. PXR has no move, borrow, or lifetime syntax yet. Existing
programs retain PHP-style array copies and shared object identity. The runtime
allocation foundation described below does not establish compile-time ownership.

## Implemented: allocation lifecycle (M6.1)

Runtime allocations use a tracked heap with private, aligned headers. Unlike the
old bump arena, individual internal buffers can be released. Array growth and
formatting growth replace and free their previous buffers. Formatting releases
its scratch buffer after constructing the result.

An exit handler releases all remaining blocks, including arrays' entry buffers,
property buffers, mixed boxes, PHP handle records, and class metadata. It follows
the allocation list rather than the value graph, so shared references and cycles
are freed exactly once. This is terminal cleanup, not collection during execution.
Runtime fatal errors using `exit(255)` also run cleanup; aborts and signals do not.

Generated code continues to receive stable pointers with the same string, array,
object, and mixed layouts. Literal strings remain static compiler constants and
must never be passed to internal heap release. Only the allocator's own pointers
may be freed or resized. Allocation sizes and several expanding operations use
checked arithmetic; impossible sizes fail before a wrapped allocation or write.

## Remaining implementation

M6 still needs a consistent language-level ownership contract before values can
be dropped safely during execution:

* Distinguish owned values and shared/exclusive borrows in typed signatures and
  IR; check moves, reassignment, control-flow joins, and loop-carried state.
* Insert drops on all exits, including early returns, break/continue, and future
  cancellation paths. Return values must escape safely; temporaries must be
  released after their final use.
* Check borrow lifetimes across calls, returns, property/array storage, and
  future suspension points. A borrowed value cannot outlive its owner.
* Define how explicit ownership interacts with existing shared object identity,
  mixed boxes, generic specialization, and PHP handles. Cycles need an explicit
  policy; plain reference counting alone would leave cycles allocated.
* Verify memory stays bounded in repeated allocation/drop workloads, and reject
  use-after-move, overlapping mutable borrows, and escaping stack/local borrows.

Until those pieces are implemented, loops that replace strings, arrays, or objects
can still retain their old language values until exit. M6 completion requires
these compile-time rules and end-to-end tests, not just a clean exit-time heap.

See [`RUNTIME.md`](RUNTIME.md) for allocation APIs and the sanitizer test command,
and [`MILESTONES.md`](MILESTONES.md) for progress. Async implementation remains
sequenced after this ownership/lifetime work.
