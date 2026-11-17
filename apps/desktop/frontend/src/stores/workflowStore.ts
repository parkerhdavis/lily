// SPDX-License-Identifier: AGPL-3.0-or-later
import { create } from "zustand";
import { createDocumentSlice } from "./workflowStore/documentSlice";
import { createNavigationSlice } from "./workflowStore/navigationSlice";
import { createProjectSlice } from "./workflowStore/projectSlice";
import type { WorkflowState } from "./workflowStore/types";

export type { WorkflowState } from "./workflowStore/types";

export const useWorkflowStore = create<WorkflowState>((...a) => {
	const [set, get] = a;
	return {
		// Initial state
		step: "hub",
		workingDir: null,
		templates: [],
		lilyFile: null,
		loading: false,
		error: null,

		// Compose slices — each provides a subset of actions
		...createNavigationSlice(set, get),
		...createDocumentSlice(set, get),
		...createProjectSlice(set, get),
	} as WorkflowState;
});
