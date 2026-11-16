// SPDX-License-Identifier: AGPL-3.0-or-later
import type { NavigationEntry } from "@/stores/navigationStore";
import type {
	Contact,
	ContactBinding,
	LilyFile,
	RoleOverride,
	VariableInfo,
	VariableSchema,
	WorkflowStage,
	WorkflowStep,
	WorkflowWaitingOn,
} from "@/types";

export interface WorkflowState {
	step: WorkflowStep;
	workingDir: string | null;
	documentPath: string | null;
	documentHtml: string;
	variables: VariableInfo[];
	variableValues: Record<string, string>;
	templates: string[];
	templateRelPath: string | null;
	lilyFile: LilyFile | null;
	/** Variable schema from the template's sidecar .lily file. */
	templateSchema: VariableSchema | null;
	dirty: boolean;
	loading: boolean;
	error: string | null;

	// Navigation actions
	setStep: (step: WorkflowStep) => void;
	openQuestionnaire: () => void;
	startAddDocument: () => void;
	returnToHub: () => void;
	goToHub: () => void;
	goToSettings: () => void;
	goToPipeline: () => void;
	goToClients: () => void;
	goToQuestionnaireEditor: () => void;
	reset: () => void;

	// Document actions
	loadTemplates: (templatesDir: string) => Promise<void>;
	openDocument: (filename: string, templateRelPath: string) => Promise<void>;
	addMultipleDocuments: (
		templateRelPaths: string[],
		templatesDir: string,
	) => Promise<void>;
	setRequiredDocuments: (templateRelPaths: string[]) => Promise<void>;
	renameDocument: (newFilename: string) => Promise<void>;
	saveDocument: () => Promise<void>;
	restoreVersion: (snapshotPath: string) => Promise<void>;
	refreshPreview: () => Promise<void>;
	deleteDocument: (filename: string) => Promise<void>;
	newVersionDocument: (filename: string) => Promise<void>;
	openTemplateFile: (templateRelPath: string) => Promise<void>;

	// Variable actions
	updateVariable: (name: string, value: string) => void;
	saveClientVariable: (name: string, value: string) => Promise<void>;
	addClientVariable: (name: string) => Promise<void>;
	removeClientVariable: (name: string) => Promise<void>;

	// Contact actions
	addContact: (contact: Omit<Contact, "id">) => Promise<Contact>;
	updateContact: (contact: Contact) => Promise<void>;
	deleteContact: (contactId: string) => Promise<void>;
	setContactBinding: (role: string, binding: ContactBinding) => Promise<void>;
	clearContactBinding: (role: string) => Promise<void>;
	resolveContactBindings: () => Promise<void>;
	setRoleOverride: (
		role: string,
		overrideData: RoleOverride | null,
	) => Promise<void>;

	// Project actions
	setWorkingDir: (dir: string) => void;
	reloadLilyFile: () => Promise<void>;
	setWorkflowStage: (stage: WorkflowStage) => Promise<void>;
	setWorkflowWaitingOn: (waitingOn: WorkflowWaitingOn) => Promise<void>;
	saveQuestionnaireNote: (
		section: string,
		noteKind: "client" | "internal",
		value: string,
	) => Promise<void>;
	restoreNavigationEntry: (entry: NavigationEntry) => Promise<void>;
}

/** Zustand slice creator type for workflow store slices. */
export type WorkflowSlice = (
	set: (
		partial:
			| Partial<WorkflowState>
			| ((state: WorkflowState) => Partial<WorkflowState>),
	) => void,
	get: () => WorkflowState,
) => Partial<WorkflowState>;
