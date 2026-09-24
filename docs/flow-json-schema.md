# Flow JSON schema

`pebbles flow --format json -o flow.json` writes the flow layout as JSON for
programmatic consumers (web frontends, analysis tools). The document is one
object with three keys: `nodes`, `edges`, and `layout`.

## nodes

One object per issue, in issue order:

| field | type | meaning |
| ----- | ---- | ------- |
| `id` | string | Full UUID of the issue |
| `short_id` | string | 8-character id prefix used in issue filenames |
| `title` | string | Issue title |
| `type` | string | Issue type slug (`bug`, `ux_problem`, `usecase`, `design`, `validation`, `epic`, `milestone`) |
| `status` | string | Status slug (`proposed`, `todo`, `in_progress`, `done`, `blocked`) |
| `x` | number | Layer coordinate in layout units; `0.0` is the first layer |
| `y` | number | Position within the layer in layout units |
| `in_critical_path` | boolean | Present only when `--goal` is given; `false` for every node (the critical-chain computation is the separate loan e4937f0b) |

## edges

One object per dependency between issues:

| field | type | meaning |
| ----- | ---- | ------- |
| `source` | string | Full UUID of the depending issue |
| `target` | string | Full UUID of the dependency |
| `type` | string | Relationship type: `required_for` or `scheduled_before`. `part_of` never appears in the flow graph |

## layout

| field | type | meaning |
| ----- | ---- | ------- |
| `width` | number | Layout bounds width in layout units |
| `height` | number | Layout bounds height in layout units |
| `layers` | integer | Number of layers in the dependency layering |

## Serialization

Deterministic: 2-space indent, stable key order, no timestamps. Identical
inputs produce byte-identical output.
