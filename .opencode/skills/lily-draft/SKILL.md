---
name: lily-draft
description: Draft a Lily document for a client from a document plan and review it in assist mode. Use when asked to draft or update a Will, Trust, POA, HIPAA, letter, or other Lily document, to fill a plan from a client record, or to review a plan against client data. Not for authoring or compiling templates; use lily-template-author for that.
---

# Lily Draft

The plan is the content. The client `.lily` is the data. The skeleton is the layout. The renderer is deterministic. This skill is the assist-mode loop around them: the agent proposes a draft, shows the data gaps and the diff, and the attorney approves before anything is saved as final.

## Inputs

- The plan path, usually `Templates/<purpose folder>/Plans/<Template>.plan.json` in the private template repository.
- The client directory containing the `.lily` file.
- The existing draft to compare against, when there is one.

## Loop

1. **Audit.** Run `scripts/lily draft --plan <plan> --client-dir <client> --output-dir .tmp/draft/<name> --previous <existing.docx> --json`. One command writes the candidate, `review.md`, `audit.json`, `render.json`, and `diff.txt`. Read `review.md` first; it carries the data gaps, condition results, render warnings, diff, and flags.
2. **Resolve gaps.** A missing client value is a question for the attorney, not a guess. A role with no assignment is a gap too: ask who fills it, or whether the role belongs in this document. A missing prose slot such as distribution instructions or a special bequest may be drafted only from the client's own answers and must be flagged for review. A missing clause is a gap: propose it, do not invent legal text.
3. **Inspect the individual steps** when the bundle needs a closer look:
   - `scripts/lily plan audit --plan <plan> --client-dir <client> --json`
   - `scripts/lily plan render --plan <plan> --client-dir <client> --output .tmp/draft/<name>.docx --json`
   - `scripts/lily docx diff <existing.docx> .tmp/draft/<name>.docx`
4. **Layout review.** Render the candidate to page images with `scripts/lily layout --docx .tmp/draft/<name>.docx --output-dir .tmp/draft/<name>-pages`, then read the pages in order. Flag a heading or signature line separated from what it belongs to, a signature or notary block split across pages, a table split at a row boundary, and a page ending on a single line where the template keeps a paragraph together. Record the page number and the block or paragraph name.
5. **Review bundle.** Return the candidate document, the audit, the diff, the layout findings, and a short flag list. Do not overwrite the client's document or mark the draft final.

## Layout Review

The renderer preserves the template's layout, but it does not decide page breaks. After the candidate renders, produce page images with `scripts/lily layout` and read them. The findings belong in the review bundle with page numbers and block names.

The plan carries `keep_with_next`, `page_break_before`, and `keep_together` on a block. After a layout review, set them on the block that needs the hint; the renderer emits `w:keepNext`, `w:pageBreakBefore`, and `w:keepLines`. Do not edit the template to force a break.

## App Assist Mode

When the Lily app invokes the agent, it passes the client directory, plan, document, and output directory in the prompt and sets `LILY_AGENT_OUT`, `LILY_AGENT_CLIENT`, `LILY_AGENT_PLAN`, `LILY_AGENT_DOCX`, and `LILY_AGENT_REPO`. In that mode the agent writes three files instead of editing the client:

- `$LILY_AGENT_OUT/proposal.lily`: a copy of the client `.lily` with only `variables` and `contact_bindings` changed.
- `$LILY_AGENT_OUT/flags.json`: a JSON array of objects with `severity` (`info`, `warning`, or `gap`), `message`, and an optional `path`.
- `$LILY_AGENT_OUT/summary.md`: a short explanation of every proposed change and flag.

The app renders the proposal as a candidate, shows the diff and flags, and the attorney approves before the client or the document changes. In this mode, never edit the client's `.lily`, the plan, or the document.

## Rules

- Never edit OOXML. The plan, the client data, and the approved clauses are the only inputs.
- Never invent legal text. Use approved clauses. If the plan cannot express the intent, record a gap with the proposed clause and a reason.
- Never fill a missing required value with a guess.
- Never flip a condition to fit a narrative. Conditions follow the client facts.
- Keep client data local. Draft outputs go under `.tmp/draft/` in the task's worktree, or `~/.opencode/.tmp/<task>/` when there is no worktree.
- The attorney's approval is the gate. Assist mode means propose, show, and wait.

## Review Checklist

- Every required slot has a value, and every optional empty value is deliberate.
- Every condition evaluates to a known result; no unknown fact silently falls to false.
- No unresolved cross-references or raw placeholders.
- The diff contains only intended data fills, condition flips, clause selections, or flagged prose.
- The clause versions are approved and pinned.
- The rendered document keeps the template's formatting. Spot-check the signature, notary, and any inserted paragraph.

## Commands

```bash
# One-command review bundle
scripts/lily draft --plan <plan.json> --client-dir <client-dir> \
  --output-dir .tmp/draft/<name> --previous <existing.docx> --json

# Layout pages
scripts/lily layout --docx .tmp/draft/<name>.docx \
  --output-dir .tmp/draft/<name>-pages

# Individual steps
scripts/lily plan audit --plan <plan.json> --client-dir <client-dir> --json
scripts/lily plan render --plan <plan.json> --client-dir <client-dir> \
  --output .tmp/draft/<name>.docx --json
scripts/lily docx text .tmp/draft/<name>.docx
scripts/lily docx diff <existing.docx> .tmp/draft/<name>.docx
```

The `plan render` command resolves the plan's `layout` path relative to the plan file when `--skeleton` is omitted. Pass `--skeleton` explicitly when the plan lives away from its template.

## Where This Connects

- Template authoring and compilation: the `lily-template-author` skill.
- The plan schema, clause model, and excerpt model: the Lily design note in the software wiki.
- The app's agent mode reads `proposal.lily`, `flags.json`, and `summary.md` and shows the candidate, diff, and flags for approval.
