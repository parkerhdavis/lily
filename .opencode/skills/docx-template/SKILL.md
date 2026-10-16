---
name: docx-template
description: Edit Lily DOCX templates using SDT tooling scripts for reliable structured document tag manipulation, variable insertion/removal, schema sync, and validation. Use when the user asks to edit, create, modify, inspect, or validate a DOCX template; mentions SDTs, structured document tags, content controls, template variables, or .lily schema files; asks to add/remove variables from a template; or is working with files in a templates directory.
---

# Lily DOCX Template Editing

You have access to Python tooling scripts in `scripts/` that manipulate DOCX templates. These tools handle ID generation, namespace correctness, run property preservation, and structural integrity.

## Available Tools

All tools are in the `scripts/` directory of this repo. Run with `python3 scripts/<tool>.py`.

### `template_inspect.py` — Inspect a template (read-only)
```bash
python3 scripts/template_inspect.py <template.docx> [--json] [--schema]
```
Shows all SDTs, bookmarks, unconverted `{Variable}` placeholders, variable inventory, and IDs. Use `--schema` to also display the schema from the template library. Use `--json` for structured output.

**Always run this first** before making any changes to understand the current state.

### `template_validate.py` — Validate structural integrity
```bash
python3 scripts/template_validate.py <template.docx> [--json]
```
Checks for: malformed SDTs, duplicate IDs, unconverted placeholders, schema drift, conditional classification inconsistency. Returns exit code 1 if errors found.

**Always run this after making changes** to verify correctness.

### `template_insert_sdt.py` — Insert a variable SDT
```bash
python3 scripts/template_insert_sdt.py <template.docx> <search_text> <variable_name> [options]
```
Options:
- `--occurrence N` — replace only the Nth occurrence (0-based, per XML part)
- `--replace-all` — replace all occurrences across all parts
- `--conditional` — use `lily-cond:` prefix instead of `lily:`
- `--dry-run` — preview without writing

If multiple occurrences exist and neither `--occurrence` nor `--replace-all` is specified, the tool will list occurrences and ask you to choose.

### `template_remove_sdt.py` — Remove a variable SDT
```bash
python3 scripts/template_remove_sdt.py <template.docx> <variable_name> <replacement_text> [options]
```
Options:
- `--occurrence N` — remove only the Nth occurrence (0-based, per XML part)
- `--dry-run` — preview without writing

Replaces the SDT with a plain `<w:r>` run containing the replacement text, preserving run properties (bold, italic, etc.).

### `template_schema_sync.py` — Sync template library schema
```bash
python3 scripts/template_schema_sync.py <template.docx> [--dry-run] [--overwrite] [--json]
```
Generates or updates the template's entry in the centralized template-library `.lily` file from the template's current SDT/bookmark inventory. Preserves existing metadata (help text, required flags, conditional definitions) unless `--overwrite` is specified.

## Standard Workflow

When editing a template, always follow this sequence:

1. **Inspect** — `template_inspect.py` to understand current state
2. **Back up** — copy the `.docx` to a `.bak` before making changes
3. **Make changes** — use the appropriate approach (see below)
4. **Validate** — `template_validate.py` to check for structural issues
5. **Verify paragraph count** — confirm the modified template has the same paragraph count as the backup

## Choosing the Right Approach

### Simple changes: `template_insert_sdt.py` / `template_remove_sdt.py`

Use the insert/remove tools for straightforward, isolated changes:
- Adding a new variable to text that already exists in the template
- Removing a single variable and replacing with plain text
- Renaming a variable (remove old → insert new on the replacement text)

**Caution:** These tools use flat text search across the entire document. When combining remove + insert in sequence, the replacement text from a remove can match in unexpected places during a subsequent insert, potentially corrupting paragraph boundaries. Always verify paragraph counts after using these tools.

### Complex restructuring: Targeted XML migration scripts

For changes that involve restructuring paragraph content — adding multiple SDTs, splitting text runs around new SDTs, or modifying the text between SDTs — write a **targeted migration script** that:

1. Reads the DOCX via `docx_utils.read_docx_entries()`
2. Finds each SDT by its **unique tag string** (e.g., `'w:tag w:val="lily:Variable Name"'`)
3. Extracts the run properties (`<w:rPr>`) from the existing SDT
4. Builds replacement XML using `docx_utils.make_sdt_xml()` for new SDTs
5. Performs surgical string replacement at the exact position
6. Writes back via `docx_utils.write_docx()`

This approach is safer because it never searches by flat text — it finds SDTs by their unique tag attribute and replaces only that specific element. See `scripts/hpoa_migrate.py` for a reference implementation.

Key `docx_utils` functions for migration scripts:
- `read_docx_entries(path)` / `write_docx(path, entries)` — read/write ZIP entries
- `find_max_id_across_parts(entries)` — get next available SDT ID
- `make_sdt_xml(id, name, value, is_conditional, rpr)` — build a valid SDT element
- `escape_xml_text(text)` — escape text for XML content

## Template Library Schema

Schemas are stored in a **centralized template-library `.lily` file** (not per-template sidecars). The library file lives in the templates root directory and contains entries for all templates keyed by relative path:

```json
{
  "lily_type": "template-library",
  "templates": {
    "02 - Power of Attorney and Medical Templates/HPOA Template.docx": {
      "variables": {
        "Client Full Name": {
          "var_type": "text",
          "required": false
        },
        "Has Secondary HPOA Agent": {
          "var_type": "conditional",
          "required": false,
          "condition": {
            "controlling_variable": "Has Secondary HPOA Agent",
            "true_template": "If {PRIMARY HPOA AGENT FULL NAME} is unable...",
            "false_template": ""
          }
        }
      }
    }
  }
}
```

The Python tools automatically locate the library by walking up from the template directory. Variable types: `text` (default), `date`, `currency`, `conditional`.

**Important:** The conditional definitions (`condition` / `conditions`) in the library are what the Rust backend uses to resolve conditional SDTs at fill time. The template `.docx` only stores the SDT tag — the branch text lives in the library. Always update both the template and the library together.

## Lily SDT Format Reference

### SDT Structure
Lily uses Word Structured Document Tags (SDTs) as content controls:
```xml
<w:sdt>
  <w:sdtPr>
    <w:id w:val="{unique_id}"/>
    <w:tag w:val="lily:{Variable Name}"/>
    <w:alias w:val="{Variable Name}"/>
  </w:sdtPr>
  <w:sdtContent>
    <w:r>{run_properties}<w:t xml:space="preserve">{value}</w:t></w:r>
  </w:sdtContent>
</w:sdt>
```

### Tag Prefixes
- `lily:` — simple replacement variables
- `lily-cond:` — conditional (ternary) variables

### Empty Values → Bookmarks
When a variable's value is empty, SDTs convert to zero-width bookmarks:
```xml
<w:bookmarkStart w:id="{id}" w:name="lily:{Variable Name}"/>
<w:bookmarkEnd w:id="{id}"/>
```

### ID Rules
- IDs are shared between SDTs and bookmarks — they must not collide
- Always scan all XML parts (document.xml, header*.xml, footer*.xml) to find the max ID
- New IDs increment from max + 1

### Variable Types
- **Replacement**: `{Variable Name}` — direct text substitution, case-aware
- **Conditional**: `{Label ?? "true text" :: "false text"}` — ternary toggle, branch text stored in the template library schema
- **Contact-role**: `{Role.property}` — auto-fills from contact data (e.g., `{Primary HPOA Agent.full_name}`)

### Casing Rules
Applied both to top-level SDTs and to `{Variable}` references nested in conditional branch text:
- `{CLIENT FULL NAME}` + value "Jane Doe" → "JANE DOE"
- `{client full name}` + value "Jane Doe" → "jane doe"
- `{Client Full Name}` + value "Jane Doe" → "Jane Doe" (as-is)

### Nested Variables in Conditional Branch Text
Conditional branch text can include `{Variable Name}` references that get resolved at fill time. These are plain text references (not SDTs) — they use the casing rules above and support contact-role dot notation.

**Resolution behavior:**
- Variable exists in pool with a value → value is substituted (with casing applied)
- Variable exists in pool with empty value → resolves to empty string (invisible)
- Variable not in pool at all → `{Variable Name}` placeholder text is kept visible

This means auto-generated helper variables (like co-agent composites) can safely resolve to empty when not applicable — they won't leave `{Placeholder}` artifacts in the output.

## Composite Helper Variables Pattern

For complex scenarios where a slot can have one or two people (e.g., co-agents for POA), the backend auto-generates **composite helper variables** that work inside conditional branch text (where nested conditionals aren't supported).

Example: For a role "Primary HPOA Agent" with co-agent "Primary HPOA Co-Agent":

| Variable | With co-agent | Without co-agent |
|---|---|---|
| `Primary HPOA Co-Agent And Name` | `" and JENNIE DOE"` | `""` |
| `Primary HPOA Co-Agent And Phone` | `" and (555) 5678, respectively"` | `""` |
| `Primary HPOA Agent Verb` | `"are"` | `"is"` |
| `Primary HPOA Agent Title` | `"co-Healthcare Representatives. Either..."` | `"Healthcare Representative."` |

These are generated in `backend/src/lily_file.rs` in `resolve_contact_variables()` (Pass 4). The detection is convention-based: roles containing "Co-Agent" map to parent roles by replacing "Co-Agent" → "Agent", and "Co-Personal Representative" → "Personal Representative".

Template text becomes clean and uniform:
```
I appoint {PRIMARY HPOA AGENT FULL NAME}{Primary HPOA Co-Agent And Name},
who can be reached at {Primary HPOA Agent Phone}{Primary HPOA Co-Agent And Phone},
to serve as my {Primary HPOA Agent Title}
```

## Common Pitfalls

1. **Don't use remove+insert for complex restructuring** — the flat text search can match across paragraphs and corrupt document structure. Use targeted XML migration scripts instead.
2. **Never hardcode IDs** — the tools handle ID generation by scanning all parts.
3. **Word splits variables across runs** — `{Client` in one run, `Name}` in another. The tools handle normalization automatically.
4. **Smart quotes** — Word auto-converts `"` to `"` `"`. The tools normalize these.
5. **XML escaping order matters** — `&` must be escaped before `<`, `>`, etc. Use the tools.
6. **Schema and template must stay in sync** — conditional definitions live in the library schema, not in the template `.docx`. Update both together.
7. **Verify paragraph counts** — after any template edit, compare paragraph count against the original to catch structural corruption early.

## Parity with Rust Backend

These Python tools maintain exact parity with the Rust backend in `backend/src/docx_ops.rs`. The same SDT format, ID management, normalization, and escaping logic is used. If you discover a bug or add a feature to the Python tools, the Rust backend likely needs the same change (and vice versa).
