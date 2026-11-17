// SPDX-License-Identifier: AGPL-3.0-or-later
import type { NavigationEntry } from "@/stores/navigationStore";
import type {
	LilyFile,
	WorkflowStage,
	WorkflowStep,
	WorkflowWaitingOn,
} from "@/types";

export interface WorkflowState {
	step: WorkflowStep;
	workingDir: string | null;
	templates: string[];
	lilyFile: LilyFile | null;
	loading: boolean;
	error: string | null;

	// Navigation actions
	setStep: (step: WorkflowStep) => void;
	startAddDocument: () => void;
	returnToHub: () => void;
	goToHub: () => void;
	goToSettings: () => void;
	goToPipeline: () => void;
	goToClients: () => void;
	reset: () => void;

	// Document actions
	loadTemplates: (templatesDir: string) => Promise<void>;
	openDocument: (filename: string, templateRelPath: string) => Promise<void>;
	addMultipleDocuments: (
		templateRelPaths: string[],
		templatesDir: string,
	) => Promise<void>;
	setRequiredDocuments: (templateRelPaths: string[]) => Promise<void>;
	deleteDocument: (filename: string) => Promise<void>;
	newVersionDocument: (filename: string) => Promise<void>;
	openTemplateFile: (templateRelPath: string) => Promise<void>;

	// Project actions
	setWorkingDir: (dir: string) => void;
	reloadLilyFile: () => Promise<void>;
	setWorkflowStage: (stage: WorkflowStage) => Promise<void>;
	setWorkflowWaitingOn: (waitingOn: WorkflowWaitingOn) => Promise<void>;
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
