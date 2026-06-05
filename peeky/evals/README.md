# Evals

LLM behavior evals for aegis. These measure routing accuracy, retrieval quality, and other model-driven decisions that aren't captured by unit tests.

## Why this directory exists

`cargo test` is for deterministic, fast, free checks of code correctness. Evals are stochastic, slow, and cost real API credits. Mixing them in `tests/` confuses lifecycle: tests run on every push, evals run on demand. Mixing them in `examples/` confuses purpose: examples are demos, evals are measurement.

Hence: `evals/` is its own thing. Runner code in `runners/`, case data in `cases/`, output in `results/` (gitignored).

## Layout

```
evals/
├── README.md          (this file)
├── cases/             input data, hand-edited JSON
│   └── tool_routing.json
├── runners/           runner code, one binary per eval
│   └── tool_routing.rs
└── results/           output (gitignored)
```

## Running

```
cargo run --release --bin eval_tool_routing
```

Pass a custom cases file:

```
cargo run --release --bin eval_tool_routing -- aegis/evals/cases/some_other.json
```

Each run is one Claude API call per case. Currently ~20 cases on Haiku, ~$0.02 per full run. Not free, not expensive.

## What each eval measures

| Eval | What it tests | Cases |
|---|---|---|
| `tool_routing` | Top-level intent classifier picks the correct path (FindAction / Memory / Chat / Integration / Agent) | `cases/tool_routing.json` |

More to come as the system grows (memory router store-vs-recall, integration router which-tool, agent loop trajectory eval, etc.).

## Adding a new eval

1. Drop case data into `cases/<name>.json` following the same schema as `tool_routing.json`.
2. Write the runner at `runners/<name>.rs`. Use `tool_routing.rs` as a template.
3. Add a `[[bin]]` entry to `aegis/Cargo.toml`:

   ```toml
   [[bin]]
   name = "eval_<name>"
   path = "evals/runners/<name>.rs"
   ```

4. Document it in the table above.

## Adding cases

Cases are JSON. Keep `id` unique. Pick a meaningful `category` for grouping in the summary. `notes` is optional but useful for adversarial cases that explain *why* the expected label is what it is.

```json
{
    "id": "memory_store_007",
    "transcript": "remember I parked on level 3",
    "expected_intent": "Memory",
    "category": "memory_store",
    "notes": "command form, not declarative"
}
```

Don't let `transcript` leak the expected label. "Use the memory tool to remember X" tests instruction-following, not routing.

## Conventions

- Runner code is Rust, same crate as aegis. No Python.
- Cases are versioned in git. Real data, not generated.
- Results are gitignored. If you want history, save runs externally or check in summaries only.
- Each eval is non-deterministic. Don't chase a single failure; track pass rates over multiple runs.
- An eval that's at 100% is suspicious. If everything passes, the cases are too easy.
