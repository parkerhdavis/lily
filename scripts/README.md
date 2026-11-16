# Lily Agent Tool Layer

`scripts/lily` is the command line over the client, template, document, and plan primitives that agent workflows need. It reads `.lily` client files and template libraries, inspects a `.docx` without the UI, extracts and diffs normalized document text, renders and audits document plans through `lily-render`, and runs the drafting agent. It prints JSON unless a command is explicitly a text diff.

## Commands

```bash
# Client data and a read-only derived summary
scripts/lily client show "/path/to/client/Client.lily"
scripts/lily client show "/path/to/client-folder"

# Every paragraph and SDT, including headers, footers, tables, and text boxes
scripts/lily template inspect "/path/to/Template.docx"

# A template's schema and plan path
scripts/lily template schema "/path/to/Templates" "03 - Will Templates/Last Will and Testament Template.docx"

# Normalized text and paragraph styles
scripts/lily docx text "/path/to/draft.docx"
scripts/lily docx text "/path/to/draft.docx" --json

# Compare two drafts by text and styles; exits 1 when they differ
scripts/lily docx diff "/path/to/left.docx" "/path/to/right.docx"

# Render and audit a document plan
scripts/lily plan render --plan "/path/to/plan.json" --client-dir "/path/to/client" \
  --output "/path/to/rendered.docx"
scripts/lily plan audit --plan "/path/to/plan.json" --client-dir "/path/to/client" --json

# Compile a template into a document plan
scripts/lily compile --templates-dir "/path/to/Templates" \
  --template-rel-path "03 - Will Templates/Last Will and Testament Template.docx" \
  --output "/path/to/plan.json" --json

# Produce a draft review bundle, or render pages for a layout check
scripts/lily draft --plan "/path/to/plan.json" --client-dir "/path/to/client" \
  --output-dir ".tmp/draft/review" --json
scripts/lily layout --docx "/path/to/draft.docx" --output-dir ".tmp/draft/pages"

# Check the environment
scripts/lily doctor
scripts/lily doctor --strict
```

## Environment

- `LILY_RENDER_BIN` overrides the `lily-render` path. The default is `target/debug/lily-render` at the repository root.
- `LILY_TEMPLATES` overrides the template library path. The default is `~/Carelaw/carelaw-lily/Templates`.

Build the renderer before using `plan render`, `draft`, or `doctor --strict`:

```bash
cd apps/desktop/backend
cargo build --bin lily-render
```

## Notes

- `template inspect` traverses the whole document tree, so it finds content inside footer text boxes that a body-only map would miss.
- `client show` derives spouse, children, minor-children, and role assignments for the agent's benefit. It does not write the resolved values back to the `.lily` file; that remains the app's job.
- The templates live in the private `carelaw-lily` repository because they carry the firm's legal text. The CLI is safe to run against synthetic fixtures; do not point it at a real client folder while testing.
- The old SDT tooling and corpus were retired. Their archive lives at `Archive/SDT Tooling/` and in the Carelaw repository's `Archive/SDT Golden Corpus/`.
