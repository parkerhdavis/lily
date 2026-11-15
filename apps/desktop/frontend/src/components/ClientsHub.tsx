// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ClientWorkflow from "@/components/ClientWorkflow";
import CopyFromSpouseDialog from "@/components/CopyFromSpouseDialog";
import MigrationDialog, {
	type FieldMapping,
	type MigrationReport,
	type OrphanedVariable,
} from "@/components/MigrationDialog";
import QuestionnaireChooser, {
	type QuestionnaireChoice,
} from "@/components/QuestionnaireChooser";
import PageHeader from "@/components/ui/PageHeader";
import SectionHeading from "@/components/ui/SectionHeading";
import { questionnaireDef as fallbackDef } from "@/data/questionnaireDef";
import { useLilyIcon } from "@/hooks/useLilyIcon";
import { useSettingsStore } from "@/stores/settingsStore";
import { useToastStore } from "@/stores/toastStore";
import { useWorkflowStore } from "@/stores/workflowStore";
import type { ClientTreeNode } from "@/types";
import type {
	QuestionnaireDefFile,
	QuestionnaireSectionDef,
} from "@/types/questionnaire";
import { extractFilename, extractFolderName } from "@/utils/path";
import { scopedQuestionnaireSections } from "@/utils/questionnaireScope";

// ─── Helpers ─────────────────────────────────────────────────────────────

function stripDocx(name: string): string {
	return name.replace(/\.docx$/i, "");
}

function formatDate(iso: string): string {
	try {
		return new Date(iso).toLocaleDateString(undefined, {
			month: "short",
			day: "numeric",
			year: "numeric",
			hour: "numeric",
			minute: "2-digit",
		});
	} catch {
		return iso;
	}
}

// ─── Tree helpers ────────────────────────────────────────────────────────

/** Check if a directory path corresponds to a client (has .lily file) in the tree data. */
function isClientInTree(nodes: ClientTreeNode[], path: string): boolean {
	for (const node of nodes) {
		if (node.path === path) return node.is_client;
		if (isClientInTree(node.children, path)) return true;
	}
	return false;
}

// ─── Questionnaire variable extraction ──────────────────────────────────

/** Extract all variable names defined by a questionnaire's sections. */
function extractQuestionnaireVariables(
	sections: QuestionnaireSectionDef[],
): Set<string> {
	const vars = new Set<string>();
	for (const section of sections) {
		if (section.kind === "contacts") continue;
		for (const q of section.questions) {
			if (q.kind === "text" || q.kind === "conditional") {
				vars.add(q.variable);
			} else if (q.kind === "contact-role") {
				for (const varName of Object.keys(q.variableMappings)) {
					vars.add(varName);
				}
			}
		}
	}
	return vars;
}

/** Compute a migration report by comparing client variables against the
 *  current questionnaire definition. */
function computeMigrationReport(
	lilyFile: import("@/types").LilyFile,
	qDef: QuestionnaireDefFile,
): MigrationReport | null {
	const clientVersion = lilyFile.questionnaire_version ?? 0;
	if (clientVersion >= qDef.version) return null;

	const qVars = extractQuestionnaireVariables(qDef.sections);

	// Gather all variable names referenced by documents
	const docVars = new Set<string>();
	for (const meta of Object.values(lilyFile.documents ?? {})) {
		for (const v of meta.variable_names ?? []) {
			docVars.add(v);
		}
	}

	// Orphaned: have a value, not in questionnaire, not in documents
	const orphaned: OrphanedVariable[] = [];
	for (const [name, value] of Object.entries(lilyFile.variables ?? {})) {
		if (!value.trim()) continue; // skip empty values
		if (qVars.has(name)) continue; // still in questionnaire
		if (docVars.has(name)) continue; // used by a document
		orphaned.push({ name, value });
	}

	// Unfilled: in questionnaire, no value in .lily
	const clientVars = lilyFile.variables ?? {};
	const unfilled: string[] = [];
	for (const v of qVars) {
		if (!clientVars[v]?.trim()) {
			unfilled.push(v);
		}
	}

	if (orphaned.length === 0) return null; // nothing to migrate

	return {
		orphaned,
		unfilled,
		currentVersion: qDef.version,
	};
}

// ─── Local types ─────────────────────────────────────────────────────────

interface LibraryTree {
	dir: string;
	name: string;
	nodes: ClientTreeNode[];
}

interface ClientDoc {
	filename: string;
	templateRelPath: string;
	modifiedAt: string;
}

// ─── Main component ──────────────────────────────────────────────────────

export default function ClientsHub() {
	const { settings, save, addRecentDirectory } = useSettingsStore();
	const {
		workingDir,
		lilyFile,
		setWorkingDir,
		loadTemplates,
		goToHub,
		goToSettings,
		openDocument,
		openQuestionnaire,
		startAddDocument,
		deleteDocument,
		newVersionDocument,
		addMultipleDocuments,
		openTemplateFile,
	} = useWorkflowStore();
	const lilyIcon = useLilyIcon();

	const [trees, setTrees] = useState<LibraryTree[]>([]);
	const [treeLoading, setTreeLoading] = useState(false);
	// Folder selected for new-client setup (no .lily yet). Lifted here so both
	// the tree and the "Open Folder" header button feed the same create flow.
	const [pendingNewClientDir, setPendingNewClientDir] = useState<string | null>(
		null,
	);

	const libraryDirs = settings.client_library_dirs;

	// Load folder trees from library dirs
	const loadTrees = useCallback(async () => {
		if (!libraryDirs || libraryDirs.length === 0) {
			setTrees([]);
			return;
		}
		setTreeLoading(true);
		try {
			const results: LibraryTree[] = [];
			for (const dir of libraryDirs) {
				try {
					const nodes = await invoke<ClientTreeNode[]>("list_library_tree", {
						libraryDir: dir,
					});
					results.push({
						dir,
						name: extractFolderName(dir),
						nodes,
					});
				} catch (err) {
					console.error(`Failed to load tree for ${dir}:`, err);
				}
			}
			setTrees(results);
		} finally {
			setTreeLoading(false);
		}
	}, [libraryDirs]);

	useEffect(() => {
		loadTrees();
	}, [loadTrees]);

	const selectClient = async (dir: string) => {
		setPendingNewClientDir(null);
		await addRecentDirectory(dir);
		save({ last_working_dir: dir });
		if (settings.templates_dir) {
			await loadTemplates(settings.templates_dir);
		}
		setWorkingDir(dir);
	};

	const pickClientFolder = async () => {
		const selected = await open({
			directory: true,
			title: "Open Folder",
			defaultPath: settings.last_working_dir ?? undefined,
		});
		if (!selected) return;
		// A folder picked from the OS dialog can live outside the library, so it
		// won't be in the loaded tree — ask the backend whether it's already a
		// client. If so, open it; otherwise route into the new-client setup flow.
		try {
			const hasLily = await invoke<boolean>("has_lily_file", {
				workingDir: selected,
			});
			if (hasLily) {
				await selectClient(selected);
			} else {
				setPendingNewClientDir(selected);
			}
		} catch (err) {
			console.error("Failed to check for .lily file:", err);
			useToastStore
				.getState()
				.addToast("error", `Couldn't open folder: ${err}`);
		}
	};

	return (
		<div className="flex flex-col h-full">
			<PageHeader title="Clients" onBack={goToHub}>
				<button
					type="button"
					className="btn btn-primary btn-sm"
					onClick={pickClientFolder}
				>
					Open Folder…
				</button>
			</PageHeader>

			<div className="flex-1 overflow-hidden">
				<ClientsTreeTab
					trees={trees}
					loading={treeLoading}
					hasLibraryDirs={libraryDirs.length > 0}
					workingDir={workingDir}
					lilyFile={lilyFile}
					pendingNewClientDir={pendingNewClientDir}
					onSetPendingNewClientDir={setPendingNewClientDir}
					onSelectClient={selectClient}
					onGoToSettings={goToSettings}
					lilyIcon={lilyIcon}
					settings={settings}
					onOpenQuestionnaire={openQuestionnaire}
					onStartAddDocument={startAddDocument}
					onOpenDocument={openDocument}
					onDeleteDocument={deleteDocument}
					onNewVersionDocument={newVersionDocument}
					onAddMultipleDocuments={addMultipleDocuments}
					onOpenTemplateFile={openTemplateFile}
					onLoadTemplates={loadTemplates}
					onReloadTrees={loadTrees}
				/>
			</div>
		</div>
	);
}

// ─── Clients Tree Tab ───────────────────────────────────────────────────

function ClientsTreeTab({
	trees,
	loading,
	hasLibraryDirs,
	workingDir,
	lilyFile,
	pendingNewClientDir,
	onSetPendingNewClientDir,
	onSelectClient,
	onGoToSettings,
	lilyIcon,
	settings,
	onOpenQuestionnaire,
	onStartAddDocument,
	onOpenDocument,
	onDeleteDocument,
	onNewVersionDocument,
	onAddMultipleDocuments,
	onOpenTemplateFile,
	onLoadTemplates,
	onReloadTrees,
}: {
	trees: LibraryTree[];
	loading: boolean;
	hasLibraryDirs: boolean;
	workingDir: string | null;
	lilyFile: import("@/types").LilyFile | null;
	pendingNewClientDir: string | null;
	onSetPendingNewClientDir: (dir: string | null) => void;
	onSelectClient: (dir: string) => void;
	onGoToSettings: () => void;
	lilyIcon: string;
	settings: import("@/types").AppSettings;
	onOpenQuestionnaire: () => void;
	onStartAddDocument: () => void;
	onOpenDocument: (filename: string, templateRelPath: string) => void;
	onDeleteDocument: (filename: string) => Promise<void>;
	onNewVersionDocument: (filename: string) => Promise<void>;
	onAddMultipleDocuments: (
		templateRelPaths: string[],
		templatesDir: string,
	) => Promise<void>;
	onOpenTemplateFile: (templateRelPath: string) => Promise<void>;
	onLoadTemplates: (templatesDir: string) => Promise<void>;
	onReloadTrees: () => Promise<void>;
}) {
	const [creatingClient, setCreatingClient] = useState(false);
	const [chosenQuestionnaire, setChosenQuestionnaire] =
		useState<QuestionnaireChoice | null>(null);

	const allTreeNodes = useMemo(() => trees.flatMap((t) => t.nodes), [trees]);

	// Wrapper: if the selected folder isn't a client, show creation prompt
	const handleSelectDir = useCallback(
		(dir: string) => {
			const isClient = isClientInTree(allTreeNodes, dir);
			if (isClient) {
				onSelectClient(dir);
			} else {
				onSetPendingNewClientDir(dir);
			}
		},
		[allTreeNodes, onSelectClient, onSetPendingNewClientDir],
	);

	const handleCreateClient = useCallback(async () => {
		if (!pendingNewClientDir) return;
		setCreatingClient(true);
		try {
			await invoke("create_lily_file", {
				workingDir: pendingNewClientDir,
				questionnaireId: chosenQuestionnaire?.id ?? null,
				questionnaireVersion: chosenQuestionnaire?.version ?? null,
			});
			await onReloadTrees();
			onSetPendingNewClientDir(null);
			onSelectClient(pendingNewClientDir);
		} catch (err) {
			console.error("Failed to create .lily file:", err);
			useToastStore
				.getState()
				.addToast("error", `Failed to create client: ${err}`);
		} finally {
			setCreatingClient(false);
		}
	}, [
		pendingNewClientDir,
		chosenQuestionnaire,
		onSelectClient,
		onReloadTrees,
		onSetPendingNewClientDir,
	]);

	if (!hasLibraryDirs) {
		return (
			<div className="flex flex-col items-center justify-center h-full gap-4 p-8">
				<img src={lilyIcon} alt="" className="size-16 opacity-20" />
				<p className="text-base-content/50 text-center max-w-sm">
					Configure a client library folder in Settings to browse and manage
					your clients.
				</p>
				<p className="text-xs text-base-content/30 text-center max-w-sm">
					A client library is a folder containing client subfolders, each with a
					.lily project file.
				</p>
				<button
					type="button"
					className="btn btn-primary btn-sm"
					onClick={onGoToSettings}
				>
					Open Settings
				</button>
			</div>
		);
	}

	if (loading) {
		return (
			<div className="flex items-center justify-center h-full">
				<span className="loading loading-spinner loading-md" />
			</div>
		);
	}

	return (
		<div className="flex h-full">
			{/* Left sidebar: folder tree */}
			<div className="w-72 shrink-0 border-r border-base-300 overflow-y-auto p-4">
				{trees.map((lib, i) => (
					<div key={lib.dir}>
						{i > 0 && <div className="border-b border-base-300 mb-4" />}
						<SectionHeading className="mb-3">{lib.name}</SectionHeading>
						{lib.nodes.length === 0 ? (
							<p className="text-sm text-base-content/50 px-3">
								No folders found.
							</p>
						) : (
							<div className="flex flex-col gap-0.5">
								{lib.nodes.map((node) => (
									<ClientTreeItem
										key={node.path}
										node={node}
										selectedDir={pendingNewClientDir ?? workingDir}
										onSelectDir={handleSelectDir}
									/>
								))}
							</div>
						)}
					</div>
				))}
			</div>

			{/* Right pane: client content */}
			{pendingNewClientDir ? (
				<div className="flex-1 flex flex-col items-center justify-center gap-4 p-8">
					<FolderIcon open={false} />
					<h3 className="text-lg font-semibold">
						{extractFolderName(pendingNewClientDir)}
					</h3>
					<p className="text-sm text-base-content/60 text-center max-w-sm">
						This folder doesn't have a client project file yet. Choose a
						questionnaire and create one to get started.
					</p>
					<div className="w-full max-w-xs text-left rounded-lg border border-base-300 bg-base-200/40 p-3">
						<p className="text-xs font-medium text-base-content/60 mb-1">
							Questionnaire
						</p>
						<QuestionnaireChooser onChange={setChosenQuestionnaire} />
					</div>
					<div className="flex gap-2">
						<button
							type="button"
							className="btn btn-ghost btn-sm"
							onClick={() => onSetPendingNewClientDir(null)}
						>
							Cancel
						</button>
						<button
							type="button"
							className="btn btn-primary btn-sm"
							onClick={handleCreateClient}
							disabled={creatingClient}
						>
							{creatingClient ? (
								<span className="loading loading-spinner loading-xs" />
							) : null}
							Create Client
						</button>
					</div>
				</div>
			) : workingDir && lilyFile ? (
				<ClientContentPane
					workingDir={workingDir}
					lilyFile={lilyFile}
					lilyIcon={lilyIcon}
					settings={settings}
					onOpenQuestionnaire={onOpenQuestionnaire}
					onStartAddDocument={onStartAddDocument}
					onOpenDocument={onOpenDocument}
					onDeleteDocument={onDeleteDocument}
					onNewVersionDocument={onNewVersionDocument}
					onAddMultipleDocuments={onAddMultipleDocuments}
					onOpenTemplateFile={onOpenTemplateFile}
					onLoadTemplates={onLoadTemplates}
				/>
			) : (
				<div className="flex-1 flex items-center justify-center text-base-content/40 text-sm">
					Select a client to view details
				</div>
			)}
		</div>
	);
}

// ─── Client Content Pane (merged from ClientHub) ────────────────────────

function ClientContentPane({
	workingDir,
	lilyFile,
	lilyIcon,
	settings,
	onOpenQuestionnaire,
	onStartAddDocument,
	onOpenDocument,
	onDeleteDocument,
	onNewVersionDocument,
	onAddMultipleDocuments,
	onOpenTemplateFile,
	onLoadTemplates,
}: {
	workingDir: string;
	lilyFile: import("@/types").LilyFile;
	lilyIcon: string;
	settings: import("@/types").AppSettings;
	onOpenQuestionnaire: () => void;
	onStartAddDocument: () => void;
	onOpenDocument: (filename: string, templateRelPath: string) => void;
	onDeleteDocument: (filename: string) => Promise<void>;
	onNewVersionDocument: (filename: string) => Promise<void>;
	onAddMultipleDocuments: (
		templateRelPaths: string[],
		templatesDir: string,
	) => Promise<void>;
	onOpenTemplateFile: (templateRelPath: string) => Promise<void>;
	onLoadTemplates: (templatesDir: string) => Promise<void>;
}) {
	const [docSearch, setDocSearch] = useState("");
	const [showCopyFromSpouse, setShowCopyFromSpouse] = useState(false);

	// Document consistency check: compare .lily metadata against actual files on disk
	const [missingOnDisk, setMissingOnDisk] = useState<Set<string>>(new Set());
	const [untrackedOnDisk, setUntrackedOnDisk] = useState<string[]>([]);
	useEffect(() => {
		(async () => {
			try {
				const report = await invoke<{
					missing_on_disk: string[];
					untracked_on_disk: string[];
				}>("check_document_consistency", { workingDir });
				setMissingOnDisk(new Set(report.missing_on_disk));
				setUntrackedOnDisk(report.untracked_on_disk.sort());
			} catch {
				// Non-critical — continue without consistency info
			}
		})();
	}, [workingDir, lilyFile]);

	// Dynamic questionnaire definition for stats + migration
	const [qDef, setQDef] = useState<QuestionnaireSectionDef[]>(fallbackDef);
	const [migrationReport, setMigrationReport] =
		useState<MigrationReport | null>(null);
	useEffect(() => {
		(async () => {
			try {
				// Only the client's own stamped questionnaire drives stats/migration.
				// An un-stamped client keeps the compiled-in fallback def until the
				// user picks one (prompted when they open the questionnaire).
				if (!lilyFile.questionnaire_id) return;
				const def = await invoke<QuestionnaireDefFile>("load_questionnaire", {
					id: lilyFile.questionnaire_id,
				});
				setQDef(def.sections);
				setMigrationReport(computeMigrationReport(lilyFile, def));
			} catch {
				// Use fallback
			}
		})();
	}, [lilyFile.questionnaire_id, lilyFile]);

	const allDocs = useMemo(() => {
		if (!lilyFile.documents) return [];
		return Object.entries(lilyFile.documents)
			.map(([filename, meta]) => ({
				filename,
				templateRelPath: meta.template_rel_path,
				modifiedAt: meta.modified_at,
			}))
			.sort((a, b) => a.filename.localeCompare(b.filename));
	}, [lilyFile]);

	const scopedQDef = useMemo(
		() => scopedQuestionnaireSections(qDef, lilyFile.required_documents),
		[qDef, lilyFile.required_documents],
	);

	const questionnaireStats = useMemo(() => {
		const vars = lilyFile.variables ?? {};
		const contactCount = lilyFile.contacts?.length ?? 0;
		let total = 0;
		let filled = 0;
		for (const section of scopedQDef) {
			if (section.kind === "contacts") {
				total++;
				if (contactCount > 0) filled++;
				continue;
			}
			for (const q of section.questions) {
				if (q.kind === "text") {
					total++;
					if (vars[q.variable]?.trim()) filled++;
				}
			}
		}
		return { total, filled };
	}, [lilyFile, scopedQDef]);

	const handleApplyMigration = useCallback(
		async (mappings: FieldMapping[], removeOrphaned: string[]) => {
			if (!migrationReport) return;
			try {
				const updated = await invoke<import("@/types").LilyFile>(
					"apply_variable_migration",
					{
						workingDir,
						mappings,
						removeOrphaned,
						newQuestionnaireVersion: migrationReport.currentVersion,
					},
				);
				useWorkflowStore.setState({ lilyFile: updated });
				setMigrationReport(null);
				useToastStore
					.getState()
					.addToast("success", "Migration applied successfully");
			} catch (err) {
				useToastStore.getState().addToast("error", `Migration failed: ${err}`);
			}
		},
		[workingDir, migrationReport],
	);

	const handleSkipMigration = useCallback(() => {
		setMigrationReport(null);
	}, []);

	const handleAddDocument = () => {
		if (settings.templates_dir) {
			onLoadTemplates(settings.templates_dir);
		}
		onStartAddDocument();
	};

	const handleNewVersionMissing = useCallback(
		async (filename: string, templateRelPath: string) => {
			if (!settings.templates_dir) {
				useToastStore
					.getState()
					.addToast(
						"error",
						"Templates folder is not configured. Set it in Settings.",
					);
				return;
			}
			try {
				await onDeleteDocument(filename);
				await onAddMultipleDocuments([templateRelPath], settings.templates_dir);
			} catch (err) {
				useToastStore
					.getState()
					.addToast("error", `Failed to create new version: ${err}`);
			}
		},
		[settings.templates_dir, onDeleteDocument, onAddMultipleDocuments],
	);

	const handleEditInDocx = useCallback(
		async (filename: string) => {
			try {
				await invoke("open_file_in_os", {
					filePath: `${workingDir}/${filename}`,
				});
			} catch (err) {
				useToastStore
					.getState()
					.addToast("error", `Failed to open document: ${err}`);
			}
		},
		[workingDir],
	);

	const handleExport = async () => {
		const folderName = extractFolderName(workingDir);
		const path = await saveDialog({
			title: "Export Client Data",
			defaultPath: `${folderName} - Export.json`,
			filters: [{ name: "JSON", extensions: ["json"] }],
		});
		if (path) {
			try {
				await invoke("export_client_data", {
					workingDir,
					exportPath: path,
				});
				useToastStore.getState().addToast("success", "Client data exported");
			} catch (err) {
				useToastStore.getState().addToast("error", `Export failed: ${err}`);
			}
		}
	};

	const handleExportArchive = async () => {
		const folderName = extractFolderName(workingDir);
		const path = await saveDialog({
			title: "Export Client Folder",
			defaultPath: `${folderName} - ${new Date().toISOString().slice(0, 10)}.zip`,
			filters: [{ name: "Zip archive", extensions: ["zip"] }],
		});
		if (!path) return;
		try {
			await invoke("export_client_archive", {
				workingDir,
				exportPath: path,
			});
			useToastStore.getState().addToast("success", "Client folder exported");
		} catch (err) {
			useToastStore.getState().addToast("error", `Export failed: ${err}`);
		}
	};

	const handleImport = async () => {
		const path = await open({
			title: "Import Client Data",
			filters: [{ name: "JSON / Lily", extensions: ["json", "lily"] }],
		});
		if (path) {
			try {
				const updated = await invoke<import("@/types").LilyFile>(
					"import_client_data",
					{ workingDir, importPath: path },
				);
				useWorkflowStore.setState({ lilyFile: updated });
				useToastStore.getState().addToast("success", "Client data imported");
			} catch (err) {
				useToastStore.getState().addToast("error", `Import failed: ${err}`);
			}
		}
	};

	const handleOpenFolder = async () => {
		try {
			await invoke("open_file_in_os", { filePath: workingDir });
		} catch (err) {
			console.error("Failed to open folder:", err);
		}
	};

	const folderName = extractFolderName(workingDir);
	const contactCount = lilyFile.contacts?.length ?? 0;
	const docCount = allDocs.length;
	// A brand-new client now always has a questionnaire_id stamped at creation
	// (the Create Client pane pre-selects one), so questionnaire_id is no longer
	// a "fresh workspace" signal — emptiness of the actual client data is.
	const isFreshWorkspace =
		docCount === 0 &&
		contactCount === 0 &&
		Object.keys(lilyFile.variables ?? {}).length === 0 &&
		(lilyFile.required_documents ?? []).length === 0;

	return (
		<>
			{migrationReport && (
				<MigrationDialog
					report={migrationReport}
					onApply={handleApplyMigration}
					onSkip={handleSkipMigration}
				/>
			)}
			{showCopyFromSpouse && (
				<CopyFromSpouseDialog
					targetDir={workingDir}
					onClose={() => setShowCopyFromSpouse(false)}
				/>
			)}
			<div className="flex-1 flex flex-col min-w-0">
				{/* Pinned header */}
				<div className="shrink-0 border-b border-base-300 px-6 py-4">
					<div className="max-w-3xl mx-auto flex items-start justify-between gap-4">
						<div className="min-w-0">
							<h2 className="text-xl font-semibold truncate">{folderName}</h2>
							<p className="text-xs text-base-content/40 font-mono truncate mt-0.5">
								{workingDir}
							</p>
							<p className="text-sm text-base-content/50 mt-1">
								{docCount} document{docCount !== 1 ? "s" : ""}
								{contactCount > 0 &&
									` \u00B7 ${contactCount} contact${contactCount !== 1 ? "s" : ""}`}
							</p>
						</div>
						<div className="flex gap-2 shrink-0">
							<button
								type="button"
								className="btn btn-ghost btn-sm"
								onClick={handleOpenFolder}
								title="Open in file manager"
							>
								Open Folder
							</button>
							<button
								type="button"
								className="btn btn-primary btn-sm"
								onClick={handleAddDocument}
							>
								+ Add Document
							</button>
							<div className="dropdown dropdown-end">
								<button
									type="button"
									className="btn btn-ghost btn-sm"
									tabIndex={0}
								>
									<svg
										xmlns="http://www.w3.org/2000/svg"
										viewBox="0 0 16 16"
										fill="currentColor"
										className="size-4"
									>
										<title>More</title>
										<path d="M8 2a1.5 1.5 0 1 1 0 3 1.5 1.5 0 0 1 0-3ZM8 6.5a1.5 1.5 0 1 1 0 3 1.5 1.5 0 0 1 0-3ZM9.5 12.5a1.5 1.5 0 1 0-3 0 1.5 1.5 0 0 0 3 0Z" />
									</svg>
								</button>
								<ul
									tabIndex={0}
									className="dropdown-content menu bg-base-100 rounded-box shadow-lg border border-base-300 w-44 p-1 z-50"
								>
									<li>
										<button type="button" onClick={handleExportArchive}>
											Export Client Folder
										</button>
									</li>
									<li>
										<button type="button" onClick={handleExport}>
											Export Client Data
										</button>
									</li>
									<li>
										<button type="button" onClick={handleImport}>
											Import Client Data
										</button>
									</li>
								</ul>
							</div>
						</div>
					</div>
				</div>

				{/* Scrollable content */}
				<div className="flex-1 overflow-y-auto p-6">
					<div className="max-w-3xl mx-auto space-y-6">
						{isFreshWorkspace && (
							<div className="rounded-xl border border-base-300 bg-base-200/40 p-4 flex items-start gap-3">
								<div className="flex-1 min-w-0">
									<div className="font-medium text-sm">
										Spouse already set up?
									</div>
									<div className="text-sm text-base-content/60 mt-0.5">
										Copy questionnaire data, contacts, and documents from their
										folder, with the client/spouse swap applied.
									</div>
								</div>
								<button
									type="button"
									className="btn btn-sm btn-outline"
									onClick={() => setShowCopyFromSpouse(true)}
								>
									Copy from Spouse
								</button>
							</div>
						)}
						{/* Questionnaire card */}
						<button
							type="button"
							className="w-full text-left p-5 rounded-xl border-2 border-primary/40 bg-base-100 shadow-[0_4px_16px_rgba(0,0,0,0.25)] hover:shadow-[0_8px_28px_rgba(0,0,0,0.35)] transition-shadow"
							onClick={onOpenQuestionnaire}
						>
							<div className="flex items-center gap-4">
								<img src={lilyIcon} alt="" className="size-9 opacity-60" />
								<div className="flex-1 min-w-0">
									<div className="font-semibold text-base">
										Client Questionnaire
									</div>
									<div className="text-sm text-base-content/50 mt-0.5">
										{questionnaireStats.total > 0
											? `${questionnaireStats.filled} of ${questionnaireStats.total} fields filled`
											: "Fill out client information"}
										{contactCount > 0 &&
											` \u00B7 ${contactCount} contact${contactCount !== 1 ? "s" : ""}`}
									</div>
								</div>
								{questionnaireStats.total > 0 && (
									<div
										className="radial-progress text-primary text-sm"
										style={
											{
												"--value": Math.round(
													(questionnaireStats.filled /
														questionnaireStats.total) *
														100,
												),
												"--size": "3rem",
												"--thickness": "3px",
											} as React.CSSProperties
										}
										role="progressbar"
									>
										{questionnaireStats.filled}/{questionnaireStats.total}
									</div>
								)}
							</div>
						</button>

						<ClientWorkflow
							lilyFile={lilyFile}
							questionnaireStats={questionnaireStats}
							templatesDir={settings.templates_dir}
							onOpenQuestionnaire={onOpenQuestionnaire}
							onStartAddDocument={onStartAddDocument}
							onOpenDocument={onOpenDocument}
							onAddMultipleDocuments={onAddMultipleDocuments}
						/>

						{/* Documents section */}
						<div>
							<SectionHeading className="mb-3">Documents</SectionHeading>

							{allDocs.length > 3 && (
								<input
									type="text"
									className="input input-bordered input-sm w-full mb-3"
									placeholder="Search documents..."
									value={docSearch}
									onChange={(e) => setDocSearch(e.target.value)}
								/>
							)}

							{allDocs.length === 0 ? (
								<div className="rounded-xl border border-base-300 bg-base-100 p-8 text-center text-base-content/50">
									<p className="text-base">No documents in this folder yet.</p>
									<button
										type="button"
										className="btn btn-primary btn-sm mt-4"
										onClick={handleAddDocument}
									>
										Add New Document
									</button>
								</div>
							) : (
								<div className="rounded-xl border border-base-300 bg-base-100 shadow-[0_4px_16px_rgba(0,0,0,0.25)] divide-y divide-base-200 overflow-hidden">
									{allDocs
										.filter((doc) => {
											if (!docSearch.trim()) return true;
											const q = docSearch.trim().toLowerCase();
											return doc.filename.toLowerCase().includes(q);
										})
										.map((doc) => (
											<DocumentRow
												key={doc.filename}
												doc={doc}
												isMissing={missingOnDisk.has(doc.filename)}
												onOpen={onOpenDocument}
												onEditInDocx={handleEditInDocx}
												onDelete={onDeleteDocument}
												onNewVersion={onNewVersionDocument}
												onNewVersionMissing={handleNewVersionMissing}
												onOpenTemplate={onOpenTemplateFile}
											/>
										))}
									{untrackedOnDisk.map((filename) => (
										<div
											key={`untracked:${filename}`}
											className="w-full text-left px-5 py-4 opacity-60"
										>
											<div className="flex flex-col gap-0.5">
												<span className="font-medium text-base flex items-center gap-2">
													{stripDocx(filename)}
													<span className="badge badge-warning badge-sm">
														untracked
													</span>
												</span>
												<span className="text-sm text-base-content/40">
													File exists on disk but is not tracked in project
												</span>
											</div>
										</div>
									))}
								</div>
							)}
						</div>
					</div>
				</div>
			</div>
		</>
	);
}

// ─── Document Row (from ClientHub) ──────────────────────────────────────

function DocumentRow({
	doc,
	isMissing,
	onOpen,
	onEditInDocx,
	onDelete,
	onNewVersion,
	onNewVersionMissing,
	onOpenTemplate,
}: {
	doc: ClientDoc;
	isMissing?: boolean;
	onOpen: (filename: string, templateRelPath: string) => void;
	onEditInDocx: (filename: string) => Promise<void>;
	onDelete: (filename: string) => Promise<void>;
	onNewVersion: (filename: string) => Promise<void>;
	onNewVersionMissing: (
		filename: string,
		templateRelPath: string,
	) => Promise<void>;
	onOpenTemplate: (templateRelPath: string) => Promise<void>;
}) {
	const [menuPos, setMenuPos] = useState<{ x: number; y: number } | null>(null);
	const [confirmingDelete, setConfirmingDelete] = useState(false);
	const [confirmingNewVersionMissing, setConfirmingNewVersionMissing] =
		useState(false);
	const menuRef = useRef<HTMLDivElement>(null);
	const deleteDialogRef = useRef<HTMLDialogElement>(null);
	const newVersionMissingDialogRef = useRef<HTMLDialogElement>(null);

	useEffect(() => {
		if (!menuPos) return;
		const handleClick = (e: MouseEvent) => {
			if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
				setMenuPos(null);
			}
		};
		const handleEscape = (e: KeyboardEvent) => {
			if (e.key === "Escape") setMenuPos(null);
		};
		document.addEventListener("mousedown", handleClick);
		document.addEventListener("keydown", handleEscape);
		return () => {
			document.removeEventListener("mousedown", handleClick);
			document.removeEventListener("keydown", handleEscape);
		};
	}, [menuPos]);

	const handleContextMenu = (e: React.MouseEvent) => {
		e.preventDefault();
		const menuW = 192;
		const menuH = 140;
		const x = Math.min(e.clientX, window.innerWidth - menuW);
		const y = Math.min(e.clientY, window.innerHeight - menuH);
		setMenuPos({ x, y });
	};

	const handleNewVersionClick = () => {
		if (isMissing) {
			setConfirmingNewVersionMissing(true);
			setTimeout(() => newVersionMissingDialogRef.current?.showModal(), 0);
		} else {
			onNewVersion(doc.filename);
		}
	};

	return (
		<>
			<div
				className={`px-5 py-4 flex items-center gap-4 hover:bg-base-200/60 transition-colors ${isMissing ? "opacity-60" : ""}`}
				onContextMenu={handleContextMenu}
			>
				<div className="flex-1 min-w-0 flex flex-col gap-0.5">
					<span className="font-medium text-base flex items-center gap-2">
						{stripDocx(doc.filename)}
						{isMissing && (
							<span className="badge badge-error badge-sm">missing</span>
						)}
					</span>
					<span className="text-sm text-base-content/40 truncate">
						{isMissing ? (
							"File no longer exists on disk"
						) : (
							<>
								from {stripDocx(extractFilename(doc.templateRelPath))}
								{" \u00B7 "}
								{formatDate(doc.modifiedAt)}
							</>
						)}
					</span>
				</div>
				<div className="shrink-0 flex items-center gap-2">
					<button
						type="button"
						className="btn btn-sm btn-primary"
						onClick={() => onOpen(doc.filename, doc.templateRelPath)}
						disabled={isMissing}
						title={
							isMissing ? "File is missing on disk" : "Open in Lily editor"
						}
					>
						Edit in Lily
					</button>
					<button
						type="button"
						className="btn btn-sm btn-ghost"
						onClick={() => onEditInDocx(doc.filename)}
						disabled={isMissing}
						title={
							isMissing
								? "File is missing on disk"
								: "Open .docx in default app"
						}
					>
						Edit in Docx
					</button>
				</div>
			</div>

			{menuPos && (
				<div
					ref={menuRef}
					className="fixed z-50 menu bg-base-100 rounded-box shadow-lg border border-base-300 w-48 p-1"
					style={{ left: menuPos.x, top: menuPos.y }}
				>
					<li>
						<button
							type="button"
							className="text-sm"
							onClick={() => {
								setMenuPos(null);
								handleNewVersionClick();
							}}
						>
							New Version
						</button>
					</li>
					<li>
						<button
							type="button"
							className="text-sm text-error"
							onClick={() => {
								setMenuPos(null);
								setConfirmingDelete(true);
								setTimeout(() => deleteDialogRef.current?.showModal(), 0);
							}}
						>
							Delete
						</button>
					</li>
					<div className="divider my-0" />
					<li>
						<button
							type="button"
							className="text-sm"
							onClick={() => {
								setMenuPos(null);
								onOpenTemplate(doc.templateRelPath);
							}}
						>
							Open Template
						</button>
					</li>
				</div>
			)}

			{confirmingDelete && (
				<dialog
					ref={deleteDialogRef}
					className="modal"
					onClick={(e) => {
						if (e.target === deleteDialogRef.current) {
							deleteDialogRef.current?.close();
							setConfirmingDelete(false);
						}
					}}
				>
					<div className="modal-box">
						<h3 className="text-lg font-bold mb-2">Delete document?</h3>
						<p className="text-base-content/70 mb-4">
							Are you sure you want to delete <strong>{doc.filename}</strong>?
							This cannot be undone.
						</p>
						<div className="modal-action">
							<button
								type="button"
								className="btn btn-ghost btn-sm"
								onClick={() => {
									deleteDialogRef.current?.close();
									setConfirmingDelete(false);
								}}
							>
								Cancel
							</button>
							<button
								type="button"
								className="btn btn-error btn-sm"
								onClick={async () => {
									deleteDialogRef.current?.close();
									setConfirmingDelete(false);
									await onDelete(doc.filename);
								}}
							>
								Delete
							</button>
						</div>
					</div>
				</dialog>
			)}

			{confirmingNewVersionMissing && (
				<dialog
					ref={newVersionMissingDialogRef}
					className="modal"
					onClick={(e) => {
						if (e.target === newVersionMissingDialogRef.current) {
							newVersionMissingDialogRef.current?.close();
							setConfirmingNewVersionMissing(false);
						}
					}}
				>
					<div className="modal-box">
						<h3 className="text-lg font-bold mb-2">Create new version?</h3>
						<p className="text-base-content/70 mb-4">
							The original document <strong>{doc.filename}</strong> is missing
							on disk. Creating a new version will remove this entry and create
							a fresh document from{" "}
							<strong>{stripDocx(extractFilename(doc.templateRelPath))}</strong>
							.
						</p>
						<div className="modal-action">
							<button
								type="button"
								className="btn btn-ghost btn-sm"
								onClick={() => {
									newVersionMissingDialogRef.current?.close();
									setConfirmingNewVersionMissing(false);
								}}
							>
								Cancel
							</button>
							<button
								type="button"
								className="btn btn-primary btn-sm"
								onClick={async () => {
									newVersionMissingDialogRef.current?.close();
									setConfirmingNewVersionMissing(false);
									await onNewVersionMissing(doc.filename, doc.templateRelPath);
								}}
							>
								Create New Version
							</button>
						</div>
					</div>
				</dialog>
			)}
		</>
	);
}

// ─── Tree Components ────────────────────────────────────────────────────

function ClientTreeItem({
	node,
	selectedDir,
	onSelectDir,
}: {
	node: ClientTreeNode;
	selectedDir: string | null;
	onSelectDir: (dir: string) => void;
}) {
	const hasChildren = node.children.length > 0;

	if (node.is_client) {
		return (
			<ClientTreeClient
				node={node}
				isSelected={selectedDir === node.path}
				hasChildren={hasChildren}
				onSelect={onSelectDir}
				selectedDir={selectedDir}
			/>
		);
	}

	// Any non-client folder is selectable to set up a new client here (the
	// "Create Client" panel only requires that no .lily exists yet). Folders
	// with subfolders are additionally expandable via the chevron.
	return (
		<ClientTreeFolder
			node={node}
			hasChildren={hasChildren}
			selectedDir={selectedDir}
			onSelectDir={onSelectDir}
		/>
	);
}

function ClientTreeFolder({
	node,
	hasChildren,
	selectedDir,
	onSelectDir,
}: {
	node: ClientTreeNode;
	hasChildren: boolean;
	selectedDir: string | null;
	onSelectDir: (dir: string) => void;
}) {
	const [expanded, setExpanded] = useState(false);
	const isSelected = selectedDir === node.path;

	return (
		<div>
			{/* Row selects the folder (\u2192 "Create Client" panel); the chevron
			    toggles expansion, mirroring how client rows behave. */}
			<button
				type="button"
				className={`btn btn-ghost btn-sm justify-start text-left w-full h-auto py-2 px-3 font-normal gap-2 ${
					isSelected
						? "bg-primary/10 text-primary font-medium"
						: "text-base-content/50"
				}`}
				onClick={() => onSelectDir(node.path)}
			>
				{hasChildren ? (
					<span
						className="text-xs opacity-40"
						onClick={(e) => {
							e.stopPropagation();
							setExpanded(!expanded);
						}}
						onKeyDown={(e) => {
							if (e.key === "Enter" || e.key === " ") {
								e.stopPropagation();
								setExpanded(!expanded);
							}
						}}
						role="button"
						tabIndex={-1}
					>
						{expanded ? "\u25BE" : "\u25B8"}
					</span>
				) : (
					<span className="w-3" />
				)}
				<FolderIcon open={hasChildren && expanded} />
				<span className="truncate">{node.name}</span>
			</button>
			{hasChildren && expanded && (
				<div className="ml-4 border-l border-base-300 pl-1">
					{node.children.map((child) => (
						<ClientTreeItem
							key={child.path}
							node={child}
							selectedDir={selectedDir}
							onSelectDir={onSelectDir}
						/>
					))}
				</div>
			)}
		</div>
	);
}

function ClientTreeClient({
	node,
	isSelected,
	hasChildren,
	onSelect,
	selectedDir,
}: {
	node: ClientTreeNode;
	isSelected: boolean;
	hasChildren: boolean;
	onSelect: (dir: string) => void;
	selectedDir: string | null;
}) {
	const [expanded, setExpanded] = useState(false);

	return (
		<div>
			<button
				type="button"
				className={`btn btn-ghost btn-sm justify-start text-left w-full h-auto py-2 px-3 font-normal gap-2 ${
					isSelected ? "bg-primary/10 text-primary font-medium" : ""
				}`}
				onClick={() => onSelect(node.path)}
			>
				{hasChildren ? (
					<span
						className="text-xs opacity-40"
						onClick={(e) => {
							e.stopPropagation();
							setExpanded(!expanded);
						}}
						onKeyDown={(e) => {
							if (e.key === "Enter" || e.key === " ") {
								e.stopPropagation();
								setExpanded(!expanded);
							}
						}}
						role="button"
						tabIndex={-1}
					>
						{expanded ? "\u25BE" : "\u25B8"}
					</span>
				) : (
					<span className="w-3" />
				)}
				<ClientIcon />
				<span className="truncate">{node.name}</span>
			</button>
			{hasChildren && expanded && (
				<div className="ml-4 border-l border-base-300 pl-1">
					{node.children.map((child) => (
						<ClientTreeItem
							key={child.path}
							node={child}
							selectedDir={selectedDir}
							onSelectDir={onSelect}
						/>
					))}
				</div>
			)}
		</div>
	);
}

// ─── Icons ──────────────────────────────────────────────────────────────

function FolderIcon({ open }: { open: boolean }) {
	return (
		<svg
			xmlns="http://www.w3.org/2000/svg"
			className="h-4 w-4 shrink-0 opacity-50"
			fill="none"
			viewBox="0 0 24 24"
			stroke="currentColor"
		>
			<title>Folder</title>
			{open ? (
				<path
					strokeLinecap="round"
					strokeLinejoin="round"
					strokeWidth={2}
					d="M5 19a2 2 0 01-2-2V7a2 2 0 012-2h4l2 2h4a2 2 0 012 2v1M5 19h14a2 2 0 002-2v-5a2 2 0 00-2-2H9a2 2 0 00-2 2v5a2 2 0 01-2 2z"
				/>
			) : (
				<path
					strokeLinecap="round"
					strokeLinejoin="round"
					strokeWidth={2}
					d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z"
				/>
			)}
		</svg>
	);
}

function ClientIcon() {
	return (
		<svg
			xmlns="http://www.w3.org/2000/svg"
			className="h-4 w-4 shrink-0 opacity-50"
			fill="none"
			viewBox="0 0 24 24"
			stroke="currentColor"
		>
			<title>Client</title>
			<path
				strokeLinecap="round"
				strokeLinejoin="round"
				strokeWidth={2}
				d="M16 7a4 4 0 11-8 0 4 4 0 018 0zM12 14a7 7 0 00-7 7h14a7 7 0 00-7-7z"
			/>
		</svg>
	);
}
