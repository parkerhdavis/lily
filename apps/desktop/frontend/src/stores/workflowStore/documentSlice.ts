// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { useSettingsStore } from "@/stores/settingsStore";
import { useUndoStore } from "@/stores/undoStore";
import type { LilyFile, VariableSchema } from "@/types";
import { extractFilename } from "@/utils/path";
import {
	buildDocumentFilename,
	loadTemplateVariables,
	mergeStoredVariables,
	pushNav,
	resolveContactVariables,
	toastError,
	toastSuccess,
} from "./helpers";
import type { WorkflowSlice } from "./types";

function planPathFor(
	templatesDir: string,
	schema: VariableSchema,
): string | null {
	if (schema.engine !== "plan" || !schema.plan_path) return null;
	return `${templatesDir}/${schema.plan_path}`;
}

/** Render a plan-driven document from the client data. */
async function applyDocumentValues(
	templatesDir: string,
	templateRelPath: string,
	docxPath: string,
	templateSchema: VariableSchema | null,
): Promise<void> {
	const planPath = templateSchema
		? planPathFor(templatesDir, templateSchema)
		: null;
	if (!planPath) {
		throw new Error("Template is not plan-driven");
	}
	await invoke("snapshot_document", { docxPath });
	await invoke("render_plan_document", {
		docxPath,
		planPath,
		skeletonPath: `${templatesDir}/${templateRelPath}`,
	});
}

export const createDocumentSlice: WorkflowSlice = (set, get) => ({
	loadTemplates: async (templatesDir) => {
		set({ loading: true, error: null });
		try {
			const templates = await invoke<string[]>("list_templates", {
				templatesDir,
			});
			set({ templates, loading: false });
		} catch (err) {
			set({ error: String(err), loading: false });
			toastError("Failed to load templates", err);
		}
	},

	setRequiredDocuments: async (templateRelPaths) => {
		const { workingDir, lilyFile } = get();
		if (!workingDir) return;

		const required = lilyFile?.required_documents ?? [];
		const desired = new Set(templateRelPaths);
		for (const document of required) {
			if (
				!desired.has(document.template_rel_path) &&
				!document.document_filename
			) {
				await invoke("remove_required_document", {
					workingDir,
					documentId: document.id,
				});
			}
		}
		const existing = new Set(
			required.map((document) => document.template_rel_path),
		);
		for (const templateRelPath of templateRelPaths) {
			if (!existing.has(templateRelPath)) {
				await invoke("add_required_document", {
					workingDir,
					templateRelPath,
					notes: "",
				});
			}
		}
		await get().reloadLilyFile();
	},

	addMultipleDocuments: async (templateRelPaths, templatesDir) => {
		const { workingDir, lilyFile } = get();
		if (!workingDir) return;

		set({ loading: true, error: null });
		try {
			// Track used filenames to avoid collisions within the batch
			const usedFilenames = new Set<string>(
				Object.keys(lilyFile?.documents ?? {}),
			);
			const addedDocs: { docPath: string; templateRelPath: string }[] = [];

			for (const templateRelPath of templateRelPaths) {
				const fullTemplatePath = `${templatesDir}/${templateRelPath}`;
				let filename = buildDocumentFilename(templateRelPath, lilyFile);

				// De-duplicate filename
				let candidate = filename;
				let counter = 2;
				while (usedFilenames.has(candidate)) {
					const base = filename.replace(/\.docx$/i, "");
					candidate = `${base} (${counter}).docx`;
					counter++;
				}
				filename = candidate;
				usedFilenames.add(filename);

				const docPath = await invoke<string>("copy_template", {
					templatePath: fullTemplatePath,
					destDir: workingDir,
					filename,
					templateRelPath,
				});

				const variables = await loadTemplateVariables(
					templatesDir,
					templateRelPath,
				);

				await invoke("set_document_variables", {
					workingDir,
					filename,
					variableNames: variables.map((v) => v.display_name),
					conditionalNames: variables
						.filter((v) => v.is_conditional)
						.map((v) => v.display_name),
					conditionalDefinitions: {},
				});

				// The selection spine: every drafted document is also a
				// required-document entry, linked to the file it produced.
				await invoke("ensure_required_document", {
					workingDir,
					templateRelPath,
					documentFilename: filename,
					status: "drafting",
				});

				addedDocs.push({ docPath, templateRelPath });
			}

			// Resolve contact variables so new relationship-based
			// conditionals pick up values immediately.
			await resolveContactVariables(workingDir);

			// Reload once at the end
			const updatedLilyFile = await invoke<LilyFile>("load_lily_file_cmd", {
				workingDir,
			});
			for (const { docPath, templateRelPath } of addedDocs) {
				let templateSchema: VariableSchema | null = null;
				try {
					templateSchema = await invoke<VariableSchema>(
						"load_template_schema",
						{
							templatesDir,
							templateRelPath,
						},
					);
				} catch {
					// Schema is optional
				}
				await applyDocumentValues(
					templatesDir,
					templateRelPath,
					docPath,
					templateSchema,
				);
			}

			pushNav(get());
			set({
				lilyFile: updatedLilyFile,
				step: "clients",
				loading: false,
			});
			toastSuccess(
				`Added ${templateRelPaths.length} document${templateRelPaths.length !== 1 ? "s" : ""}`,
			);
		} catch (err) {
			set({ error: String(err), loading: false });
			toastError("Failed to add documents", err);
		}
	},

	openDocument: async (filename, templateRelPath) => {
		const { workingDir, lilyFile } = get();
		if (!workingDir) return;

		set({ loading: true, error: null });
		try {
			const docPath = `${workingDir}/${filename}`;

			const templatesDir = useSettingsStore.getState().settings.templates_dir;
			const docTemplateRelPath =
				templateRelPath ?? lilyFile?.documents[filename]?.template_rel_path;
			let variables =
				templatesDir && docTemplateRelPath
					? await loadTemplateVariables(templatesDir, docTemplateRelPath)
					: [];

			// Merge with stored names to restore nested variables inside
			// false conditionals, preserving document order and contact-role variants.
			variables = mergeStoredVariables(variables, filename, lilyFile);

			const conditionalSet = new Set(lilyFile?.conditional_variables ?? []);
			if (conditionalSet.size > 0) {
				variables = variables.map((v) =>
					conditionalSet.has(v.display_name)
						? { ...v, is_conditional: true }
						: v,
				);
			}

			// Record variable names from this document and resolve
			// contact variables so derived values are up-to-date.
			await invoke("set_document_variables", {
				workingDir,
				filename,
				variableNames: variables.map((v) => v.display_name),
				conditionalNames: variables
					.filter((v) => v.is_conditional)
					.map((v) => v.display_name),
				conditionalDefinitions: {},
			});
			await resolveContactVariables(workingDir);

			// Reload .lily to pick up resolved values
			const updatedLilyFile = await invoke<LilyFile>("load_lily_file_cmd", {
				workingDir,
			});

			const savedVars = updatedLilyFile?.variables ?? {};
			const variableValues: Record<string, string> = {};
			for (const v of variables) {
				const defaultVal = v.is_conditional ? "false" : "";
				variableValues[v.display_name] =
					savedVars[v.display_name] ?? defaultVal;
			}

			const docMeta = updatedLilyFile?.documents[filename];
			if (docMeta?.role_overrides) {
				for (const override of Object.values(docMeta.role_overrides)) {
					for (const [varName, value] of Object.entries(override.values)) {
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

			// Sync current values into the .docx so it reflects the
			// latest questionnaire data without requiring a manual save
			const settings = useSettingsStore.getState().settings;
			let templateSchema: VariableSchema | null = null;
			if (settings.templates_dir && docTemplateRelPath) {
				try {
					templateSchema = await invoke<VariableSchema>(
						"load_template_schema",
						{
							templatesDir: settings.templates_dir,
							templateRelPath: docTemplateRelPath,
						},
					);
				} catch {
					// Schema is optional
				}
				await applyDocumentValues(
					settings.templates_dir,
					docTemplateRelPath,
					docPath,
					templateSchema,
				);
			}

			const documentHtml = await invoke<string>("get_document_html", {
				docxPath: docPath,
			});

			pushNav(get());
			set({
				documentPath: docPath,
				templateRelPath,
				variables,
				variableValues,
				documentHtml,
				lilyFile: updatedLilyFile,
				templateSchema,
				dirty: false,
				step: "edit-variables",
				loading: false,
			});
		} catch (err) {
			set({ error: String(err), loading: false });
			toastError("Failed to open document", err);
		}
	},

	renameDocument: async (newFilename) => {
		const { documentPath } = get();
		if (!documentPath) return;

		const oldFilename = extractFilename(documentPath);
		set({ loading: true, error: null });
		try {
			const newPath = await invoke<string>("rename_document", {
				docxPath: documentPath,
				newFilename,
			});
			set({ documentPath: newPath, loading: false });
			useUndoStore.getState().push({
				description: `Rename to ${newFilename}`,
				timestamp: Date.now(),
				redo: async () => {
					const { documentPath: dp } = get();
					if (!dp) return;
					const np = await invoke<string>("rename_document", {
						docxPath: dp,
						newFilename,
					});
					set({ documentPath: np });
				},
				undo: async () => {
					const { documentPath: dp } = get();
					if (!dp) return;
					const np = await invoke<string>("rename_document", {
						docxPath: dp,
						newFilename: oldFilename,
					});
					set({ documentPath: np });
				},
			});
		} catch (err) {
			set({ error: String(err), loading: false });
			toastError("Failed to rename document", err);
		}
	},

	saveDocument: async () => {
		const {
			documentPath,
			variableValues,
			workingDir,
			lilyFile,
			templateRelPath,
		} = get();
		if (!documentPath) return;

		set({ loading: true, error: null });
		try {
			let templateSchema: VariableSchema | null = null;
			const settings = useSettingsStore.getState().settings;
			const docFilename = extractFilename(documentPath);
			const docTemplateRelPath =
				templateRelPath ?? lilyFile?.documents[docFilename]?.template_rel_path;
			if (settings.templates_dir && docTemplateRelPath) {
				try {
					templateSchema = await invoke<VariableSchema>(
						"load_template_schema",
						{
							templatesDir: settings.templates_dir,
							templateRelPath: docTemplateRelPath,
						},
					);
				} catch {
					// Schema is optional
				}
			}

			const filename = extractFilename(documentPath);
			const docMeta = lilyFile?.documents[filename];
			if (docMeta?.role_overrides && workingDir) {
				for (const [role, override] of Object.entries(docMeta.role_overrides)) {
					const updatedValues = { ...override.values };
					for (const varName of Object.keys(updatedValues)) {
						if (variableValues[varName] !== undefined) {
							updatedValues[varName] = variableValues[varName];
						}
					}
					await invoke("set_role_override", {
						workingDir,
						filename,
						role,
						overrideData: {
							contact_id: override.contact_id,
							values: updatedValues,
						},
					});
				}
			}

			// Plan templates read from the .lily, so persist the current
			// document values before rendering.
			if (templateSchema?.engine === "plan" && workingDir) {
				await invoke("save_client_variables", {
					workingDir,
					variableValues,
				});
			}

			if (!settings.templates_dir) {
				throw new Error("Templates directory is not configured");
			}
			await applyDocumentValues(
				settings.templates_dir,
				docTemplateRelPath ?? "",
				documentPath,
				templateSchema,
			);

			if (workingDir) {
				const lilyFile = await invoke<LilyFile>("load_lily_file_cmd", {
					workingDir,
				});
				set({ lilyFile });
			}

			set({ loading: false, dirty: false });
			toastSuccess("Document saved");
		} catch (err) {
			set({ error: String(err), loading: false });
			toastError("Failed to save document", err);
		}
	},

	refreshPreview: async () => {
		const { documentPath } = get();
		if (!documentPath) return;

		try {
			const documentHtml = await invoke<string>("get_document_html", {
				docxPath: documentPath,
			});
			set({ documentHtml });
		} catch (err) {
			console.error("Failed to refresh preview:", err);
		}
	},

	restoreVersion: async (snapshotPath) => {
		const { documentPath } = get();
		if (!documentPath) return;
		await invoke("restore_document_version", {
			docxPath: documentPath,
			snapshotPath,
		});
		const documentHtml = await invoke<string>("get_document_html", {
			docxPath: documentPath,
		});
		set({ documentHtml, dirty: false });
	},

	deleteDocument: async (filename) => {
		const { workingDir } = get();
		if (!workingDir) return;

		try {
			await invoke("delete_document", { workingDir, filename });
			await get().reloadLilyFile();
		} catch (err) {
			console.error("Failed to delete document:", err);
			toastError("Failed to delete document", err);
		}
	},

	newVersionDocument: async (filename) => {
		const { workingDir } = get();
		if (!workingDir) return;

		try {
			await invoke<string>("new_version_document", {
				workingDir,
				filename,
			});
			await get().reloadLilyFile();
		} catch (err) {
			console.error("Failed to create new version:", err);
			toastError("Failed to create new version", err);
		}
	},

	openTemplateFile: async (templateRelPath) => {
		const settings = (await invoke("load_settings")) as {
			templates_dir: string | null;
		};
		if (!settings.templates_dir) return;

		const fullPath = `${settings.templates_dir}/${templateRelPath}`;
		try {
			await invoke("open_file_in_os", { filePath: fullPath });
		} catch (err) {
			console.error("Failed to open template file:", err);
			toastError("Failed to open template file", err);
		}
	},
});
