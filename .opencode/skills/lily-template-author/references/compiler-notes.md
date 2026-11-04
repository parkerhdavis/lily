# Compiler Notes

These notes explain what `scripts/lily_compile.py` does with a template and what the reviewer should check. They are reference material for the `lily-template-author` skill.

## Conditional Mapping

The template library stores conditional definitions in document order. A conditional variable can have a `condition` (one definition) or `conditions` (one per occurrence). The compiler walks the document, counts occurrences of each conditional tag, and assigns the matching definition to each occurrence.

That ordinal mapping is the one piece of the old engine's behavior the compiler keeps. It happens once, at compile time. The generated plan has clause IDs such as `clause-has-spouse-3-true`, so later renders do not depend on the document order. A template edit requires a new compile and a review of the clause diff.

For each occurrence with a non-empty branch, the compiler emits a clause and a clause reference:

```json
{
  "clause": "clause-has-spouse-3-true",
  "when": { "eq": ["Has Spouse", true] }
}
```

A non-empty false branch gets a second clause with `false`. When both branches are empty, the occurrence is dropped.

## Casing

Simple slots preserve the SDT run's formatting marks. A name that the template renders in all caps through a `w:caps` property becomes a slot with `marks: ["caps"]`, not a literal uppercase value, so the text remains editable and the display matches the template.

Placeholders inside branch text use the placeholder's casing to set `format`:

- `{CLIENT FULL NAME}` becomes `format: "upper"`.
- `{Client Full Name}` stays as-is.
- `{client full name}` becomes `format: "lower"`.

The renderer looks up slot names case-insensitively, so `{PRIMARY HPOA AGENT FULL NAME}` resolves to the `Primary HPOA Agent Full Name` value and uppercases it.

## Marks

The compiler captures `bold`, `italic`, `underline`, `caps`, and `small_caps` from the SDT or run properties. Font family, size, color, spacing, and other direct formatting are not captured; the layout skeleton's paragraph and character styles supply them. If a template relies on direct formatting that the styles do not define, the rendered document will differ and the difference belongs in the review.

## Tables, Images, Cross-References

The compiler warns and skips tables and drawings. The renderer does not support them yet. Cross-references are not inferred; a template that hardcodes an article number keeps the literal text until the Trust work adds numbered block references.

## Acceptance Evidence

The HPOA template compiles to 85 blocks, 2 clauses, and 18 slots. With the `hpoa-couple-secondary` fixture, the compiled plan renders text identical to the deterministic snapshot, including the two spacing defects the template contains, because the compiler preserves the template rather than fixing it.

The Last Will template compiles to 72 blocks, 14 clauses, and 17 slots. With `will-couple-adult-children` and `will-single-no-children`, the compiled plan renders text identical to the deterministic snapshots, including the known `successor Personal Representative. of my estate` defect and the fixed `is/are` wording. With `will-couple-minor-children`, the only difference is the guardian sentence: the deterministic engine leaves `{PRIMARY GUARDIAN FULL NAME}` and `{SECONDARY GUARDIAN FULL NAME}` visible, while the plan renderer substitutes empty values because the client data model has no guardian roles. The Will migration must add that data or condition the guardian sentence out.

The Last Will compiler warns about one drawing. The text comparison is unaffected, but the rendered document may be missing a notary or signature graphic; confirm that during the Will migration.
