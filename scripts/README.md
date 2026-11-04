# Lily Agent Tool Layer

`scripts/lily` is the command line over the client, template, and document primitives that agent workflows need. It reads `.lily` client files and template libraries, inspects a `.docx` without the UI, extracts and diffs normalized document text, and forwards to the headless `lily-fill` binary. The plan, clause, render, and validate commands will extend this CLI when the plan renderer lands.

The CLI is read-only except for `fill`, which writes the filled document through the existing engine. It prints JSON unless a command is explicitly a text diff.

## Commands

```bash
# Client data and a read-only derived summary
scripts/lily client show "/path/to/client/Client.lily"
scripts/lily client show "/path/to/client-folder"

# Every paragraph and SDT, including headers, footers, tables, and text boxes
scripts/lily template inspect "/path/to/Template.docx"

# A template's schema and conditional definitions
scripts/lily template schema "/path/to/Templates" "03 - Will Templates/Last Will and Testament Template.docx"
scripts/lily template schema "/path/to/Templates" "03 - Will Templates/Last Will and Testament Template.docx" --conditions-only

# Normalized text and paragraph styles
scripts/lily docx text "/path/to/draft.docx"
scripts/lily docx text "/path/to/draft.docx" --json

# Compare two drafts by text and styles; exits 1 when they differ
scripts/lily docx diff "/path/to/left.docx" "/path/to/right.docx"

# Render a document plan through the deterministic renderer
scripts/lily plan render --plan "/path/to/plan.json" --client-dir "/path/to/client" \
  --output "/path/to/rendered.docx"
scripts/lily plan render --plan "/path/to/plan.json" --client-dir "/path/to/client" \
  --skeleton "/path/to/skeleton.docx" --output "/path/to/rendered.docx" --json

# Compile a template into a document plan
scripts/lily compile --templates-dir "/path/to/Templates" \
  --template-rel-path "03 - Will Templates/Last Will and Testament Template.docx" \
  --output "/path/to/plan.json" --json

# Forward to the headless fill harness
scripts/lily fill --client-dir "/path/to/client" --docx "Template.docx" \
  --templates-dir "/path/to/Templates" \
  --template-rel-path "03 - Will Templates/Last Will and Testament Template.docx" \
  --output "/path/to/filled.docx"

# Check the environment
scripts/lily doctor
scripts/lily doctor --strict
```

## Environment

- `LILY_FILL_BIN` overrides the `lily-fill` path. The default is `target/debug/lily-fill` at the repository root.
- `LILY_RENDER_BIN` overrides the `lily-render` path. The default is `target/debug/lily-render` at the repository root.
- `LILY_CORPUS` overrides the golden corpus path. The default is `~/Carelaw/carelaw-lily/Tests`.
- `LILY_TEMPLATES` overrides the template library path. The default is `~/Carelaw/carelaw-lily/Templates`.

Build the binaries before using `fill`, `plan render`, or `doctor --strict`:

```bash
cd apps/desktop/backend
cargo build --bin lily-fill
cargo build --bin lily-render
```

## Notes

- `template inspect` traverses the whole document tree, so it finds SDTs inside footer text boxes that a body-only map would miss. That is the gap that produced the HPOA spike's footer false positive.
- `client show` derives spouse, children, minor-children, and role assignments for the agent's benefit. It does not write the resolved values back to the `.lily` file; that remains the app's job.
- The corpus and templates live in the private `carelaw-lily` repository because they carry the firm's legal text. The CLI is safe to run against synthetic fixtures; do not point it at a real client folder while testing.
