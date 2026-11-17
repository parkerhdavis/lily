// SPDX-License-Identifier: AGPL-3.0-or-later
import { useNavigationStore } from "@/stores/navigationStore";
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
		set({
			step: "hub",
			workingDir: null,
			templates: [],
			lilyFile: null,
			loading: false,
			error: null,
		});
	},
});
