// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { useSettingsStore } from "@/stores/settingsStore";
import { useToastStore } from "@/stores/toastStore";
import { useWorkflowStore } from "@/stores/workflowStore";
import type {
	CreatedQuestionnaireClient,
	QuestionnairePreview,
	QuestionnairePreviewClient,
} from "@/types";
import { extractFolderName } from "@/utils/path";

interface QuestionnaireImportDialogProps {
	open: boolean;
	onClose: () => void;
	onCreated: (directory: string) => void;
}

function clientName(client: QuestionnairePreviewClient): string {
	return [client.first_name, client.last_name].filter(Boolean).join(" ");
}

export default function QuestionnaireImportDialog({
	open: isOpen,
	onClose,
	onCreated,
}: QuestionnaireImportDialogProps) {
	const dialogRef = useRef<HTMLDialogElement>(null);
	const settings = useSettingsStore((state) => state.settings);
	const goToSettings = useWorkflowStore((state) => state.goToSettings);
	const [pdfPath, setPdfPath] = useState<string | null>(null);
	const [preview, setPreview] = useState<QuestionnairePreview | null>(null);
	const [folderName, setFolderName] = useState("");
	const [parentDir, setParentDir] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	useEffect(() => {
		const dialog = dialogRef.current;
		if (!dialog) return;
		if (isOpen && !dialog.open) {
			setError(null);
			setPdfPath(null);
			setPreview(null);
			setBusy(false);
			dialog.showModal();
		} else if (!isOpen && dialog.open) {
			dialog.close();
		}
	}, [isOpen]);

	const choosePdf = async () => {
		const selected = await open({
			multiple: false,
			title: "Choose Questionnaire PDF",
			filters: [{ name: "PDF", extensions: ["pdf"] }],
		});
		if (!selected || Array.isArray(selected)) return;
		setBusy(true);
		setError(null);
		try {
			const result = await invoke<QuestionnairePreview>(
				"preview_pdf_questionnaire",
				{ pdfPath: selected },
			);
			setPreview(result);
			setPdfPath(selected);
			setFolderName(result.suggested_folder_name);
			setParentDir(settings.client_library_dirs?.[0] ?? null);
		} catch (err) {
			setError(String(err));
		} finally {
			setBusy(false);
		}
	};

	const create = async () => {
		if (!preview || !pdfPath || !parentDir || !folderName.trim()) return;
		setBusy(true);
		setError(null);
		try {
			const created = await invoke<CreatedQuestionnaireClient>(
				"create_client_from_questionnaire",
				{ parentDir, pdfPath, folderName: folderName.trim() },
			);
			useToastStore
				.getState()
				.addToast(
					"success",
					`Created ${created.folder_name} with ${created.required_document_count} proposed documents`,
				);
			onCreated(created.directory);
			onClose();
		} catch (err) {
			setError(String(err));
		} finally {
			setBusy(false);
		}
	};

	const people = preview?.people.length ?? 0;
	const documents = preview?.required_document_count ?? 0;
	const warnings = preview?.warnings ?? [];

	return (
		<dialog
			ref={dialogRef}
			className="modal"
			onClose={onClose}
			onCancel={onClose}
		>
			<div className="modal-box max-w-4xl">
				<h3 className="text-lg font-semibold">New Client from Questionnaire</h3>
				<p className="mt-1 text-sm text-base-content/60">
					Import the filled PDF, review what Lily read, then create the client
					folder. The PDF stays the source for later re-imports.
				</p>

				{!preview ? (
					<div className="mt-6 flex flex-col items-center gap-3 rounded-xl border border-dashed border-base-300 p-8">
						<p className="text-sm text-base-content/60 text-center max-w-md">
							Choose a filled Estate Planning Questionnaire PDF. Lily reads the
							AcroForm fields directly and does not modify the PDF.
						</p>
						<button
							type="button"
							className="btn btn-primary btn-sm"
							onClick={() => void choosePdf()}
							disabled={busy}
						>
							{busy ? (
								<span className="loading loading-spinner loading-xs" />
							) : null}
							Choose PDF
						</button>
					</div>
				) : (
					<div className="mt-4 space-y-4">
						<div className="flex flex-wrap items-center justify-between gap-2 rounded-lg bg-base-200/50 px-3 py-2 text-xs">
							<span className="truncate font-mono">
								{preview.source_filename}
							</span>
							<span className="text-base-content/50">
								{preview.field_count} fields · {people} people · {documents}{" "}
								proposed documents
							</span>
						</div>

						<div className="grid gap-3 sm:grid-cols-2">
							{preview.clients.map((client) => (
								<div
									key={client.key}
									className="rounded-lg border border-base-300 bg-base-100 p-3"
								>
									<p className="text-xs font-medium text-base-content/50">
										Client {client.key.toUpperCase()}
									</p>
									<p className="font-semibold">{clientName(client)}</p>
									<p className="text-xs text-base-content/60">
										{[client.date_of_birth, client.phone, client.email]
											.filter(Boolean)
											.join(" · ")}
									</p>
									<p className="text-xs text-base-content/50">
										{[client.city, client.state, client.zip]
											.filter(Boolean)
											.join(", ")}
									</p>
								</div>
							))}
						</div>

						{warnings.length > 0 && (
							<div className="alert alert-warning items-start text-xs">
								<div>
									<p className="font-semibold">
										{warnings.length} item{warnings.length === 1 ? "" : "s"}{" "}
										need attention
									</p>
									<ul className="mt-1 list-disc space-y-0.5 pl-4">
										{warnings.map((warning) => (
											<li key={warning}>{warning}</li>
										))}
									</ul>
								</div>
							</div>
						)}

						<div className="grid gap-3 sm:grid-cols-2">
							<label className="form-control">
								<span className="label-text text-xs font-medium mb-1">
									Folder Name
								</span>
								<input
									type="text"
									className="input input-bordered input-sm"
									value={folderName}
									onChange={(event) => setFolderName(event.target.value)}
								/>
							</label>
							<label className="form-control">
								<span className="label-text text-xs font-medium mb-1">
									Client Library
								</span>
								{settings.client_library_dirs?.length ? (
									<select
										className="select select-bordered select-sm"
										value={parentDir ?? ""}
										onChange={(event) => setParentDir(event.target.value)}
									>
										{settings.client_library_dirs.map((dir) => (
											<option key={dir} value={dir}>
												{extractFolderName(dir)}
											</option>
										))}
									</select>
								) : (
									<button
										type="button"
										className="btn btn-outline btn-sm"
										onClick={() => {
											onClose();
											goToSettings();
										}}
									>
										Add a Client Library
									</button>
								)}
							</label>
						</div>

						{error && <p className="text-sm text-error">{error}</p>}
					</div>
				)}

				<div className="modal-action">
					<button
						type="button"
						className="btn btn-ghost btn-sm"
						onClick={onClose}
					>
						Cancel
					</button>
					{preview && (
						<button
							type="button"
							className="btn btn-primary btn-sm"
							onClick={() => void create()}
							disabled={busy || !parentDir || !folderName.trim()}
						>
							{busy ? (
								<span className="loading loading-spinner loading-xs" />
							) : null}
							Create Client
						</button>
					)}
				</div>
				<form method="dialog" className="modal-backdrop">
					<button type="submit">close</button>
				</form>
			</div>
		</dialog>
	);
}
