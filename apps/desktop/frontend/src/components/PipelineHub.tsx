// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useState } from "react";
import PageHeader from "@/components/ui/PageHeader";
import SectionHeading from "@/components/ui/SectionHeading";
import { useLilyIcon } from "@/hooks/useLilyIcon";
import { useQuestionnaireStore } from "@/stores/questionnaireStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { useWorkflowStore } from "@/stores/workflowStore";
import { loadTemplateVariables } from "@/stores/workflowStore/helpers";
import type {
	ReferenceDocument,
	TemplateTreeNode,
	VariableInfo,
} from "@/types";
import type { QuestionnaireIndex } from "@/types/questionnaire";
import { extractFilename } from "@/utils/path";

// ─── Tree building (reused from TemplatePicker) ─────────────────────────────

function buildTree(paths: string[]): TemplateTreeNode[] {
	const root: TemplateTreeNode[] = [];
	for (const relPath of paths) {
		const segments = relPath.split("/");
		let children = root;
		for (let i = 0; i < segments.length; i++) {
			const name = segments[i];
			const isFile = i === segments.length - 1;
			if (isFile) {
				children.push({ kind: "file", name, relPath });
			} else {
				let folder = children.find(
					(n): n is TemplateTreeNode & { kind: "folder" } =>
						n.kind === "folder" && n.name === name,
				);
				if (!folder) {
					folder = { kind: "folder", name, children: [] };
					children.push(folder);
				}
				children = folder.children;
			}
		}
	}
	sortTree(root);
	return root;
}

function sortTree(nodes: TemplateTreeNode[]) {
	nodes.sort((a, b) => {
		if (a.kind !== b.kind) return a.kind === "folder" ? -1 : 1;
		return a.name.localeCompare(b.name);
	});
	for (const n of nodes) {
		if (n.kind === "folder") sortTree(n.children);
	}
}

function stripDocx(name: string): string {
	return name.replace(/\.docx$/i, "");
}

// ─── Tab types ──────────────────────────────────────────────────────────────

type PipelineTab = "templates" | "processes" | "team";

// ─── Main component ─────────────────────────────────────────────────────────

export default function PipelineHub() {
	const { settings, save } = useSettingsStore();
	const goToHub = useWorkflowStore((s) => s.goToHub);
	const goToSettings = useWorkflowStore((s) => s.goToSettings);
	const goToQuestionnaireEditor = useWorkflowStore(
		(s) => s.goToQuestionnaireEditor,
	);
	const {
		index: questionnaireIndex,
		loadIndex: loadQuestionnaireIndex,
		loadQuestionnaire,
		createQuestionnaire,
	} = useQuestionnaireStore();

	const [activeTab, setActiveTab] = useState<PipelineTab>("templates");
	const [templates, setTemplates] = useState<string[]>([]);
	const [referenceDocs, setReferenceDocs] = useState<ReferenceDocument[]>([]);
	const [loading, setLoading] = useState(false);
	const [selectedTemplate, setSelectedTemplate] = useState<string | null>(null);
	const [templateVars, setTemplateVars] = useState<VariableInfo[]>([]);
	const [loadingVars, setLoadingVars] = useState(false);

	// Load questionnaire index on mount
	useEffect(() => {
		loadQuestionnaireIndex();
	}, [loadQuestionnaireIndex]);

	// Load templates on mount
	useEffect(() => {
		if (settings.templates_dir) {
			setLoading(true);
			invoke<string[]>("list_templates", {
				templatesDir: settings.templates_dir,
			})
				.then(setTemplates)
				.catch((err) => console.error("Failed to load templates:", err))
				.finally(() => setLoading(false));
			invoke<ReferenceDocument[]>("list_reference_documents", {
				templatesDir: settings.templates_dir,
			})
				.then(setReferenceDocs)
				.catch((err) =>
					console.error("Failed to load reference documents:", err),
				);
		}
	}, [settings.templates_dir]);

	// Load variables when a template is selected
	useEffect(() => {
		if (!selectedTemplate || !settings.templates_dir) {
			setTemplateVars([]);
			return;
		}

		setLoadingVars(true);
		loadTemplateVariables(settings.templates_dir, selectedTemplate)
			.then(setTemplateVars)
			.catch((err) => {
				console.error("Failed to load template variables:", err);
				setTemplateVars([]);
			})
			.finally(() => setLoadingVars(false));
	}, [selectedTemplate, settings.templates_dir]);

	const tree = useMemo(() => buildTree(templates), [templates]);

	const openInEditor = useCallback(async () => {
		if (!selectedTemplate || !settings.templates_dir) return;
		const fullPath = `${settings.templates_dir}/${selectedTemplate}`;
		try {
			await invoke("open_file_in_os", { filePath: fullPath });
		} catch (err) {
			console.error("Failed to open template:", err);
		}
	}, [selectedTemplate, settings.templates_dir]);

	const openReference = useCallback(
		async (relPath: string) => {
			if (!settings.templates_dir) return;
			try {
				await invoke("open_file_in_os", {
					filePath: `${settings.templates_dir}/${relPath}`,
				});
			} catch (err) {
				console.error("Failed to open reference document:", err);
			}
		},
		[settings.templates_dir],
	);

	const pickTemplatesDir = async () => {
		const selected = await open({
			directory: true,
			title: "Select Templates Folder",
			defaultPath: settings.templates_dir ?? undefined,
		});
		if (selected) {
			await save({ templates_dir: selected });
		}
	};

	const replacementVars = templateVars.filter((v) => !v.is_conditional);
	const conditionalVars = templateVars.filter((v) => v.is_conditional);

	return (
		<div className="flex flex-col h-full">
			<PageHeader title="Pipeline Management" onBack={goToHub} />

			{/* Tab bar */}
			<div className="border-b border-base-300">
				<div className="flex">
					{(
						[
							{ id: "templates", label: "Templates" },
							{ id: "processes", label: "Processes" },
							{ id: "team", label: "Team" },
						] as const
					).map((tab) => (
						<button
							key={tab.id}
							type="button"
							className={`flex-1 py-3 text-sm font-medium border-b-2 transition-colors ${
								activeTab === tab.id
									? "border-primary text-primary"
									: "border-transparent text-base-content/50 hover:text-base-content/80 hover:bg-base-200/50"
							}`}
							onClick={() => setActiveTab(tab.id)}
						>
							{tab.label}
						</button>
					))}
				</div>
			</div>

			{/* Tab content */}
			<div className="flex-1 overflow-hidden">
				{activeTab === "templates" && (
					<TemplatesTab
						tree={tree}
						referenceDocs={referenceDocs}
						loading={loading}
						templatesDir={settings.templates_dir}
						selectedTemplate={selectedTemplate}
						templateVars={templateVars}
						replacementVars={replacementVars}
						conditionalVars={conditionalVars}
						loadingVars={loadingVars}
						onSelectTemplate={setSelectedTemplate}
						onOpenInEditor={openInEditor}
						onOpenReference={openReference}
						onPickTemplatesDir={pickTemplatesDir}
						questionnaireIndex={questionnaireIndex}
						questionnairesDir={settings.questionnaires_dir}
						onOpenQuestionnaire={async (id) => {
							await loadQuestionnaire(id);
							goToQuestionnaireEditor();
						}}
						onNewQuestionnaire={async () => {
							const def = await createQuestionnaire("New Questionnaire");
							await loadQuestionnaire(def.id);
							goToQuestionnaireEditor();
						}}
						onGoToSettings={goToSettings}
					/>
				)}
				{activeTab === "processes" && (
					<PlaceholderTab
						title="Processes"
						description="Define common client processes and document packages. Group templates into workflows that can be assigned to clients."
					/>
				)}
				{activeTab === "team" && (
					<PlaceholderTab
						title="Team"
						description="Manage team members, roles, and work assignments. Define who handles which parts of the client engagement process."
					/>
				)}
			</div>
		</div>
	);
}

// ─── Templates Tab ──────────────────────────────────────────────────────────

function TemplatesTab({
	tree,
	referenceDocs,
	loading,
	templatesDir,
	selectedTemplate,
	templateVars,
	replacementVars,
	conditionalVars,
	loadingVars,
	onSelectTemplate,
	onOpenInEditor,
	onOpenReference,
	onPickTemplatesDir,
	questionnaireIndex,
	questionnairesDir,
	onOpenQuestionnaire,
	onNewQuestionnaire,
	onGoToSettings,
}: {
	tree: TemplateTreeNode[];
	referenceDocs: ReferenceDocument[];
	loading: boolean;
	templatesDir: string | null;
	questionnairesDir: string | null;
	selectedTemplate: string | null;
	templateVars: VariableInfo[];
	replacementVars: VariableInfo[];
	conditionalVars: VariableInfo[];
	loadingVars: boolean;
	onSelectTemplate: (relPath: string | null) => void;
	onOpenInEditor: () => void;
	onOpenReference: (relPath: string) => void;
	onPickTemplatesDir: () => void;
	questionnaireIndex: QuestionnaireIndex | null;
	onOpenQuestionnaire: (id: string) => void;
	onNewQuestionnaire: () => void;
	onGoToSettings: () => void;
}) {
	if (!templatesDir) {
		return (
			<div className="flex flex-col items-center justify-center h-full gap-4 p-8">
				<p className="text-base-content/50 text-center max-w-sm">
					Set a templates folder to browse and manage your document templates.
				</p>
				<button
					type="button"
					className="btn btn-primary"
					onClick={onPickTemplatesDir}
				>
					Select Templates Folder
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
			{/* Left: template tree */}
			<div className="w-72 shrink-0 border-r border-base-300 overflow-y-auto p-4">
				{/* CLIENT SETUP section */}
				<SectionHeading className="mb-3">Client Setup</SectionHeading>
				<div className="flex flex-col gap-0.5 mb-4">
					{!questionnairesDir ? (
						<div className="px-3 py-2 space-y-2">
							<p className="text-xs text-base-content/40">
								Set a questionnaires folder in Settings to manage questionnaire
								definitions.
							</p>
							<button
								type="button"
								className="btn btn-ghost btn-xs text-base-content/50"
								onClick={onGoToSettings}
							>
								Open Settings
							</button>
						</div>
					) : (
						<>
							{questionnaireIndex?.questionnaires.map((q) => (
								<button
									key={q.id}
									type="button"
									className="btn btn-ghost btn-sm justify-start text-left w-full h-auto py-2 px-3 font-normal gap-2"
									onClick={() => onOpenQuestionnaire(q.id)}
								>
									<svg
										xmlns="http://www.w3.org/2000/svg"
										className="h-4 w-4 shrink-0 opacity-50"
										fill="none"
										viewBox="0 0 24 24"
										stroke="currentColor"
									>
										<title>Questionnaire</title>
										<path
											strokeLinecap="round"
											strokeLinejoin="round"
											strokeWidth={2}
											d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-6 9l2 2 4-4"
										/>
									</svg>
									<span className="truncate flex-1">{q.name}</span>
								</button>
							))}
							<button
								type="button"
								className="btn btn-ghost btn-sm justify-start text-left w-full h-auto py-2 px-3 font-normal gap-2 text-base-content/50"
								onClick={onNewQuestionnaire}
							>
								<svg
									xmlns="http://www.w3.org/2000/svg"
									className="h-4 w-4 shrink-0 opacity-50"
									fill="none"
									viewBox="0 0 24 24"
									stroke="currentColor"
								>
									<title>New</title>
									<path
										strokeLinecap="round"
										strokeLinejoin="round"
										strokeWidth={2}
										d="M12 4v16m8-8H4"
									/>
								</svg>
								<span className="truncate">New Questionnaire</span>
							</button>
						</>
					)}
				</div>

				<div className="border-b border-base-300 mb-4" />

				{/* TEMPLATE LIBRARY section */}
				<SectionHeading className="mb-3">Template Library</SectionHeading>
				{tree.length === 0 ? (
					<p className="text-sm text-base-content/50">No templates found.</p>
				) : (
					<div className="flex flex-col gap-0.5">
						{tree.map((node) => (
							<TreeItem
								key={node.kind === "file" ? node.relPath : node.name}
								node={node}
								selectedTemplate={selectedTemplate}
								onSelect={onSelectTemplate}
							/>
						))}
					</div>
				)}

				{referenceDocs.length > 0 && (
					<>
						<div className="border-b border-base-300 my-4" />
						<SectionHeading className="mb-3">
							Reference Documents
						</SectionHeading>
						<div className="flex flex-col gap-0.5">
							{referenceDocs.map((document) => (
								<button
									key={document.rel_path}
									type="button"
									className="btn btn-ghost btn-sm justify-start text-left w-full h-auto py-2 px-3 font-normal gap-2"
									title={document.rel_path}
									onClick={() => onOpenReference(document.rel_path)}
								>
									<svg
										xmlns="http://www.w3.org/2000/svg"
										className="h-4 w-4 shrink-0 opacity-50"
										fill="none"
										viewBox="0 0 24 24"
										stroke="currentColor"
									>
										<title>PDF</title>
										<path
											strokeLinecap="round"
											strokeLinejoin="round"
											strokeWidth={2}
											d="M7 21h10a2 2 0 002-2V9.414a1 1 0 00-.293-.707l-5.414-5.414A1 1 0 0012.586 3H7a2 2 0 00-2 2v14a2 2 0 002 2z"
										/>
									</svg>
									<span className="truncate flex-1">{document.name}</span>
								</button>
							))}
						</div>
					</>
				)}
			</div>

			{/* Right: template details */}
			<div className="flex-1 overflow-y-auto p-6">
				{selectedTemplate ? (
					<TemplateDetails
						relPath={selectedTemplate}
						variables={templateVars}
						replacementVars={replacementVars}
						conditionalVars={conditionalVars}
						loadingVars={loadingVars}
						onOpenInEditor={onOpenInEditor}
					/>
				) : (
					<div className="flex items-center justify-center h-full text-base-content/40 text-sm">
						Select a template to view details
					</div>
				)}
			</div>
		</div>
	);
}

// ─── Template Details Panel ─────────────────────────────────────────────────

function TemplateDetails({
	relPath,
	variables,
	replacementVars,
	conditionalVars,
	loadingVars,
	onOpenInEditor,
}: {
	relPath: string;
	variables: VariableInfo[];
	replacementVars: VariableInfo[];
	conditionalVars: VariableInfo[];
	loadingVars: boolean;
	onOpenInEditor: () => void;
}) {
	const filename = extractFilename(relPath);
	const folder = relPath.includes("/")
		? relPath.substring(0, relPath.lastIndexOf("/"))
		: "";

	return (
		<div className="max-w-lg">
			<h3 className="text-xl font-semibold mb-1">{stripDocx(filename)}</h3>
			{folder && (
				<p className="text-xs text-base-content/40 mb-4 font-mono">{folder}</p>
			)}

			<div className="flex gap-2 mb-6">
				<button
					type="button"
					className="btn btn-outline btn-sm"
					onClick={onOpenInEditor}
				>
					Open in Word
				</button>
			</div>

			{loadingVars ? (
				<div className="flex items-center gap-2 text-sm text-base-content/50">
					<span className="loading loading-spinner loading-xs" />
					Loading variables...
				</div>
			) : variables.length === 0 ? (
				<p className="text-sm text-base-content/50">
					No variables found in this template.
				</p>
			) : (
				<div className="space-y-6">
					{/* Summary */}
					<div className="flex gap-4">
						<div className="px-4 py-3 rounded-lg bg-base-100 border border-base-300 text-center">
							<div className="text-2xl font-bold">{variables.length}</div>
							<div className="text-xs text-base-content/50">
								Total Variables
							</div>
						</div>
						{replacementVars.length > 0 && (
							<div className="px-4 py-3 rounded-lg bg-base-100 border border-base-300 text-center">
								<div className="text-2xl font-bold">
									{replacementVars.length}
								</div>
								<div className="text-xs text-base-content/50">Replacement</div>
							</div>
						)}
						{conditionalVars.length > 0 && (
							<div className="px-4 py-3 rounded-lg bg-base-100 border border-base-300 text-center">
								<div className="text-2xl font-bold">
									{conditionalVars.length}
								</div>
								<div className="text-xs text-base-content/50">Conditional</div>
							</div>
						)}
					</div>

					{/* Replacement variables */}
					{replacementVars.length > 0 && (
						<div>
							<SectionHeading className="mb-2">
								Replacement Variables
							</SectionHeading>
							<div className="rounded-xl border border-base-300 divide-y divide-base-200">
								{replacementVars.map((v) => (
									<div key={v.display_name} className="px-3 py-2 text-sm">
										<span className="font-mono text-xs">
											{"{"}
											{v.display_name}
											{"}"}
										</span>
									</div>
								))}
							</div>
						</div>
					)}

					{/* Conditional variables */}
					{conditionalVars.length > 0 && (
						<div>
							<SectionHeading className="mb-2">
								Conditional Variables
							</SectionHeading>
							<div className="rounded-xl border border-base-300 divide-y divide-base-200">
								{conditionalVars.map((v) => (
									<div
										key={v.display_name}
										className="px-3 py-2 text-sm flex items-center gap-2"
									>
										<span className="badge badge-xs badge-warning">?</span>
										<span className="font-mono text-xs">{v.display_name}</span>
									</div>
								))}
							</div>
						</div>
					)}
				</div>
			)}
		</div>
	);
}

// ─── Tree Components ────────────────────────────────────────────────────────

function TreeItem({
	node,
	selectedTemplate,
	onSelect,
}: {
	node: TemplateTreeNode;
	selectedTemplate: string | null;
	onSelect: (relPath: string) => void;
}) {
	if (node.kind === "folder") {
		return (
			<TreeFolder
				node={node}
				selectedTemplate={selectedTemplate}
				onSelect={onSelect}
			/>
		);
	}
	return (
		<TreeFile
			node={node}
			isSelected={selectedTemplate === node.relPath}
			onSelect={onSelect}
		/>
	);
}

function TreeFolder({
	node,
	selectedTemplate,
	onSelect,
}: {
	node: TemplateTreeNode & { kind: "folder" };
	selectedTemplate: string | null;
	onSelect: (relPath: string) => void;
}) {
	const [expanded, setExpanded] = useState(false);

	return (
		<div>
			<button
				type="button"
				className="btn btn-ghost btn-sm justify-start text-left w-full h-auto py-2 px-3 font-medium gap-2"
				onClick={() => setExpanded(!expanded)}
			>
				<span className="text-xs opacity-40">
					{expanded ? "\u25BE" : "\u25B8"}
				</span>
				<svg
					xmlns="http://www.w3.org/2000/svg"
					className="h-4 w-4 shrink-0 opacity-50"
					fill="none"
					viewBox="0 0 24 24"
					stroke="currentColor"
				>
					<title>Folder</title>
					{expanded ? (
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
				<span className="truncate">{node.name}</span>
			</button>
			{expanded && (
				<div className="ml-4 border-l border-base-300 pl-1">
					{node.children.map((child) => (
						<TreeItem
							key={child.kind === "file" ? child.relPath : child.name}
							node={child}
							selectedTemplate={selectedTemplate}
							onSelect={onSelect}
						/>
					))}
				</div>
			)}
		</div>
	);
}

function TreeFile({
	node,
	isSelected,
	onSelect,
}: {
	node: TemplateTreeNode & { kind: "file" };
	isSelected: boolean;
	onSelect: (relPath: string) => void;
}) {
	return (
		<button
			type="button"
			className={`btn btn-ghost btn-sm justify-start text-left w-full h-auto py-2 px-3 font-normal gap-2 ${
				isSelected ? "bg-primary/10 text-primary font-medium" : ""
			}`}
			onClick={() => onSelect(node.relPath)}
		>
			<svg
				xmlns="http://www.w3.org/2000/svg"
				className="h-4 w-4 shrink-0 opacity-50"
				fill="none"
				viewBox="0 0 24 24"
				stroke="currentColor"
			>
				<title>Document</title>
				<path
					strokeLinecap="round"
					strokeLinejoin="round"
					strokeWidth={2}
					d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
				/>
			</svg>
			<span className="truncate">{stripDocx(node.name)}</span>
		</button>
	);
}

// ─── Placeholder Tab ────────────────────────────────────────────────────────

function PlaceholderTab({
	title,
	description,
}: {
	title: string;
	description: string;
}) {
	const lilyIcon = useLilyIcon();
	return (
		<div className="flex flex-col items-center justify-center h-full gap-4 p-8">
			<img src={lilyIcon} alt="" className="size-16 opacity-20" />
			<h3 className="text-xl font-semibold text-base-content/60">{title}</h3>
			<p className="text-sm text-base-content/40 text-center max-w-sm">
				{description}
			</p>
			<span className="badge badge-info badge-sm">In Development</span>
		</div>
	);
}
