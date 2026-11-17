// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { useToastStore } from "@/stores/toastStore";
import type { LilyFile } from "@/types";
import { pushNav } from "./helpers";
import type { WorkflowSlice } from "./types";

export const createProjectSlice: WorkflowSlice = (set, get) => ({
	setWorkingDir: (dir) => {
		pushNav(get());
		set({ workingDir: dir, step: "clients" });

		invoke<LilyFile>("load_lily_file_cmd", { workingDir: dir })
			.then((lilyFile) => {
				// Guard against race: if the user switched dirs while loading,
				// discard this stale result.
				if (get().workingDir !== dir) return;
				set({ lilyFile });
				for (const w of lilyFile.warnings ?? []) {
					useToastStore.getState().addToast("warning", w);
				}
				// Clients created before the selection spine existed have
				// documents but no required-document entries. Link them once;
				// once a selection exists, new documents are linked as they
				// are added.
				if ((lilyFile.required_documents ?? []).length === 0) {
					invoke<number>("backfill_required_documents", {
						workingDir: dir,
					})
						.then((changed) => {
							if (changed > 0 && get().workingDir === dir) {
								void get().reloadLilyFile();
							}
						})
						.catch((err) =>
							console.error("Failed to backfill required documents:", err),
						);
				}
			})
			.catch((err) => console.error("Failed to load .lily file:", err));
	},

	reloadLilyFile: async () => {
		const { workingDir } = get();
		if (!workingDir) return;

		try {
			const lilyFile = await invoke<LilyFile>("load_lily_file_cmd", {
				workingDir,
			});
			set({ lilyFile });
		} catch (err) {
			console.error("Failed to reload .lily file:", err);
		}
	},

	setWorkflowStage: async (stage) => {
		const { workingDir } = get();
		if (!workingDir) return;
		try {
			await invoke("set_workflow_stage", { workingDir, stage });
			await get().reloadLilyFile();
		} catch (err) {
			console.error("Failed to set workflow stage:", err);
			useToastStore
				.getState()
				.addToast("error", `Failed to update the engagement stage: ${err}`);
		}
	},

	setWorkflowWaitingOn: async (waitingOn) => {
		const { workingDir } = get();
		if (!workingDir) return;
		try {
			await invoke("set_workflow_waiting_on", { workingDir, waitingOn });
			await get().reloadLilyFile();
		} catch (err) {
			console.error("Failed to set workflow waiting state:", err);
			useToastStore
				.getState()
				.addToast("error", `Failed to update the waiting state: ${err}`);
		}
	},

	restoreNavigationEntry: async (entry) => {
		const current = get();
		const needsLilyReload =
			entry.workingDir !== current.workingDir && entry.workingDir;
		set({
			step: entry.step,
			workingDir: entry.workingDir,
		});

		if (needsLilyReload) {
			try {
				const lilyFile = await invoke<LilyFile>("load_lily_file_cmd", {
					workingDir: entry.workingDir,
				});
				set({ lilyFile });
			} catch (err) {
				console.error("Failed to reload .lily file:", err);
			}
		}
	},
});
