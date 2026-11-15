// SPDX-License-Identifier: AGPL-3.0-or-later
import type { RequiredDocument } from "@/types";
import type { QuestionnaireSectionDef } from "@/types/questionnaire";

/** Keep the questionnaire sections that apply to at least one selected
 *  document. Sections without a document list are always shown. Before any
 *  documents are selected the full questionnaire is shown, so the attorney
 *  can still capture client data. */
export function scopedQuestionnaireSections(
	sections: QuestionnaireSectionDef[],
	requiredDocuments: RequiredDocument[] | undefined,
): QuestionnaireSectionDef[] {
	if (!requiredDocuments || requiredDocuments.length === 0) return sections;
	const selected = new Set(
		requiredDocuments.map((document) => document.template_rel_path),
	);
	return sections.filter((section) => {
		if (!section.documents || section.documents.length === 0) return true;
		return section.documents.some((path) => selected.has(path));
	});
}
