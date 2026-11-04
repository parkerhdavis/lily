// SPDX-License-Identifier: AGPL-3.0-or-later
export type DocumentStatus =
	| "not_started"
	| "drafting"
	| "reviewing"
	| "complete"
	| "executed";

export interface RequiredDocument {
	id: string;
	template_rel_path: string;
	status: DocumentStatus;
	document_filename: string | null;
	notes: string;
}

export interface ClientSummary {
	directory: string;
	client_name: string;
	total_documents: number;
	required_documents: RequiredDocumentSummary[];
	contacts_count: number;
	has_questionnaire: boolean;
}

export interface RequiredDocumentSummary {
	template_rel_path: string;
	status: DocumentStatus;
	document_filename: string | null;
}

export interface ClientTreeNode {
	name: string;
	path: string;
	is_client: boolean;
	client_summary: ClientSummary | null;
	children: ClientTreeNode[];
}

export interface PersistedNavEntry {
	step: string;
	working_dir: string | null;
	document_path: string | null;
	template_rel_path: string | null;
	label: string;
	visited_at: number;
}

export interface AppSettings {
	templates_dir: string | null;
	last_working_dir: string | null;
	recent_directories: string[];
	window_width: number | null;
	window_height: number | null;
	theme: string | null;
	zoom: number | null;
	footer_size: string | null;
	last_step: string | null;
	last_template_rel_path: string | null;
	autosave: boolean | null;
	questionnaires_dir: string | null;
	client_library_dirs: string[];
	navigation_history: PersistedNavEntry[];
}

export type WorkflowStep =
	| "hub"
	| "clients"
	| "questionnaire"
	| "select-template"
	| "edit-variables"
	| "app-settings"
	| "pipeline"
	| "questionnaire-editor"
	| "template-editor";

/** A single occurrence of text found in a template document. */
export interface TextOccurrence {
	index: number;
	context: string;
	paragraph_number: number;
}

/** A per-document override for a contact role. */
export interface RoleOverride {
	/** The contact ID for this override, or null for custom manual values. */
	contact_id: string | null;
	/** The specific variable values for this override. */
	values: Record<string, string>;
}

/** Metadata for a single document tracked in the .lily project file. */
export interface DocumentMeta {
	template_rel_path: string;
	created_at: string;
	modified_at: string;
	/** Display names of variables this document uses. Recorded when the
	 *  template is first processed so the variable list survives after
	 *  placeholders are replaced with real values in the docx. */
	variable_names: string[];
	/** Per-document role overrides (roles that diverge from the questionnaire). */
	role_overrides: Record<string, RoleOverride>;
	/** Per-document variable overrides (variables that diverge from client-level values). */
	variable_overrides: Record<string, string>;
}

/** Predefined relationship options for the contact dropdown. */
export const RELATIONSHIP_OPTIONS = [
	"Spouse",
	"Child",
	"Sibling",
	"Parent",
	"Niece/Nephew",
	"Aunt/Uncle",
	"Cousin",
	"Friend",
	"Other",
] as const;

/** A contact associated with a client (family member, agent, trustee, etc.). */
export interface Contact {
	id: string;
	full_name: string;
	first_name: string;
	middle_name: string;
	last_name: string;
	relationship: string;
	other_relationship: string;
	phone: string;
	email: string;
	address: string;
	city: string;
	state: string;
	zip: string;
	is_minor: boolean;
}

/** Maps a role (e.g., "Healthcare POA Agent") to a contact and defines which
 *  variables auto-fill from which contact properties. */
export interface ContactBinding {
	/** The contact ID bound to this role, or null for manual ("Other") entry. */
	contact_id: string | null;
	/** Map from variable display name to contact property key. */
	variable_mappings: Record<string, string>;
	/** For "contact-list" roles: the ordered contact IDs aggregated into one
	 *  variable. The single `variable_mappings` entry maps the target variable
	 *  → the contact property to join (with "; "). Absent for single-contact
	 *  bindings. */
	contact_ids?: string[];
}

/** The .lily project file stored in each client/working directory. */
export interface LilyFile {
	lily_version: number;
	/** Client-level variable values shared across all documents. */
	variables: Record<string, string>;
	/** Display names of conditional (ternary) variables that render as
	 *  toggles. */
	conditional_variables: string[];
	/** Map from document filename to its metadata. */
	documents: Record<string, DocumentMeta>;
	/** Contacts associated with this client. */
	contacts: Contact[];
	/** Contact-to-role bindings, keyed by role name. */
	contact_bindings: Record<string, ContactBinding>;
	/** Questionnaire notes keyed by section title. */
	questionnaire_notes: Record<string, SectionNotes>;
	/** ID of the questionnaire definition used for this client. */
	questionnaire_id: string | null;
	/** Version of the questionnaire definition when it was last applied. */
	questionnaire_version: number | null;
	/** Documents required for this client, with status tracking. */
	required_documents: RequiredDocument[];
	/** Non-persisted warnings from loading this file. */
	warnings: string[];
}

/** Result of `copy_from_spouse_lily`: the new resolved .lily plus
 *  per-document outcomes and any non-fatal warnings. */
export interface CopyFromSpouseResult {
	lily: LilyFile;
	copied_documents: string[];
	skipped_documents: string[];
	warnings: string[];
}

/** Notes attached to a questionnaire section. */
export interface SectionNotes {
	/** Notes from/for the client (visible in client-facing tools). */
	client: string;
	/** Internal notes for the legal team (not visible to clients). */
	internal: string;
}

/** Info about a single logical variable, with case-variant grouping. */
export interface VariableInfo {
	/** Display name shown in the UI (title-case preferred).
	 *  For conditional variables, this is the label portion (before `??`). */
	display_name: string;
	/** All distinct casings found in the document for this variable. */
	variants: string[];
	/** Whether this is a conditional (ternary) variable that renders as a
	 *  checkbox. Conditional variables use `{Label ?? true_text :: false_text}`
	 *  syntax and store `"true"` / `"false"` as their value. */
	is_conditional: boolean;
}

/** Type of a variable in a template schema. */
export type VariableType = "text" | "date" | "currency" | "conditional";

/** Defines the true/false branch logic for a conditional variable. */
export interface ConditionalDef {
	/** The variable whose value controls which branch is shown. */
	controlling_variable: string;
	/** Template text for the true branch, with {VarName} references. */
	true_template: string;
	/** Template text for the false branch (often empty). */
	false_template: string;
}

/** Schema definition for a single variable in a template. */
export interface VariableSchemaEntry {
	/** The type of this variable. */
	var_type: VariableType;
	/** Default value if not provided. */
	default?: string;
	/** Help text shown to the user. */
	help?: string;
	/** Date format string (for date variables, e.g., "MM/DD/YYYY"). */
	date_format?: string;
	/** Whether this field is required. */
	required: boolean;
	/** Conditional branch logic (only for var_type "conditional").
	 *  Single-occurrence conditionals use `condition`.
	 *  Multi-occurrence conditionals (same variable, different text at
	 *  different places) use `conditions` with one entry per occurrence. */
	condition?: ConditionalDef;
	/** Multiple conditional definitions for multi-occurrence conditionals. */
	conditions?: ConditionalDef[];
	/** Contact role this variable auto-fills from. */
	contact_role?: string;
	/** Contact property this variable maps to. */
	contact_property?: string;
}

/** Schema file for a template (.lily sidecar). */
export interface VariableSchema {
	lily_type: string;
	template_filename: string;
	/** Which fill engine the template uses. Missing means the SDT engine. */
	engine?: "sdt" | "plan";
	/** Path to the document plan, relative to the templates directory. */
	plan_path?: string;
	variables: Record<string, VariableSchemaEntry>;
}

/** A node in the template folder tree. */
export type TemplateTreeNode =
	| { kind: "folder"; name: string; children: TemplateTreeNode[] }
	| { kind: "file"; name: string; relPath: string };
