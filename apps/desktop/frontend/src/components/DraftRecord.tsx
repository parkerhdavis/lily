// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState } from "react";
import { useWorkflowStore } from "@/stores/workflowStore";
import { extractFilename } from "@/utils/path";

interface DraftRecordProps {
	documentPath: string | null;
}

/** Shorten a SHA-256 for display while keeping the full value in a tooltip. */
function shortHash(hash: string | null | undefined): string {
	if (!hash) return "not recorded";
	return hash.slice(0, 12);
}

function formatTime(value: string | null | undefined): string {
	if (!value) return "not recorded";
	const date = new Date(value);
	if (Number.isNaN(date.getTime())) return value;
	return date.toLocaleString();
}

export default function DraftRecord({ documentPath }: DraftRecordProps) {
	const lilyFile = useWorkflowStore((state) => state.lilyFile);
	const [open, setOpen] = useState(false);

	if (!documentPath) return null;
	const record = lilyFile?.documents[extractFilename(documentPath)]?.provenance;
	const approval = record?.approval;
	const clauseCount = record ? Object.keys(record.clauses ?? {}).length : 0;

	return (
		<div className="mb-4 rounded-lg border border-base-300 bg-base-200/40">
			<button
				type="button"
				className="flex w-full items-center justify-between gap-2 px-3 py-2 text-left"
				onClick={() => setOpen((value) => !value)}
			>
				<span className="text-sm font-semibold">Draft Record</span>
				<span
					className={`badge badge-sm ${approval ? "badge-success" : "badge-ghost"}`}
				>
					{approval ? "Approved" : record ? "Generated" : "None"}
				</span>
			</button>
			{open && (
				<div className="space-y-2 px-3 pb-3 text-xs">
					{!record ? (
						<p className="text-base-content/50">
							This document has no draft record yet. Saving it renders the
							current plan and writes one.
						</p>
					) : (
						<>
							<div className="flex justify-between gap-2">
								<span className="text-base-content/60">Plan</span>
								<span className="truncate text-right">
									{record.plan_id} v{record.plan_version}
								</span>
							</div>
							<div className="flex justify-between gap-2">
								<span className="text-base-content/60">Generated</span>
								<span className="text-right">
									{formatTime(record.generated_at)}
								</span>
							</div>
							<div className="flex justify-between gap-2">
								<span className="text-base-content/60">Renderer</span>
								<span className="text-right">{record.renderer_version}</span>
							</div>
							<div className="flex justify-between gap-2">
								<span className="text-base-content/60">Clauses</span>
								<span className="text-right">{clauseCount}</span>
							</div>
							<div className="flex justify-between gap-2">
								<span className="text-base-content/60">Client Data</span>
								<span className="font-mono" title={record.client_data_hash}>
									{shortHash(record.client_data_hash)}
								</span>
							</div>
							<div className="flex justify-between gap-2">
								<span className="text-base-content/60">Template</span>
								<span className="font-mono" title={record.template_hash ?? ""}>
									{shortHash(record.template_hash)}
								</span>
							</div>
							<div className="flex justify-between gap-2">
								<span className="text-base-content/60">Document</span>
								<span className="font-mono" title={record.document_hash ?? ""}>
									{shortHash(record.document_hash)}
								</span>
							</div>
							{record.agent && (
								<>
									<div className="flex justify-between gap-2">
										<span className="text-base-content/60">Agent</span>
										<span className="truncate text-right">
											{record.agent.command}
											{record.agent.model
												? ` \u00b7 ${record.agent.model}`
												: ""}
										</span>
									</div>
									<div className="flex justify-between gap-2">
										<span className="text-base-content/60">Prompt</span>
										<span
											className="font-mono"
											title={record.agent.prompt_hash}
										>
											{shortHash(record.agent.prompt_hash)}
										</span>
									</div>
									<div className="flex justify-between gap-2">
										<span className="text-base-content/60">Flags</span>
										<span className="text-right">
											{record.agent.flags.total} total,{" "}
											{record.agent.flags.gaps} gaps,{" "}
											{record.agent.flags.warnings} warnings
										</span>
									</div>
								</>
							)}
							{approval && (
								<>
									<div className="flex justify-between gap-2">
										<span className="text-base-content/60">Approved</span>
										<span className="text-right">
											{formatTime(approval.approved_at)}
										</span>
									</div>
									<div className="flex justify-between gap-2">
										<span className="text-base-content/60">Approved By</span>
										<span className="truncate text-right">
											{approval.approved_by ?? "not recorded"}
										</span>
									</div>
								</>
							)}
						</>
					)}
				</div>
			)}
		</div>
	);
}
