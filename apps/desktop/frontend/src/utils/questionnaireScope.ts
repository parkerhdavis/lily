// SPDX-License-Identifier: AGPL-3.0-or-later
import type { RequiredDocument } from "@/types";
import type { QuestionnaireSectionDef } from "@/types/questionnaire";

/** Whether a section or question applies to at least one selected document.
 *  A missing or empty list means it applies to every engagement. */
function appliesToSelection(
	documents: string[] | undefined,
	selected: Set<string>,
): boolean {
	if (!documents || documents.length === 0) return true;
	return documents.some((path) => selected.has(path));
}

/** Keep the sections and questions that serve the documents selected for this
 *  engagement. Sections left with no questions are dropped unless they render
 *  the contact list. */
export function scopedQuestionnaireSections(
	sections: QuestionnaireSectionDef[],
	requiredDocuments: RequiredDocument[] | undefined,
): QuestionnaireSectionDef[] {
	if (!requiredDocuments || requiredDocuments.length === 0) return sections;
	const selected = new Set(
		requiredDocuments.map((document) => document.template_rel_path),
	);
	const scoped: QuestionnaireSectionDef[] = [];
	for (const section of sections) {
		if (!appliesToSelection(section.documents, selected)) continue;
		if (section.kind === "contacts") {
			scoped.push(section);
			continue;
		}
		const questions = section.questions.filter((question) =>
			appliesToSelection(question.documents, selected),
		);
		if (questions.length > 0) {
			scoped.push({ ...section, questions });
		}
	}
	return scoped;
}
