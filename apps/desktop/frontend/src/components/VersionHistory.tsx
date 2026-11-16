// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import DiffView from "@/components/DiffView";
import { useToastStore } from "@/stores/toastStore";
import { useWorkflowStore } from "@/stores/workflowStore";
import type {
	DocumentDiffLine,
	DocumentVersion,
	SignedArtifact,
} from "@/types";
import { extractFilename } from "@/utils/path";

interface VersionHistoryProps {
	documentPath: string | null;
}

export default function VersionHistory({ documentPath }: VersionHistoryProps) {
	const [versions, setVersions] = useState<DocumentVersion[]>([]);
	const [error, setError] = useState<string | null>(null);
	const [open, setOpen] = useState(false);
	const [busy, setBusy] = useState(false);
	const [diff, setDiff] = useState<DocumentDiffLine[] | null>(null);
	const [diffVersion, setDiffVersion] = useState<DocumentVersion | null>(null);
	const diffDialogRef = useRef<HTMLDialogElement>(null);
	const restoreVersion = useWorkflowStore((state) => state.restoreVersion);
	const lilyFile = useWorkflowStore((state) => state.lilyFile);
	const workingDir = useWorkflowStore((state) => state.workingDir);

	const load = useCallback(async () => {
		if (!documentPath) return;
		try {
			const result = await invoke<DocumentVersion[]>("list_document_versions", {
				docxPath: documentPath,
			});
			setVersions(result);
			setError(null);
		} catch (err) {
			setError(String(err));
		}
	}, [documentPath]);

	useEffect(() => {
		void load();
	}, [load]);

	if (!documentPath) return null;

	const signedArtifacts =
		lilyFile?.required_documents?.find(
			(document) =>
				document.document_filename === extractFilename(documentPath),
		)?.signed_artifacts ?? [];

	const openSigned = async (artifact: SignedArtifact) => {
		if (!workingDir) return;
		try {
			await invoke("open_file_in_os", {
				filePath: `${workingDir}/${artifact.filename}`,
			});
		} catch (err) {
			useToastStore
				.getState()
				.addToast("error", `Failed to open signed PDF: ${String(err)}`);
		}
	};

	const compare = async (version: DocumentVersion) => {
		setBusy(true);
		try {
			const lines = await invoke<DocumentDiffLine[]>("diff_document_versions", {
				docxPath: documentPath,
				snapshotPath: version.path,
			});
			setDiff(lines);
			setDiffVersion(version);
			setError(null);
			diffDialogRef.current?.showModal();
		} catch (err) {
			setError(String(err));
		} finally {
			setBusy(false);
		}
	};

	const restore = async (version: DocumentVersion) => {
		if (
			!window.confirm(
				`Restore the version from ${version.modified}? The current version is saved first.`,
			)
		) {
			return;
		}
		setBusy(true);
		try {
			await restoreVersion(version.path);
			await load();
		} catch (err) {
			setError(String(err));
		} finally {
			setBusy(false);
		}
	};

	return (
		<div className="mb-4 rounded-lg border border-base-300 bg-base-200/40">
			<button
				type="button"
				className="flex w-full items-center justify-between gap-2 px-3 py-2 text-left"
				onClick={() => setOpen((value) => !value)}
			>
				<span className="text-sm font-semibold">Version history</span>
				<span className="badge badge-sm badge-ghost">{versions.length}</span>
			</button>
			{open && (
				<div className="space-y-2 px-3 pb-3 text-xs">
					{error && <p className="text-error">{error}</p>}
					{signedArtifacts.length > 0 && (
						<div className="rounded border border-base-300 bg-base-100 px-2 py-1.5">
							<p className="font-semibold">Signed Copy</p>
							{signedArtifacts.map((artifact) => (
								<div
									key={artifact.filename}
									className="mt-1 flex items-center justify-between gap-2"
								>
									<div className="min-w-0">
										<p className="truncate">{artifact.filename}</p>
										<p className="text-base-content/50">
											{new Date(artifact.attached_at).toLocaleString()} ·{" "}
											{Math.max(1, Math.round(artifact.size / 1024))} KB ·{" "}
											<span className="font-mono" title={artifact.sha256}>
												{artifact.sha256.slice(0, 12)}
											</span>
										</p>
									</div>
									<button
										type="button"
										className="btn btn-ghost btn-xs shrink-0"
										onClick={() => void openSigned(artifact)}
									>
										Open
									</button>
								</div>
							))}
						</div>
					)}
					{versions.length === 0 && (
						<p className="text-base-content/50">No saved versions yet.</p>
					)}
					{versions.slice(0, 10).map((version) => (
						<div
							key={version.path}
							className="flex items-center justify-between gap-2"
						>
							<div className="min-w-0">
								<p className="truncate">{version.modified}</p>
								<p className="text-base-content/50">
									{Math.max(1, Math.round(version.size / 1024))} KB
								</p>
							</div>
							<div className="flex shrink-0 gap-1">
								<button
									type="button"
									className="btn btn-ghost btn-xs"
									disabled={busy}
									onClick={() => void compare(version)}
								>
									Compare
								</button>
								<button
									type="button"
									className="btn btn-ghost btn-xs"
									disabled={busy}
									onClick={() => void restore(version)}
								>
									Restore
								</button>
							</div>
						</div>
					))}
					{versions.length > 10 && (
						<p className="text-base-content/50">
							Showing the newest 10 of {versions.length}.
						</p>
					)}
					<p className="text-base-content/50">
						Restoring replaces the document file. Saving afterwards re-renders
						from the current client data.
					</p>
					<button
						type="button"
						className="btn btn-ghost btn-xs px-1"
						onClick={() => void load()}
					>
						Refresh
					</button>
				</div>
			)}

			<dialog ref={diffDialogRef} className="modal">
				<div className="modal-box max-w-3xl">
					<h3 className="text-lg font-semibold">
						Changes from {diffVersion?.modified ?? "the selected version"}
					</h3>
					<p className="mt-1 text-xs text-base-content/60">
						Lines marked − were in that version; lines marked + are in the
						current document.
					</p>
					<div className="mt-3">
						<DiffView diff={diff ?? []} />
					</div>
					<div className="modal-action">
						<button
							type="button"
							className="btn btn-sm"
							onClick={() => diffDialogRef.current?.close()}
						>
							Close
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
