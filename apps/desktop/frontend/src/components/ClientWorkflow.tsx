// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useMemo, useRef, useState } from "react";
import { useWorkflowStore } from "@/stores/workflowStore";
import type {
	DocumentStatus,
	LilyFile,
	RequiredDocument,
	WorkflowStage,
	WorkflowWaitingOn,
} from "@/types";

const ENGAGEMENT_LETTER =
	"01 - Engagement and Cover Letters/Engagement Letter Template.docx";

const STAGES: { value: WorkflowStage; label: string }[] = [
	{ value: "intake", label: "Intake" },
	{ value: "questionnaire", label: "Questionnaire" },
	{ value: "engagement", label: "Engagement letter" },
	{ value: "signed", label: "Signed" },
	{ value: "drafting", label: "Drafting" },
	{ value: "review", label: "Review" },
	{ value: "complete", label: "Complete" },
];

const STATUS_LABELS: Record<DocumentStatus, string> = {
	not_started: "Not started",
	drafting: "Drafting",
	reviewing: "Reviewing",
	complete: "Complete",
	executed: "Executed",
};

function statusBadge(status: DocumentStatus): string {
	switch (status) {
		case "executed":
			return "badge-success";
		case "complete":
			return "badge-info";
		case "reviewing":
			return "badge-warning";
		case "drafting":
			return "badge-primary";
		default:
			return "badge-ghost";
	}
}

function suggestedStage(
	required: RequiredDocument[],
	questionnaireComplete: boolean,
	engagement: RequiredDocument | undefined,
): WorkflowStage {
	if (required.length === 0) return "intake";
	if (!questionnaireComplete) return "questionnaire";
	if (!engagement || engagement.status !== "executed") return "engagement";
	const selected = required.filter(
		(document) => document.template_rel_path !== ENGAGEMENT_LETTER,
	);
	if (
		selected.length > 0 &&
		selected.every((document) => document.status === "not_started")
	) {
		return "signed";
	}
	if (
		required.some(
			(document) =>
				document.status === "not_started" ||
				document.status === "drafting" ||
				document.status === "reviewing",
		)
	) {
		return "drafting";
	}
	if (required.every((document) => document.status === "executed")) {
		return "complete";
	}
	return "review";
}

export default function ClientWorkflow({
	lilyFile,
	questionnaireStats,
	templatesDir,
	onOpenQuestionnaire,
	onStartAddDocument,
	onOpenDocument,
	onAddMultipleDocuments,
}: {
	lilyFile: LilyFile;
	questionnaireStats: { total: number; filled: number };
	templatesDir: string | null;
	onOpenQuestionnaire: () => void;
	onStartAddDocument: () => void;
	onOpenDocument: (filename: string, templateRelPath: string) => void;
	onAddMultipleDocuments: (
		templateRelPaths: string[],
		templatesDir: string,
	) => Promise<void>;
}) {
	const setWorkflowStage = useWorkflowStore((state) => state.setWorkflowStage);
	const setWorkflowWaitingOn = useWorkflowStore(
		(state) => state.setWorkflowWaitingOn,
	);
	const reloadLilyFile = useWorkflowStore((state) => state.reloadLilyFile);
	const workingDir = useWorkflowStore((state) => state.workingDir);
	const templates = useWorkflowStore((state) => state.templates);
	const setRequiredDocuments = useWorkflowStore(
		(state) => state.setRequiredDocuments,
	);
	const [selectedTemplates, setSelectedTemplates] = useState<Set<string>>(
		new Set(),
	);
	const selectionDialogRef = useRef<HTMLDialogElement>(null);

	const required = lilyFile.required_documents ?? [];
	const openSelection = useCallback(() => {
		setSelectedTemplates(
			new Set(required.map((document) => document.template_rel_path)),
		);
		selectionDialogRef.current?.showModal();
	}, [required]);
	const lockedPaths = useMemo(
		() =>
			new Set(
				required
					.filter((document) => document.document_filename)
					.map((document) => document.template_rel_path),
			),
		[required],
	);
	const templateGroups = useMemo(() => {
		const groups = new Map<string, string[]>();
		for (const path of templates) {
			const separator = path.lastIndexOf("/");
			const folder = separator >= 0 ? path.slice(0, separator) : "Templates";
			const list = groups.get(folder) ?? [];
			list.push(path);
			groups.set(folder, list);
		}
		return [...groups.entries()].sort(([left], [right]) =>
			left.localeCompare(right),
		);
	}, [templates]);
	const toggleTemplate = (path: string) => {
		setSelectedTemplates((current) => {
			const next = new Set(current);
			if (next.has(path)) next.delete(path);
			else next.add(path);
			return next;
		});
	};
	const saveSelection = useCallback(async () => {
		await setRequiredDocuments([...selectedTemplates]);
		selectionDialogRef.current?.close();
	}, [selectedTemplates, setRequiredDocuments]);
	const engagement = required.find(
		(document) => document.template_rel_path === ENGAGEMENT_LETTER,
	);
	const questionnaireComplete =
		questionnaireStats.total > 0 &&
		questionnaireStats.filled >= questionnaireStats.total;
	const suggested = suggestedStage(required, questionnaireComplete, engagement);

	const next = useMemo(() => {
		if (required.length === 0) {
			return {
				label: "Select documents",
				description:
					"Choose the documents this engagement needs so the questionnaire and letter can be scoped.",
				action: openSelection,
				disabled: false,
			};
		}
		if (!questionnaireComplete) {
			return {
				label: "Open questionnaire",
				description:
					"The questionnaire is scoped to the selected documents. Complete it before the engagement letter.",
				action: onOpenQuestionnaire,
				disabled: false,
			};
		}
		if (!engagement) {
			return {
				label: "Add engagement letter",
				description:
					"The engagement letter summarizes the selected documents and the estate distribution.",
				action: () => {
					if (templatesDir) {
						void onAddMultipleDocuments([ENGAGEMENT_LETTER], templatesDir);
					}
				},
				disabled: !templatesDir,
			};
		}
		if (engagement.status !== "executed") {
			return {
				label: "Mark engagement letter signed",
				description:
					"Record the signed engagement letter to move the client into drafting.",
				action: () => {
					if (!workingDir) return;
					void invoke("update_required_document_status", {
						workingDir,
						documentId: engagement.id,
						status: "executed",
					}).then(() => reloadLilyFile());
				},
				disabled: !workingDir,
			};
		}
		const nextDraft = required.find(
			(document) =>
				document.status === "not_started" && document.template_rel_path,
		);
		if (nextDraft) {
			return {
				label: "Draft next document",
				description:
					"Draft the selected documents from the questionnaire and plan library.",
				action: () => {
					if (templatesDir) {
						void onAddMultipleDocuments(
							[nextDraft.template_rel_path],
							templatesDir,
						);
					}
				},
				disabled: !templatesDir,
			};
		}
		if (
			!required.every(
				(document) =>
					document.status === "complete" || document.status === "executed",
			)
		) {
			const ready = required.find(
				(document) =>
					document.document_filename &&
					document.status !== "complete" &&
					document.status !== "executed",
			);
			return {
				label: ready ? "Open next document" : "Review documents",
				description:
					"Review the drafted documents and mark them complete when they are ready.",
				action: ready?.document_filename
					? () =>
							onOpenDocument(ready.document_filename!, ready.template_rel_path)
					: onStartAddDocument,
				disabled: false,
			};
		}
		return {
			label: "Complete",
			description: "Every selected document is complete or executed.",
			action: undefined,
			disabled: true,
		};
	}, [
		required,
		questionnaireComplete,
		engagement,
		templatesDir,
		workingDir,
		reloadLilyFile,
		openSelection,
		onStartAddDocument,
		onOpenQuestionnaire,
		onAddMultipleDocuments,
		onOpenDocument,
	]);

	return (
		<>
			<div className="rounded-xl border border-base-300 bg-base-100 shadow-[0_4px_16px_rgba(0,0,0,0.25)] p-5">
				<div className="flex flex-wrap items-start justify-between gap-3">
					<div>
						<div className="font-semibold text-base">Engagement</div>
						<p className="text-sm text-base-content/50 mt-0.5">
							{next.description}
						</p>
					</div>
					<div className="flex flex-wrap items-center justify-end gap-2">
						<button
							type="button"
							className="btn btn-ghost btn-xs"
							onClick={openSelection}
						>
							Select documents
						</button>
						<span className="text-xs text-base-content/40">Waiting on</span>
						<select
							className="select select-bordered select-sm"
							value={lilyFile.workflow_waiting_on ?? "none"}
							onChange={(event) =>
								void setWorkflowWaitingOn(
									event.target.value as WorkflowWaitingOn,
								)
							}
						>
							<option value="none">No one</option>
							<option value="attorney">Attorney</option>
							<option value="client">Client</option>
						</select>
						<span className="text-xs text-base-content/40">Stage</span>
						<select
							className="select select-bordered select-sm"
							value={lilyFile.workflow_stage ?? "intake"}
							onChange={(event) =>
								void setWorkflowStage(event.target.value as WorkflowStage)
							}
						>
							{STAGES.map((stage) => (
								<option key={stage.value} value={stage.value}>
									{stage.label}
								</option>
							))}
						</select>
						{lilyFile.workflow_stage !== suggested && (
							<button
								type="button"
								className="btn btn-ghost btn-xs"
								onClick={() => void setWorkflowStage(suggested)}
								title={`Set the stage to ${suggested}`}
							>
								Use suggested
							</button>
						)}
					</div>
				</div>

				<div className="mt-4 flex flex-wrap items-center justify-between gap-3 border-t border-base-300 pt-3">
					<div className="text-xs text-base-content/50">
						{required.length} selected ·{" "}
						{
							required.filter(
								(document) =>
									document.status === "complete" ||
									document.status === "executed",
							).length
						}{" "}
						complete
					</div>
					{next.action && (
						<button
							type="button"
							className="btn btn-primary btn-sm"
							onClick={next.action}
							disabled={next.disabled}
						>
							{next.label}
						</button>
					)}
				</div>

				{required.length > 0 && (
					<div className="mt-4 space-y-1 border-t border-base-300 pt-3">
						{required.map((document) => (
							<div
								key={document.id}
								className="flex items-center justify-between gap-2 rounded-lg bg-base-200/40 px-2 py-1.5 text-xs"
							>
								<span className="min-w-0 truncate">
									{document.template_rel_path.replace(/^.*\//, "")}
								</span>
								<div className="flex shrink-0 items-center gap-1">
									<span
										className={`badge badge-sm ${statusBadge(document.status)}`}
									>
										{STATUS_LABELS[document.status]}
									</span>
									{document.document_filename ? (
										<button
											type="button"
											className="btn btn-ghost btn-xs"
											onClick={() =>
												onOpenDocument(
													document.document_filename!,
													document.template_rel_path,
												)
											}
										>
											Open
										</button>
									) : (
										<button
											type="button"
											className="btn btn-ghost btn-xs"
											onClick={() => {
												if (templatesDir) {
													void onAddMultipleDocuments(
														[document.template_rel_path],
														templatesDir,
													);
												}
											}}
											disabled={!templatesDir}
										>
											Draft
										</button>
									)}
								</div>
							</div>
						))}
					</div>
				)}
			</div>

			<dialog ref={selectionDialogRef} className="modal">
				<div className="modal-box max-w-2xl">
					<h3 className="text-lg font-semibold">Select documents</h3>
					<p className="mt-1 text-xs text-base-content/60">
						The selection scopes the questionnaire, the engagement letter, and
						the drafting queue. Documents already drafted stay in the plan.
					</p>
					<div className="mt-3 max-h-[60vh] space-y-3 overflow-auto">
						{templateGroups.length === 0 && (
							<p className="text-sm text-base-content/50">
								No templates found. Check the templates folder in Settings.
							</p>
						)}
						{templateGroups.map(([folder, paths]) => (
							<div key={folder}>
								<div className="text-xs font-semibold text-base-content/50">
									{folder}
								</div>
								<div className="mt-1 space-y-1">
									{paths.map((path) => {
										const name = path.slice(path.lastIndexOf("/") + 1);
										const locked = lockedPaths.has(path);
										return (
											<label
												key={path}
												className={`flex items-center gap-2 rounded px-2 py-1 text-sm ${
													locked ? "opacity-60" : "hover:bg-base-200/60"
												}`}
											>
												<input
													type="checkbox"
													className="checkbox checkbox-sm"
													checked={selectedTemplates.has(path)}
													disabled={locked}
													onChange={() => toggleTemplate(path)}
												/>
												<span className="truncate">{name}</span>
											</label>
										);
									})}
								</div>
							</div>
						))}
					</div>
					<div className="modal-action">
						<button
							type="button"
							className="btn btn-sm"
							onClick={() => selectionDialogRef.current?.close()}
						>
							Cancel
						</button>
						<button
							type="button"
							className="btn btn-primary btn-sm"
							onClick={() => void saveSelection()}
						>
							Save selection
						</button>
					</div>
				</div>
				<form method="dialog" className="modal-backdrop">
					<button type="submit">close</button>
				</form>
			</dialog>
		</>
	);
}
