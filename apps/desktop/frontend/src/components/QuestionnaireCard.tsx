// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useRef, useState } from "react";
import { useToastStore } from "@/stores/toastStore";
import type { LilyFile, QuestionnaireUpdateReport } from "@/types";

interface QuestionnaireCardProps {
	workingDir: string;
	lilyFile: LilyFile;
	onReload: () => Promise<void>;
}

function shortHash(hash: string): string {
	return hash.slice(0, 12);
}

export default function QuestionnaireCard({
	workingDir,
	lilyFile,
	onReload,
}: QuestionnaireCardProps) {
	const dialogRef = useRef<HTMLDialogElement>(null);
	const [report, setReport] = useState<QuestionnaireUpdateReport | null>(null);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const record = lilyFile.questionnaire_pdf;
	const storedPath = record ? `${workingDir}/${record.filename}` : null;

	const openStored = async () => {
		if (!storedPath) return;
		try {
			await invoke("open_file_in_os", { filePath: storedPath });
		} catch (err) {
			useToastStore
				.getState()
				.addToast("error", `Failed to open questionnaire: ${String(err)}`);
		}
	};

	const chooseAndPreview = async () => {
		const selected = await open({
			multiple: false,
			title: record
				? "Re-import Questionnaire PDF"
				: "Import Questionnaire PDF",
			defaultPath: storedPath ?? undefined,
			filters: [{ name: "PDF", extensions: ["pdf"] }],
		});
		if (!selected || Array.isArray(selected)) return;
		setBusy(true);
		setError(null);
		try {
			const result = await invoke<QuestionnaireUpdateReport>(
				"preview_questionnaire_update",
				{ workingDir, pdfPath: selected },
			);
			setReport(result);
			dialogRef.current?.showModal();
		} catch (err) {
			setError(String(err));
		} finally {
			setBusy(false);
		}
	};

	const apply = async () => {
		if (!report) return;
		setBusy(true);
		setError(null);
		try {
			const result = await invoke<QuestionnaireUpdateReport>(
				"apply_questionnaire_update",
				{ workingDir, pdfPath: report.preview.source_path },
			);
			await onReload();
			setReport(null);
			dialogRef.current?.close();
			useToastStore
				.getState()
				.addToast(
					"success",
					`Questionnaire imported: ${result.contacts_added} contacts added, ${result.documents_added} documents proposed`,
				);
		} catch (err) {
			setError(String(err));
		} finally {
			setBusy(false);
		}
	};

	return (
		<>
			<div className="rounded-xl border border-base-300 bg-base-100 p-4">
				<div className="flex flex-wrap items-start justify-between gap-3">
					<div className="min-w-0">
						<div className="font-semibold text-sm">Questionnaire PDF</div>
						{record ? (
							<>
								<p className="text-sm text-base-content/60 truncate">
									{record.filename}
								</p>
								<p className="text-xs text-base-content/40 mt-0.5">
									Imported {new Date(record.imported_at).toLocaleString()} ·{" "}
									<span className="font-mono" title={record.sha256}>
										{shortHash(record.sha256)}
									</span>{" "}
									· map v{record.field_map_version}
								</p>
							</>
						) : (
							<p className="text-sm text-base-content/50 mt-0.5">
								No questionnaire PDF is attached to this client.
							</p>
						)}
						{error && <p className="text-xs text-error mt-1">{error}</p>}
					</div>
					<div className="flex shrink-0 gap-2">
						{record && (
							<button
								type="button"
								className="btn btn-ghost btn-sm"
								onClick={() => void openStored()}
							>
								Open PDF
							</button>
						)}
						<button
							type="button"
							className="btn btn-outline btn-sm"
							onClick={() => void chooseAndPreview()}
							disabled={busy}
						>
							{busy ? (
								<span className="loading loading-spinner loading-xs" />
							) : null}
							{record ? "Re-import" : "Import Questionnaire"}
						</button>
					</div>
				</div>
			</div>

			<dialog ref={dialogRef} className="modal">
				<div className="modal-box max-w-2xl">
					<h3 className="text-lg font-semibold">Import Questionnaire</h3>
					{report && (
						<div className="mt-3 space-y-3 text-sm">
							<p className="text-base-content/60">
								{report.preview.source_filename} ·{" "}
								{report.preview.clients
									.map((client) =>
										[client.first_name, client.last_name]
											.filter(Boolean)
											.join(" "),
									)
									.join(" and ")}
							</p>
							<div className="grid grid-cols-2 gap-2 rounded-lg bg-base-200/50 p-3 text-xs">
								<span>Variables added</span>
								<span className="text-right">
									{report.variables_added.length}
								</span>
								<span>Variables changed</span>
								<span className="text-right">
									{report.variables_changed.length}
								</span>
								<span>Contacts added</span>
								<span className="text-right">{report.contacts_added}</span>
								<span>Contacts updated</span>
								<span className="text-right">{report.contacts_updated}</span>
								<span>Documents added</span>
								<span className="text-right">{report.documents_added}</span>
								<span>Documents updated</span>
								<span className="text-right">{report.documents_updated}</span>
							</div>
							{report.variables_changed.length > 0 && (
								<p className="text-xs text-base-content/50">
									Changed: {report.variables_changed.join(", ")}
								</p>
							)}
							{report.preview.warnings.length > 0 && (
								<div className="alert alert-warning items-start text-xs">
									<ul className="list-disc space-y-0.5 pl-4">
										{report.preview.warnings.map((warning) => (
											<li key={warning}>{warning}</li>
										))}
									</ul>
								</div>
							)}
							{error && <p className="text-xs text-error">{error}</p>}
							<p className="text-xs text-base-content/50">
								Applying updates the derived client data, contacts, roles, and
								proposed documents. Existing drafts and statuses are kept.
							</p>
						</div>
					)}
					<div className="modal-action">
						<button
							type="button"
							className="btn btn-ghost btn-sm"
							onClick={() => dialogRef.current?.close()}
						>
							Cancel
						</button>
						<button
							type="button"
							className="btn btn-primary btn-sm"
							onClick={() => void apply()}
							disabled={busy || !report}
						>
							{busy ? (
								<span className="loading loading-spinner loading-xs" />
							) : null}
							Apply Import
						</button>
					</div>
					<form method="dialog" className="modal-backdrop">
						<button type="submit">close</button>
					</form>
				</div>
			</dialog>
		</>
	);
}
