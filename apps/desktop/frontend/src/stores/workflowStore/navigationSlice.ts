// SPDX-License-Identifier: AGPL-3.0-or-later
import { useNavigationStore } from "@/stores/navigationStore";
import { useUndoStore } from "@/stores/undoStore";
import { pushNav } from "./helpers";
import type { WorkflowSlice } from "./types";

export const createNavigationSlice: WorkflowSlice = (set, get) => ({
	setStep: (step) => {
		pushNav(get());
		set({ step });
	},

	startAddDocument: () => {
		pushNav(get());
		set({ step: "select-template" });
	},

	returnToHub: () => {
		pushNav(get());
		set({
			step: "clients",
			documentPath: null,
			documentHtml: "",
			variables: [],
			variableValues: {},
			templateRelPath: null,
			templateSchema: null,
			dirty: false,
			error: null,
		});
		get().reloadLilyFile();
	},

	goToHub: () => {
		pushNav(get());
		set({ step: "hub" });
	},
	goToSettings: () => {
		pushNav(get());
		set({ step: "app-settings" });
	},
	goToClients: () => {
		pushNav(get());
		set({ step: "clients" });
	},
	goToPipeline: () => {
		pushNav(get());
		set({ step: "pipeline" });
	},
	reset: () => {
		useNavigationStore.getState().clear();
		useUndoStore.getState().clear();
		set({
			step: "hub",
			workingDir: null,
			documentPath: null,
			documentHtml: "",
			variables: [],
			variableValues: {},
			templates: [],
			templateRelPath: null,
			templateSchema: null,
			lilyFile: null,
			dirty: false,
			loading: false,
			error: null,
		});
	},
});
