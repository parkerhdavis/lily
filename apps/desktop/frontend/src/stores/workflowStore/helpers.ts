// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { useNavigationStore } from "@/stores/navigationStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { useToastStore } from "@/stores/toastStore";
import type {
	LilyFile,
	PersistedNavEntry,
	VariableInfo,
	WorkflowStep,
} from "@/types";
import { extractFilename, extractFolderName } from "@/utils/path";
import type { WorkflowState } from "./types";

const MAX_PERSISTED_HISTORY = 20;

/** Show a toast error notification. */
export function toastError(message: string, err?: unknown) {
	const detail = err ? `: ${String(err)}` : "";
	useToastStore.getState().addToast("error", `${message}${detail}`);
}

/** Show a toast success notification. */
export function toastSuccess(message: string) {
	useToastStore.getState().addToast("success", message);
}

/**
 * Resolve contact bindings on the backend and surface any non-fatal
 * warnings (e.g., a role bound to a deleted contact) as warning toasts.
 */
export async function resolveContactVariables(workingDir: string) {
	const warnings = await invoke<string[]>("resolve_contact_variables", {
		workingDir,
	});
	const toast = useToastStore.getState().addToast;
	for (const w of warnings) {
		toast("warning", w, 8000);
	}
}

/** Build a human-readable label for a given step + context. */
export function navLabel(
	step: WorkflowStep,
	workingDir: string | null,
): string {
	const folderName = workingDir ? extractFolderName(workingDir) : "";
	switch (step) {
		case "hub":
			return "Lily Hub";
		case "clients":
			return folderName ? `Clients \u203A ${folderName}` : "Clients";
		case "select-template":
			return `${folderName} \u203A Add Document`;
		case "app-settings":
			return "Settings";
		case "pipeline":
			return "Pipeline";
		default:
			return step;
	}
}

/** Push the current state to navigation history before navigating away. */
export function pushNav(state: WorkflowState) {
	const entry = {
		step: state.step,
		workingDir: state.workingDir,
		documentPath: null,
		templateRelPath: null,
		label: navLabel(state.step, state.workingDir),
	};
	useNavigationStore.getState().push(entry);
	debouncedPersistNavEntry(entry);
}

/**
 * Persist a navigation entry to settings for the "recent pages" list.
 * Deduplicates by (step, working_dir, document_path), keeps most recent,
 * excludes hub entries, caps at MAX_PERSISTED_HISTORY.
 */
function persistNavEntry(entry: {
	step: string;
	workingDir: string | null;
	documentPath: string | null;
	templateRelPath: string | null;
	label: string;
}) {
	if (entry.step === "hub") return;

	const settings = useSettingsStore.getState().settings;
	const newEntry: PersistedNavEntry = {
		step: entry.step,
		working_dir: entry.workingDir,
		document_path: entry.documentPath,
		template_rel_path: entry.templateRelPath,
		label: entry.label,
		visited_at: Date.now(),
	};

	// Deduplicate: remove existing entry with same composite key
	const key = (e: PersistedNavEntry) =>
		`${e.step}|${e.working_dir ?? ""}|${e.document_path ?? ""}`;
	const newKey = key(newEntry);
	const filtered = settings.navigation_history.filter((e) => key(e) !== newKey);

	const updated = [newEntry, ...filtered].slice(0, MAX_PERSISTED_HISTORY);
	useSettingsStore.getState().save({ navigation_history: updated });
}

/** Debounced version of persistNavEntry (500ms trailing edge). */
let persistTimer: ReturnType<typeof setTimeout> | null = null;
function debouncedPersistNavEntry(
	entry: Parameters<typeof persistNavEntry>[0],
) {
	if (persistTimer) clearTimeout(persistTimer);
	persistTimer = setTimeout(() => {
		persistTimer = null;
		persistNavEntry(entry);
	}, 500);
}

/**
 * Build a client-personalized document filename from a template path.
 *
 * Strips the extension and any " Template" suffix, then appends
 * "- {First} {Last}" from client variables when available.
 */
export function buildDocumentFilename(
	templateRelPath: string,
	lilyFile: LilyFile | null,
): string {
	const raw = extractFilename(templateRelPath);
	const extMatch = raw.match(/\.(docx|dotx)$/i);
	let baseName = extMatch ? raw.slice(0, -extMatch[0].length) : raw;
	baseName = baseName.replace(/ Template$/i, "");

	const firstName = lilyFile?.variables["Client First Name"]?.trim() ?? "";
	const lastName = lilyFile?.variables["Client Last Name"]?.trim() ?? "";
	const clientName = [firstName, lastName].filter(Boolean).join(" ");

	return clientName ? `${baseName} - ${clientName}.docx` : `${baseName}.docx`;
}

/** Load a plan template's slots as the document's variable list. */
export async function loadTemplateVariables(
	templatesDir: string,
	templateRelPath: string,
): Promise<VariableInfo[]> {
	try {
		return await invoke<VariableInfo[]>("load_template_variables", {
			templatesDir,
			templateRelPath,
		});
	} catch {
		return [];
	}
}
