// SPDX-License-Identifier: AGPL-3.0-or-later
/** A single question in the questionnaire. */
export type QuestionDef =
	| {
			kind: "text";
			/** The variable name this question populates. */
			variable: string;
			/** Human-readable label shown in the form. */
			label: string;
			/** Optional placeholder text. */
			placeholder?: string;
			/** If true, this field takes half width (for side-by-side layouts). */
			half?: boolean;
			/** If set, this field takes 1/3 width. */
			third?: boolean;
	  }
	| {
			kind: "conditional";
			/** The conditional variable name. */
			variable: string;
			label: string;
			/** Descriptive text for the "true" state. */
			trueLabel?: string;
			/** Descriptive text for the "false" state. */
			falseLabel?: string;
	  }
	| {
			kind: "contact-role";
			/** The role name (e.g., "Primary HPOA Agent"). Used as the key
			 *  in the contact_bindings map. */
			role: string;
			label: string;
			/** Map from variable display name to contact property key.
			 *  Defines which variables auto-fill from the selected contact. */
			variableMappings: Record<string, string>;
			/** Optional co-agent role name. When set, the UI offers a toggle
			 *  to assign a second person who serves alongside the primary
			 *  contact in the same role (e.g., co-agents for a POA). */
			coAgentRole?: string;
			/** Variable mappings for the co-agent, following the same
			 *  structure as `variableMappings`. */
			coAgentVariableMappings?: Record<string, string>;
			/** When true, the role holds an ordered list of assignments instead
			 *  of a single contact. Each assignment is one person or a shared
			 *  group, and the plan's role node chooses the legal wording. */
			multiple?: boolean;
			/** When true, only one shared assignment is allowed. Use this for
			 *  roles that act together, such as deed beneficiaries. */
			sharedOnly?: boolean;
	  }
	| {
			kind: "contact-list";
			/** The role name (e.g., "Additional HIPAA Releases"). Used as the key
			 *  in the contact_bindings map; the binding stores the selected
			 *  contact IDs. */
			role: string;
			label: string;
			/** The variable populated with the joined list of contact values
			 *  (each selected contact's `property`, joined with "; "). */
			listVariable: string;
			/** Contact property aggregated from each selected contact.
			 *  Defaults to "full_name". */
			property?: string;
	  }
	| {
			kind: "derived";
			/** The variable name this derived question produces. */
			variable: string;
			/** Human-readable label shown in the form. */
			label: string;
			/** Variable names that this value is computed from. */
			sources: string[];
			/** String used to join non-empty source values. Defaults to " ". */
			join?: string;
	  };

/** A section grouping related questions.
 *
 *  Sections with `kind: "contacts"` render an inline contact list instead
 *  of iterating over `questions` (which should be empty for those sections). */
export interface QuestionnaireSectionDef {
	title: string;
	description?: string;
	/** Section kind. Defaults to `"standard"` if omitted. */
	kind?: "standard" | "contacts";
	/** Which tab this section belongs to. */
	tab: "client-info" | "contacts" | "assignments";
	/** Template paths this section applies to. Omitted means the section is
	 *  shown for every engagement; an empty array means the same. */
	documents?: string[];
	questions: QuestionDef[];
}

/** The full questionnaire definition — an ordered list of sections. */
export type QuestionnaireDef = QuestionnaireSectionDef[];

// ─── Persisted questionnaire types ──────────────────────────────────────────

/** A complete questionnaire definition file stored on disk. */
export interface QuestionnaireDefFile {
	lily_type: string;
	id: string;
	name: string;
	version: number;
	tabs: { id: string; label: string }[];
	sections: QuestionnaireSectionDef[];
}

/** An entry in the questionnaire index (derived from scanning the directory). */
export interface QuestionnaireIndexEntry {
	id: string;
	name: string;
	version: number;
}

/** The questionnaire index — lists all definitions and which is active. */
export interface QuestionnaireIndex {
	questionnaires: QuestionnaireIndexEntry[];
}
