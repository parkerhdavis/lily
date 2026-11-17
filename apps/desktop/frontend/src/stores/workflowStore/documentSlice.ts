// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { useSettingsStore } from "@/stores/settingsStore";
import type { LilyFile, VariableSchema } from "@/types";
import {
	buildDocumentFilename,
	loadTemplateVariables,
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
					clientKey: null,
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

			// Rendering writes the draft record, so reload before the hub
			// reads the updated document metadata.
			const renderedLilyFile = await invoke<LilyFile>("load_lily_file_cmd", {
				workingDir,
			});

			pushNav(get());
			set({
				lilyFile: renderedLilyFile,
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
			const settings = useSettingsStore.getState().settings;
			const docTemplateRelPath =
				templateRelPath ?? lilyFile?.documents[filename]?.template_rel_path;
			if (settings.templates_dir && docTemplateRelPath) {
				let schema: VariableSchema | null = null;
				try {
					schema = await invoke<VariableSchema>("load_template_schema", {
						templatesDir: settings.templates_dir,
						templateRelPath: docTemplateRelPath,
					});
				} catch {
					// The template may have no sidecar schema; render only if it does.
				}
				if (schema && planPathFor(settings.templates_dir, schema)) {
					await applyDocumentValues(
						settings.templates_dir,
						docTemplateRelPath,
						docPath,
						schema,
					);
					const renderedLilyFile = await invoke<LilyFile>(
						"load_lily_file_cmd",
						{
							workingDir,
						},
					);
					set({ lilyFile: renderedLilyFile });
				}
			}
			await invoke("open_file_in_os", { filePath: docPath });
			set({ loading: false });
		} catch (err) {
			set({ error: String(err), loading: false });
			toastError("Failed to open document", err);
		}
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
