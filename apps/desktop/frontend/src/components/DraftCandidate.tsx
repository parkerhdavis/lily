// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useRef, useState } from "react";
import DiffView from "@/components/DiffView";
import { useWorkflowStore } from "@/stores/workflowStore";
import type { PlanCandidateReport } from "@/types";

interface DraftCandidateProps {
	planPath: string | null;
	documentPath: string | null;
	skeletonPath: string | null;
}

/** Render the plan to a temporary candidate and show it with a diff before
 *  the attorney approves it as the document. */
export default function DraftCandidate({
	planPath,
	documentPath,
	skeletonPath,
}: DraftCandidateProps) {
	const [open, setOpen] = useState(false);
	const [candidate, setCandidate] = useState<PlanCandidateReport | null>(null);
	const [tab, setTab] = useState<"candidate" | "diff">("candidate");
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const dialogRef = useRef<HTMLDialogElement>(null);
	const workingDir = useWorkflowStore((state) => state.workingDir);
	const variableValues = useWorkflowStore((state) => state.variableValues);
	const dirty = useWorkflowStore((state) => state.dirty);
	const refreshPreview = useWorkflowStore((state) => state.refreshPreview);

	const render = useCallback(async () => {
		if (!planPath || !documentPath || !skeletonPath) return;
		setBusy(true);
		setError(null);
		try {
			// The renderer reads the .lily file, so persist the current
			// variable values first. This does not touch the document.
			if (workingDir && dirty) {
				await invoke("save_client_variables", {
					workingDir,
					variableValues,
				});
			}
			const result = await invoke<PlanCandidateReport>(
				"render_plan_candidate",
				{
					docxPath: documentPath,
					planPath,
					skeletonPath,
				},
			);
			setCandidate(result);
			setTab("candidate");
			dialogRef.current?.showModal();
		} catch (err) {
			setError(String(err));
		} finally {
			setBusy(false);
		}
	}, [planPath, documentPath, skeletonPath, workingDir, variableValues, dirty]);

	const discard = useCallback(async () => {
		if (!candidate) return;
		try {
			await invoke("discard_plan_candidate", {
				candidatePath: candidate.candidate_path,
			});
		} catch {
			// The temporary file is disposable; closing the dialog is enough.
		}
		setCandidate(null);
		dialogRef.current?.close();
	}, [candidate]);

	const approve = useCallback(async () => {
		if (!candidate || !documentPath) return;
		if (
			!window.confirm(
				"Replace the current document with this candidate? The current version is saved in history.",
			)
		) {
			return;
		}
		setBusy(true);
		setError(null);
		try {
			await invoke("approve_plan_candidate", {
				docxPath: documentPath,
				candidatePath: candidate.candidate_path,
			});
			await refreshPreview();
			useWorkflowStore.setState({ dirty: false });
			setCandidate(null);
			dialogRef.current?.close();
		} catch (err) {
			setError(String(err));
		} finally {
			setBusy(false);
		}
	}, [candidate, documentPath, refreshPreview]);

	if (!planPath || !documentPath || !skeletonPath) return null;

	return (
		<div className="mb-4 rounded-lg border border-base-300 bg-base-200/40">
			<button
				type="button"
				className="flex w-full items-center justify-between gap-2 px-3 py-2 text-left"
				onClick={() => setOpen((value) => !value)}
			>
				<span className="text-sm font-semibold">Draft candidate</span>
				<span className="badge badge-sm badge-ghost">
					{candidate ? "ready" : "review"}
				</span>
			</button>
			{open && (
				<div className="space-y-2 px-3 pb-3 text-xs">
					<p className="text-base-content/60">
						Render the current plan and client data without overwriting the
						document. Review the candidate and its diff before approving it as
						the draft.
					</p>
					{error && <p className="text-error">{error}</p>}
					<button
						type="button"
						className="btn btn-primary btn-xs"
						onClick={() => void render()}
						disabled={busy}
					>
						{busy ? (
							<span className="loading loading-spinner loading-xs" />
						) : (
							"Review candidate"
						)}
					</button>
				</div>
			)}

			<dialog
				ref={dialogRef}
				className="modal"
				onClose={() => {
					if (candidate) void discard();
				}}
			>
				<div className="modal-box max-w-5xl">
					<h3 className="text-lg font-semibold">Draft candidate</h3>
					{candidate && (
						<p className="mt-1 text-xs text-base-content/60">
							{candidate.plan_id} v{candidate.plan_version} ·{" "}
							{candidate.blocks_rendered} block
							{candidate.blocks_rendered === 1 ? "" : "s"} rendered
						</p>
					)}
					{candidate && candidate.warnings.length > 0 && (
						<div className="alert alert-warning mt-3 py-2 text-xs">
							<ul className="list-disc pl-4">
								{candidate.warnings.map((warning) => (
									<li key={warning}>{warning}</li>
								))}
							</ul>
						</div>
					)}
					{error && <p className="mt-2 text-sm text-error">{error}</p>}
					<div role="tablist" className="tabs tabs-boxed mt-3 w-fit">
						<button
							type="button"
							role="tab"
							className={`tab ${tab === "candidate" ? "tab-active" : ""}`}
							onClick={() => setTab("candidate")}
						>
							Candidate
						</button>
						<button
							type="button"
							role="tab"
							className={`tab ${tab === "diff" ? "tab-active" : ""}`}
							onClick={() => setTab("diff")}
						>
							Diff
						</button>
					</div>
					{tab === "candidate" ? (
						<div
							className="mt-3 max-h-[60vh] overflow-auto rounded border border-base-300 bg-base-100 p-6 prose prose-sm max-w-none"
							dangerouslySetInnerHTML={{
								__html: candidate?.candidate_html ?? "",
							}}
						/>
					) : (
						<div className="mt-3">
							<DiffView
								diff={candidate?.diff ?? []}
								emptyMessage="The candidate matches the current document."
							/>
						</div>
					)}
					<div className="modal-action">
						<button
							type="button"
							className="btn btn-sm"
							onClick={() => void discard()}
							disabled={busy}
						>
							Discard
						</button>
						<button
							type="button"
							className="btn btn-primary btn-sm"
							onClick={() => void approve()}
							disabled={busy || !candidate}
						>
							Approve as draft
						</button>
					</div>
				</div>
				<form method="dialog" className="modal-backdrop">
					<button type="submit">close</button>
				</form>
			</dialog>
		</div>
	);
}
