---
name: lily-draft
description: Draft a Lily document for a client from a document plan and review it in assist mode. Use when asked to draft or update a Will, Trust, POA, HIPAA, letter, or other Lily document, to fill a plan from a client record, or to review a plan against client data. Not for authoring or compiling templates; use lily-template-author for that.
---

# Lily Draft

The plan is the content. The client `.lily` is the data. The skeleton is the layout. The renderer is deterministic. This skill is the assist-mode loop around them: the agent proposes a draft, shows the data gaps and the diff, and the attorney approves before anything is saved as final.

## Inputs

- The plan path, usually `Templates/Plans/<Template>.plan.json` in the private template repository.
- The client directory containing the `.lily` file.
- The existing draft to compare against, when there is one.

## Loop

1. **Audit.** Run `scripts/lily plan audit --plan <plan> --client-dir <client> --json`. Read the used slots, the empty values, and the condition results. The audit marks a value `computed` when the renderer derives it from contacts or facts; it marks `missing` when no slot value exists.
2. **Resolve gaps.** A missing client value is a question for the attorney, not a guess. A missing prose slot such as distribution instructions or a special bequest may be drafted only from the client's own answers and must be flagged for review. A missing clause is a gap: propose it, do not invent legal text.
3. **Render.** Run `scripts/lily plan render --plan <plan> --client-dir <client> --output .tmp/draft/<name>.docx --json`. The render warnings are authoritative for required slots and unresolved references.
4. **Diff.** Run `scripts/lily docx diff <existing> .tmp/draft/<name>.docx`. Read every changed paragraph. A diff is either a data fill, a condition flip, a clause selection, or a flagged prose edit; anything else is a defect.
5. **Review bundle.** Return the candidate document, the audit, the diff, and a short flag list. Do not overwrite the client's document or mark the draft final.

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
# Audit a plan against a client
scripts/lily plan audit --plan <plan.json> --client-dir <client-dir> --json

# Render a candidate
scripts/lily plan render --plan <plan.json> --client-dir <client-dir> \
  --output .tmp/draft/<name>.docx --json

# Read or compare documents
scripts/lily docx text .tmp/draft/<name>.docx
scripts/lily docx diff <existing.docx> .tmp/draft/<name>.docx
```

The `plan render` command resolves the plan's `layout` path relative to the plan file when `--skeleton` is omitted. Pass `--skeleton` explicitly when the plan lives away from its template.

## Where This Connects

- Template authoring and compilation: the `lily-template-author` skill.
- The plan schema, clause model, and excerpt model: the Lily design note in the software wiki.
- The app review surface is still pending. Until it lands, this loop runs from the agent's shell and the attorney reviews the candidate document and diff.
