// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { useSettingsStore } from "@/stores/settingsStore";
import { useToastStore } from "@/stores/toastStore";
import { useWorkflowStore } from "@/stores/workflowStore";
import type { Contact, CopyFromSpouseResult, LilyFile } from "@/types";
import { extractFolderName } from "@/utils/path";

type Stage =
	| { kind: "pick" }
	| {
			kind: "loading-source";
			sourceDir: string;
	  }
	| {
			kind: "review";
			sourceDir: string;
			sourceLily: LilyFile;
			spouseCandidates: Contact[];
			selectedSpouseId: string | null;
	  }
	| {
			kind: "running";
			sourceDir: string;
			selectedSpouseId: string | null;
	  };

export default function CopyFromSpouseDialog({
	targetDir,
	onClose,
}: {
	targetDir: string;
	onClose: () => void;
}) {
	const dialogRef = useRef<HTMLDialogElement>(null);
	const [stage, setStage] = useState<Stage>({ kind: "pick" });
	const [error, setError] = useState<string | null>(null);

	const templatesDir = useSettingsStore((s) => s.settings.templates_dir);
	const reloadLilyFile = useWorkflowStore((s) => s.reloadLilyFile);

	useEffect(() => {
		dialogRef.current?.showModal();
	}, []);

	const closeAndReset = useCallback(() => {
		dialogRef.current?.close();
		onClose();
	}, [onClose]);

	const handlePickFolder = useCallback(async () => {
		setError(null);
		const selected = await openDialog({
			directory: true,
			title: "Select Spouse's Folder",
			defaultPath: targetDir,
		});
		if (!selected) return;
		if (selected === targetDir) {
			setError("Pick a different folder than the current client.");
			return;
		}

		setStage({ kind: "loading-source", sourceDir: selected });
		try {
			const sourceLily = await invoke<LilyFile>("load_lily_file_cmd", {
				workingDir: selected,
			});
			const spouses = (sourceLily.contacts ?? []).filter(
				(c) => c.relationship.toLowerCase() === "spouse",
			);
			setStage({
				kind: "review",
				sourceDir: selected,
				sourceLily,
				spouseCandidates: spouses,
				selectedSpouseId: spouses.length === 1 ? spouses[0].id : null,
			});
		} catch (err) {
			setError(`Could not read source .lily: ${err}`);
			setStage({ kind: "pick" });
		}
	}, [targetDir]);

	const handleConfirm = useCallback(async () => {
		if (stage.kind !== "review") return;
		if (!templatesDir) {
			setError("Templates folder is not configured. Open Settings first.");
			return;
		}
		const { sourceDir, selectedSpouseId, spouseCandidates } = stage;
		if (spouseCandidates.length > 1 && !selectedSpouseId) {
			setError("Pick which contact to promote to client.");
			return;
		}

		setStage({ kind: "running", sourceDir, selectedSpouseId });
		setError(null);
		try {
			const result = await invoke<CopyFromSpouseResult>(
				"copy_from_spouse_lily",
				{
					targetDir,
					sourceDir,
					spouseContactId: selectedSpouseId,
					templatesDir,
				},
			);
			useWorkflowStore.setState({ lilyFile: result.lily });
			await reloadLilyFile();

			const toast = useToastStore.getState().addToast;
			if (result.warnings.length > 0) {
				toast(
					"warning",
					`Copied ${result.copied_documents.length} document${
						result.copied_documents.length === 1 ? "" : "s"
					}, ${result.warnings.length} warning${
						result.warnings.length === 1 ? "" : "s"
					}`,
				);
				for (const w of result.warnings) {
					toast("warning", w);
				}
			} else {
				toast(
					"success",
					`Copied from spouse: ${result.copied_documents.length} document${
						result.copied_documents.length === 1 ? "" : "s"
					} created`,
				);
			}
			closeAndReset();
		} catch (err) {
			setError(String(err));
			// Return to review so the user can retry or pick again.
			setStage({
				kind: "review",
				sourceDir: stage.sourceDir,
				sourceLily: (stage as { sourceLily?: LilyFile }).sourceLily as LilyFile,
				spouseCandidates: (stage as { spouseCandidates?: Contact[] })
					.spouseCandidates as Contact[],
				selectedSpouseId: stage.selectedSpouseId,
			});
		}
	}, [stage, targetDir, templatesDir, reloadLilyFile, closeAndReset]);

	const renderBody = () => {
		switch (stage.kind) {
			case "pick":
				return (
					<>
						<p className="text-base-content/70 mb-4">
							Pick the folder of this client's spouse. We'll create this
							client's .lily by promoting the spouse's chosen contact to client,
							copying client data and contacts, and recreating documents from
							template with swapped values.
						</p>
						<div className="alert alert-info text-sm mb-4">
							Manual edits made to the spouse's .docx files (outside the
							variable system) won't carry over — documents are rebuilt from
							template.
						</div>
						{error && (
							<div className="alert alert-error text-sm mb-4">{error}</div>
						)}
						<div className="modal-action">
							<button
								type="button"
								className="btn btn-ghost btn-sm"
								onClick={closeAndReset}
							>
								Cancel
							</button>
							<button
								type="button"
								className="btn btn-primary btn-sm"
								onClick={handlePickFolder}
							>
								Pick Spouse's Folder
							</button>
						</div>
					</>
				);
			case "loading-source":
				return (
					<div className="flex items-center justify-center py-8">
						<span className="loading loading-spinner loading-md" />
					</div>
				);
			case "review": {
				const { sourceDir, sourceLily, spouseCandidates, selectedSpouseId } =
					stage;
				const docCount = Object.keys(sourceLily.documents ?? {}).length;
				const reqCount = (sourceLily.required_documents ?? []).length;
				const contactCount = (sourceLily.contacts ?? []).length;
				const noSpouse = spouseCandidates.length === 0;
				const multipleSpouses = spouseCandidates.length > 1;
				const promoted = spouseCandidates.find(
					(c) => c.id === selectedSpouseId,
				);

				return (
					<>
						<dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm mb-4">
							<dt className="text-base-content/60">Source folder</dt>
							<dd className="font-mono break-all">
								{extractFolderName(sourceDir)}
							</dd>
							<dt className="text-base-content/60">Source has</dt>
							<dd>
								{docCount} document{docCount === 1 ? "" : "s"} · {contactCount}{" "}
								contact{contactCount === 1 ? "" : "s"} · {reqCount} required
								document{reqCount === 1 ? "" : "s"}
							</dd>
							<dt className="text-base-content/60">Promote to client</dt>
							<dd>
								{noSpouse ? (
									<span className="text-error">
										No "Spouse" contact found in source.
									</span>
								) : multipleSpouses ? (
									<select
										className="select select-bordered select-sm w-full"
										value={selectedSpouseId ?? ""}
										onChange={(e) =>
											setStage({
												...stage,
												selectedSpouseId: e.target.value || null,
											})
										}
									>
										<option value="">Pick one…</option>
										{spouseCandidates.map((c) => (
											<option key={c.id} value={c.id}>
												{c.full_name}
											</option>
										))}
									</select>
								) : (
									<span className="font-medium">{promoted?.full_name}</span>
								)}
							</dd>
						</dl>

						<div className="alert alert-info text-sm mb-4">
							Manual edits to the spouse's .docx files won't carry over —
							documents are rebuilt from template.
						</div>

						{error && (
							<div className="alert alert-error text-sm mb-4">{error}</div>
						)}

						<div className="modal-action">
							<button
								type="button"
								className="btn btn-ghost btn-sm"
								onClick={closeAndReset}
							>
								Cancel
							</button>
							<button
								type="button"
								className="btn btn-ghost btn-sm"
								onClick={handlePickFolder}
							>
								Pick Different Folder
							</button>
							<button
								type="button"
								className="btn btn-primary btn-sm"
								onClick={handleConfirm}
								disabled={noSpouse || (multipleSpouses && !selectedSpouseId)}
							>
								Copy from Spouse
							</button>
						</div>
					</>
				);
			}
			case "running":
				return (
					<div className="flex flex-col items-center justify-center py-8 gap-3">
						<span className="loading loading-spinner loading-md" />
						<p className="text-sm text-base-content/60">
							Copying and rebuilding documents…
						</p>
					</div>
				);
		}
	};

	return (
		<dialog
			ref={dialogRef}
			className="modal"
			onClick={(e) => {
				if (e.target === dialogRef.current && stage.kind !== "running") {
					closeAndReset();
				}
			}}
		>
			<div className="modal-box max-w-lg">
				<h3 className="text-lg font-bold mb-3">Copy from Spouse</h3>
				{renderBody()}
			</div>
		</dialog>
	);
}
