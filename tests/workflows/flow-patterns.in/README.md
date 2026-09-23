# Flow pattern test fixtures

Groundwork for the flow visualization layout tests (loan 33a55c30). Each
sub-directory is one independent pebbles store (`.pebbles` with `.no_jj`),
named after a graph pattern. The store loads with zero warnings
(`PEBBLES_DB_PATH=<dir>/.pebbles pebbles list`) and `pebbles reindex` is a
no-op on every store.

## Stores

- `diamond/` - A -> B, A -> C, B -> D, C -> D. Two converging paths.
  Layers (--show-done): d1aaaaaa=0, d2bbbbbb/d3cccccc=1, d4dddddd=2.
  Statuses: todo, in_progress, todo, proposed. No done node, so the shape
  is identical with and without --show-done.
- `fan-out/` - A -> B, A -> C, A -> D (multiple dependents). Layers:
  f1aaaaaa=0, f2bbbbbb/f3cccccc/f4dddddd=1. f4dddddd is done (hidden by
  default). f5eeeeee is an epic; f2/f3/f4 are part_of it, so the epic has
  no flow edges and renders standalone. f2/f3/f4 are blocked by their
  non-done root even though they are git-complete leaves.
- `fan-in/` - A -> D, B -> D, C -> D (multiple dependencies). Layers:
  b1aaaaaa/b2bbbbbb/b3cccccc=0, b4dddddd=1. b2bbbbbb uses
  `scheduled_before` (a flow edge that blocks like required_for); the other
  two use `required_for`. All three sources non-done, so b4dddddd is fully
  blocked. b5eeeeee is a done standalone.
- `chain/` - A -> B -> C -> D -> E (linear). Layers (--show-done):
  c1aaaaaa=0, c2bbbbbb=1, c3cccccc=2, c4dddddd=3, c5eeeeee=4. c1aaaaaa is
  done, so by default the chain renders B..E with B at layer 0 and B is
  NOT blocked (its only predecessor is done). c2bbbbbb blocks c3cccccc,
  which blocks c4dddddd, which blocks c5eeeeee.
- `disconnected/` - Multiple unrelated chains plus one fully standalone
  issue. Chain one: a1aaaaaa -> a2bbbbbb -> a3cccccc (layers 0-2). Chain
  two: a4dddddd (done) -> a5eeeeee: a4 is done, so a5eeeeee is not blocked
  and shows as standalone in the default view. Chain three:
  a6ffffff -> a7a7a7a7 via `scheduled_before` (layers 0-1). Solo:
  a8b8b8b8 (done, standalone).
- `mixed/` - One store combining the shapes. A single connected component
  runs diamond (ab000002 -> ac000003/ad000004 -> ae000005 -> af000006 ->
  ba000007 -> bb000008/bc000009/bd00000a), layers 0-5; a fan-in component
  (be00000b/bf00000c/c000000d -> c1000001), layers 0-1; an island chain
  (c2c2c2c2 -> c3c3c3c3), layers 0-1; and aa000001, an epic that renders
  standalone. Nearly every non-epic issue is part_of the epic to exercise
  part_of in bulk. bd00000a and c000000d exercise done-node and
  `scheduled_before` cases respectively.

## Blocking semantics encoded

- `required_for` and `scheduled_before` from a NON-done issue block the
  target (visible in `pebbles next`).
- `required_for` from a done issue does not block (chain c1aaaaaa -> B,
  disconnected a4dddddd -> Y).
- `part_of` never blocks and never creates a flow edge (fan-out branches,
  mixed children); epics render standalone in `flow`.

## Status and type mix

Statuses: proposed, todo, in_progress, and done all appear; blocked is
derived from non-done incoming edges, never stored. Types used across the
stores: bug, ux_problem, usecase, design, validation, epic. Timestamps are
distinct per issue so load order (updated_at descending) is deterministic.
