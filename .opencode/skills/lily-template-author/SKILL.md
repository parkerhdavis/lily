---
name: lily-template-author
description: Compile a Lily .docx template into a document plan, render it, and review the diff. Use when adding a template to the plan engine, recompiling after a template change, or debugging a plan that renders incorrectly. Not for hand-editing SDTs or variables inside a template; use docx-template for that.
---

# Lily Template Author

The plan engine replaces the SDT fill path. A template becomes a document plan: ordered blocks, inline content nodes, clauses, slots, and conditions. The compiler captures the template, a human or agent reviews the plan, and the renderer produces the document from the plan plus client data. The `.docx` template remains the layout skeleton, so its styles, headers, footers, and section properties survive.

## The Loop

1. **Compile** the template into a plan.
2. **Render** the plan with a representative client fixture.
3. **Compare** the rendered text against the deterministic corpus snapshot or the source template.
4. **Fix the plan**, not the XML, when the render is wrong. Rename blocks and clauses, adjust casing, split a clause, or add a term.
5. **Re-render and review** until the differences are deliberate.
6. **Wire the plan into the app** only after the review passes.

## Commands

```bash
# Compile a template into a plan
scripts/lily compile \
  --templates-dir ~/Carelaw/carelaw-lily/Templates \
  --template-rel-path "03 - Will Templates/Last Will and Testament Template.docx" \
  --output .working/plans/will.json --json

# Render the plan for a client fixture
scripts/lily plan render \
  --plan .working/plans/will.json \
  --client-dir .working/clients/will-couple \
  --skeleton ~/Carelaw/carelaw-lily/Templates/03\ -\ Will\ Templates/Last\ Will\ and\ Testament\ Template.docx \
  --output .working/plans/will.docx --json

# Read the rendered text
scripts/lily docx text .working/plans/will.docx

# Compare two rendered documents
scripts/lily docx diff .working/plans/deterministic.docx .working/plans/will.docx
```

The `plan` command resolves the plan's `layout` path relative to the plan file when `--skeleton` is omitted. Pass `--skeleton` explicitly when the plan lives away from its template.

The golden corpus snapshots are normalized text, so compare the rendered text against `~/Carelaw/carelaw-lily/Tests/snapshots/`. A deterministic `.docx` for the same fixture can be regenerated with `scripts/lily fill` or the corpus runner.

## What the Compiler Captures

- Paragraph order, paragraph styles, and empty spacing paragraphs.
- Simple `lily:` SDTs as slots, with the run's `caps`, bold, italic, underline, and small-caps properties preserved as marks.
- Conditional `lily-cond:` SDTs as true/false clauses, using the template library's branch templates.
- Nested `{Variable}` references inside branch text as slots with casing formats.
- Tabs and line breaks.
- Slot declarations with type, required flag, default, help text, and contact-role source.

## What It Does Not Do

- It does not infer defined terms. Add terms manually for documents such as the Trust Agreement.
- It does not capture tables or images yet. It warns and skips them.
- It does not infer article numbering or cross-references. Add `numbering` to headings and `number` and `ref` nodes in the plan.
- It does not edit headers or footers; the renderer replaces simple `lily:` slots there.
- It does not make legal judgments about clause boundaries or branch correctness. That is the review.

## Review Checklist

- Block order and paragraph styles match the template.
- Every `lily:` placeholder became a slot, and no raw `[[lily:...]]` text remains.
- Every conditional has the true and false clauses the library defines, with the right controlling variable.
- Uppercase branch placeholders carry `format: "upper"`; title-case ones do not.
- Run marks survive, especially `caps` on names that the template renders in small caps or all caps.
- Tabs and line breaks are present in signature and notary blocks.
- Compiler warnings are resolved or deliberately accepted.
- Known template defects are either preserved, so the migration diff is honest, or fixed in the plan with a note.

## Compiler Notes

- The compiler maps the Nth conditional SDT to the Nth entry in the library's `conditions` array. That mapping happens once, at compile time; the generated plan has stable clause IDs. Re-compile after a template edit and review the clause pairs rather than assuming the old mapping still holds.
- A non-empty false branch becomes a second clause with the opposite condition. Only one branch renders. Empty branches are omitted.
- Simple slots inherit the SDT run's marks. Branch placeholders use the placeholder casing to set `format`, matching the old engine's casing rule.
- The plan is the source of truth after compilation. Do not edit the generated `.docx` by hand and expect the plan to follow.

## References

- `references/compiler-notes.md` for the conditional mapping, casing rules, and the acceptance evidence from the HPOA and Will compiles.
- The design is at `~/Software/wiki-software/53 Lily/90-99 Agents/Transient/Lily Document Plan and Clause Library — Design.md`.
- The tool layer is documented in `scripts/README.md`.
- The compiler is `scripts/lily_compile.py`; the renderer is `apps/desktop/backend/src/plan.rs`.
