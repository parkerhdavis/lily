// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { useToastStore } from "@/stores/toastStore";
import { useUndoStore } from "@/stores/undoStore";
import type { LilyFile, VariableInfo } from "@/types";
import { extractFilename } from "@/utils/path";
import { mergeStoredVariables, pushNav } from "./helpers";
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

	saveQuestionnaireNote: async (section, noteKind, value) => {
		const { workingDir } = get();
		if (!workingDir) return;

		const oldValue =
			get().lilyFile?.questionnaire_notes?.[section]?.[noteKind] ?? "";
		await invoke("save_questionnaire_note", {
			workingDir,
			section,
			noteKind,
			value,
		});
		const { lilyFile } = get();
		if (lilyFile) {
			const notes = { ...(lilyFile.questionnaire_notes ?? {}) };
			const sectionNotes = notes[section] ?? {
				client: "",
				internal: "",
			};
			notes[section] = { ...sectionNotes, [noteKind]: value };
			set({
				lilyFile: { ...lilyFile, questionnaire_notes: notes },
			});
		}
		useUndoStore.getState().push({
			description: `Change ${noteKind} note for ${section}`,
			timestamp: Date.now(),
			redo: async () => {
				await invoke("save_questionnaire_note", {
					workingDir,
					section,
					noteKind,
					value,
				});
				const { lilyFile: lf } = get();
				if (lf) {
					const notes = {
						...(lf.questionnaire_notes ?? {}),
					};
					const sn = notes[section] ?? {
						client: "",
						internal: "",
					};
					notes[section] = { ...sn, [noteKind]: value };
					set({
						lilyFile: {
							...lf,
							questionnaire_notes: notes,
						},
					});
				}
			},
			undo: async () => {
				await invoke("save_questionnaire_note", {
					workingDir,
					section,
					noteKind,
					value: oldValue,
				});
				const { lilyFile: lf } = get();
				if (lf) {
					const notes = {
						...(lf.questionnaire_notes ?? {}),
					};
					const sn = notes[section] ?? {
						client: "",
						internal: "",
					};
					notes[section] = { ...sn, [noteKind]: oldValue };
					set({
						lilyFile: {
							...lf,
							questionnaire_notes: notes,
						},
					});
				}
			},
		});
	},

	restoreNavigationEntry: async (entry) => {
		// For template-editor, delegate to openTemplateEditor which handles
		// backup creation, HTML loading, and variable extraction.
		if (entry.step === "template-editor" && entry.templateRelPath) {
			const templatesDir = (
				await import("@/stores/settingsStore")
			).useSettingsStore.getState().settings.templates_dir;
			if (templatesDir) {
				await get().openTemplateEditor(entry.templateRelPath, templatesDir);
				return;
			}
		}

		const current = get();
		const needsLilyReload =
			entry.workingDir !== current.workingDir && entry.workingDir;
		const needsDocReload =
			entry.documentPath && entry.documentPath !== current.documentPath;

		set({
			step: entry.step,
			workingDir: entry.workingDir,
			documentPath: entry.documentPath,
			templateRelPath: entry.templateRelPath,
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

		if (needsDocReload && entry.step === "edit-variables") {
			try {
				const documentHtml = await invoke<string>("get_document_html", {
					docxPath: entry.documentPath,
				});
				let variables = await invoke<VariableInfo[]>("extract_variables", {
					docxPath: entry.documentPath,
				});
				const { lilyFile } = get();

				// Merge with stored names to restore nested variables inside
				// false conditionals, preserving document order and contact-role variants.
				const filename = entry.documentPath
					? extractFilename(entry.documentPath)
					: "";
				variables = mergeStoredVariables(variables, filename, lilyFile);

				const conditionalSet = new Set(lilyFile?.conditional_variables ?? []);
				if (conditionalSet.size > 0) {
					variables = variables.map((v) =>
						conditionalSet.has(v.display_name)
							? { ...v, is_conditional: true }
							: v,
					);
				}
				const savedVars = lilyFile?.variables ?? {};
				const variableValues: Record<string, string> = {};
				for (const v of variables) {
					const defaultVal = v.is_conditional ? "false" : "";
					variableValues[v.display_name] =
						savedVars[v.display_name] ?? defaultVal;
				}

				// Apply per-document role overrides
				const docMeta = lilyFile?.documents[filename];
				if (docMeta?.role_overrides) {
					for (const ro of Object.values(docMeta.role_overrides)) {
						for (const [varName, value] of Object.entries(ro.values)) {
							variableValues[varName] = value;
						}
					}
				}
				// Apply per-document variable overrides
				if (docMeta?.variable_overrides) {
					for (const [varName, value] of Object.entries(
						docMeta.variable_overrides,
					)) {
						variableValues[varName] = value;
					}
				}

				set({ documentHtml, variables, variableValues, dirty: false });
			} catch (err) {
				console.error("Failed to reload document:", err);
			}
		}
	},
});
