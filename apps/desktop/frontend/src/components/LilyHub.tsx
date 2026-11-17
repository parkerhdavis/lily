// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import AppSwitcher from "@/components/ui/AppSwitcher";
import SectionHeading from "@/components/ui/SectionHeading";
import { useLilyIcon } from "@/hooks/useLilyIcon";
import { useSettingsStore } from "@/stores/settingsStore";
import { useWorkflowStore } from "@/stores/workflowStore";
import type { ClientSummary, PersistedNavEntry } from "@/types";
import { extractFolderName } from "@/utils/path";

/** Steps that operate on an individual client (require a working dir). */
const CLIENT_STEPS = new Set(["clients", "select-template"]);

function describeLastStep(step: string, dirName: string | null): string {
	if (step === "clients") return "Clients";
	if (CLIENT_STEPS.has(step) && dirName) return dirName;
	if (step === "pipeline") return "Pipeline";
	if (step === "app-settings") return "Settings";
	return "";
}

/** Format a timestamp as relative time. */
function relativeTime(ms: number): string {
	const diff = Date.now() - ms;
	const minutes = Math.floor(diff / 60000);
	if (minutes < 1) return "Just now";
	if (minutes < 60) return `${minutes}m ago`;
	const hours = Math.floor(minutes / 60);
	if (hours < 24) return `${hours}h ago`;
	const days = Math.floor(hours / 24);
	if (days === 1) return "Yesterday";
	if (days < 7) return `${days}d ago`;
	return new Date(ms).toLocaleDateString(undefined, {
		month: "short",
		day: "numeric",
	});
}

export default function LilyHub() {
	const { settings, save, addRecentDirectory } = useSettingsStore();
	const lilyIcon = useLilyIcon();
	const {
		setWorkingDir,
		loadTemplates,
		goToSettings,
		goToPipeline,
		goToClients,
	} = useWorkflowStore();

	// Load a lightweight preview summary for the resume card
	const [resumePreview, setResumePreview] = useState<ClientSummary | null>(
		null,
	);
	useEffect(() => {
		const { last_step, last_working_dir } = settings;
		if (!last_step || !last_working_dir || !CLIENT_STEPS.has(last_step)) return;
		invoke<ClientSummary[]>("load_client_summaries", {
			directories: [last_working_dir],
		})
			.then((summaries) => setResumePreview(summaries[0] ?? null))
			.catch(() => setResumePreview(null));
	}, [settings.last_step, settings.last_working_dir]);

	const selectClient = async (dir: string) => {
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
			title: "Open Client Folder",
			defaultPath: settings.last_working_dir ?? undefined,
		});
		if (selected) {
			await selectClient(selected);
		}
	};

	const resumeLastSession = async () => {
		const { last_step, last_working_dir } = settings;
		if (!last_step) return;

		if (last_step === "clients") {
			goToClients();
		} else if (CLIENT_STEPS.has(last_step) && last_working_dir) {
			await selectClient(last_working_dir);
		} else if (last_step === "pipeline") {
			goToPipeline();
		} else if (last_step === "app-settings") {
			goToSettings();
		}
	};

	const navigateToEntry = async (entry: PersistedNavEntry) => {
		if (entry.step === "clients") {
			goToClients();
		} else if (CLIENT_STEPS.has(entry.step) && entry.working_dir) {
			await selectClient(entry.working_dir);
		} else if (entry.step === "pipeline") {
			goToPipeline();
		} else if (entry.step === "app-settings") {
			goToSettings();
		}
	};

	// Derive recent pages from persisted nav history, excluding the resume entry
	const recentPages = useMemo(() => {
		const history = settings.navigation_history ?? [];
		const { last_step, last_working_dir } = settings;
		return history
			.filter((e) => {
				// Only steps this version can navigate to; older persisted
				// history may name a step it cannot open.
				if (e.step !== "pipeline" && e.step !== "app-settings") {
					if (!CLIENT_STEPS.has(e.step)) return false;
				}
				// Exclude the "resume" entry (already shown separately)
				if (
					last_step &&
					e.step === last_step &&
					(e.working_dir ?? null) === (last_working_dir ?? null)
				)
					return false;
				return true;
			})
			.slice(0, 5);
	}, [
		settings.navigation_history,
		settings.last_step,
		settings.last_working_dir,
	]);

	const dirName = (path: string) => extractFolderName(path);

	const resumeLabel = settings.last_step
		? describeLastStep(
				settings.last_step,
				settings.last_working_dir ? dirName(settings.last_working_dir) : null,
			)
		: "";

	return (
		<div className="flex flex-col h-full">
			{/* Header bar */}
			<header className="flex items-center gap-4 px-6 py-4 border-b border-base-300 bg-base-100">
				<img src={lilyIcon} alt="Lily" className="size-9 drop-shadow-sm" />
				<div className="flex-1 min-w-0">
					<h1 className="text-xl font-bold tracking-tight">Lily</h1>
					<p className="text-xs text-base-content/40">
						Legal Drafting and Client Management — Developed by{" "}
						<a
							href="https://github.com/parkerhdavis"
							target="_blank"
							rel="noopener noreferrer"
							className="underline hover:text-base-content/60"
						>
							Parker H. Davis
						</a>
					</p>
				</div>
				<button
					type="button"
					className="btn btn-ghost btn-sm"
					onClick={pickClientFolder}
					title="Open Client Folder"
				>
					<svg
						xmlns="http://www.w3.org/2000/svg"
						viewBox="0 0 20 20"
						fill="currentColor"
						className="size-5 opacity-60"
					>
						<title>Open Folder</title>
						<path
							fillRule="evenodd"
							d="M3.75 3A1.75 1.75 0 0 0 2 4.75v3.26a3.235 3.235 0 0 1 1.75-.51h12.5c.644 0 1.245.188 1.75.51V6.75A1.75 1.75 0 0 0 16.25 5h-4.836a.25.25 0 0 1-.177-.073L9.823 3.513A1.75 1.75 0 0 0 8.586 3H3.75ZM3.75 9A1.75 1.75 0 0 0 2 10.75v4.5c0 .966.784 1.75 1.75 1.75h12.5A1.75 1.75 0 0 0 18 15.25v-4.5A1.75 1.75 0 0 0 16.25 9H3.75Z"
							clipRule="evenodd"
						/>
					</svg>
				</button>
				<AppSwitcher />
			</header>

			{/* Main content */}
			<div className="flex-1 overflow-y-auto flex items-center justify-center">
				<div className="max-w-4xl w-full px-6 py-8 space-y-6">
					{/* Branding */}
					<div className="flex flex-col items-center gap-2 pb-6">
						<img src={lilyIcon} alt="Lily" className="size-14 drop-shadow-sm" />
						<span className="text-2xl font-bold tracking-tight">Lily</span>
					</div>

					{/* Module panels */}
					<div className="grid grid-cols-3 gap-5">
						{/* Clients */}
						<button
							type="button"
							className="card bg-base-100 border border-base-300 shadow-[0_4px_16px_rgba(0,0,0,0.25)] hover:shadow-[0_8px_28px_rgba(0,0,0,0.35)] hover:bg-base-200/50 hover:border-base-content/20 transition-all cursor-pointer text-left"
							onClick={goToClients}
						>
							<div className="card-body p-8 gap-3">
								<div className="flex items-center gap-3">
									<svg
										xmlns="http://www.w3.org/2000/svg"
										viewBox="0 0 20 20"
										fill="currentColor"
										className="size-6 text-base-content/50"
									>
										<title>Clients</title>
										<path d="M7 8a3 3 0 1 0 0-6 3 3 0 0 0 0 6ZM14.5 9a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5ZM1.615 16.428a1.224 1.224 0 0 1-.569-1.175 6.002 6.002 0 0 1 11.908 0c.058.467-.172.92-.57 1.174A9.953 9.953 0 0 1 7 18a9.953 9.953 0 0 1-5.385-1.572ZM14.5 16h-.106c.07-.297.088-.611.048-.933a7.47 7.47 0 0 0-1.588-3.755 4.502 4.502 0 0 1 5.874 2.636.818.818 0 0 1-.36.98A7.465 7.465 0 0 1 14.5 16Z" />
									</svg>
									<span className="font-semibold text-lg">Clients</span>
								</div>
								<p className="text-sm text-base-content/50">
									Manage client details and docs and track progress
								</p>
							</div>
						</button>

						{/* Pipeline */}
						<button
							type="button"
							className="card bg-base-100 border border-base-300 shadow-[0_4px_16px_rgba(0,0,0,0.25)] hover:shadow-[0_8px_28px_rgba(0,0,0,0.35)] hover:bg-base-200/50 hover:border-base-content/20 transition-all cursor-pointer text-left"
							onClick={goToPipeline}
						>
							<div className="card-body p-8 gap-3">
								<div className="flex items-center gap-3">
									<svg
										xmlns="http://www.w3.org/2000/svg"
										viewBox="0 0 20 20"
										fill="currentColor"
										className="size-6 text-base-content/50"
									>
										<title>Pipeline</title>
										<path d="M3.75 3A1.75 1.75 0 0 0 2 4.75v3.26a3.235 3.235 0 0 1 1.75-.51h12.5c.644 0 1.245.188 1.75.51V6.75A1.75 1.75 0 0 0 16.25 5h-4.836a.25.25 0 0 1-.177-.073L9.823 3.513A1.75 1.75 0 0 0 8.586 3H3.75ZM3.75 9A1.75 1.75 0 0 0 2 10.75v4.5c0 .966.784 1.75 1.75 1.75h12.5A1.75 1.75 0 0 0 18 15.25v-4.5A1.75 1.75 0 0 0 16.25 9H3.75Z" />
									</svg>
									<span className="font-semibold text-lg">Pipeline</span>
								</div>
								<p className="text-sm text-base-content/50">
									Configure templates, processes, and team settings
								</p>
							</div>
						</button>

						{/* Settings */}
						<button
							type="button"
							className="card bg-base-100 border border-base-300 shadow-[0_4px_16px_rgba(0,0,0,0.25)] hover:shadow-[0_8px_28px_rgba(0,0,0,0.35)] hover:bg-base-200/50 hover:border-base-content/20 transition-all cursor-pointer text-left"
							onClick={goToSettings}
						>
							<div className="card-body p-8 gap-3">
								<div className="flex items-center gap-3">
									<svg
										xmlns="http://www.w3.org/2000/svg"
										viewBox="0 0 20 20"
										fill="currentColor"
										className="size-6 text-base-content/50"
									>
										<title>Settings</title>
										<path
											fillRule="evenodd"
											d="M7.84 1.804A1 1 0 0 1 8.82 1h2.36a1 1 0 0 1 .98.804l.331 1.652a6.993 6.993 0 0 1 1.929 1.115l1.598-.54a1 1 0 0 1 1.186.447l1.18 2.044a1 1 0 0 1-.205 1.251l-1.267 1.113a7.047 7.047 0 0 1 0 2.228l1.267 1.113a1 1 0 0 1 .206 1.25l-1.18 2.045a1 1 0 0 1-1.187.447l-1.598-.54a6.993 6.993 0 0 1-1.929 1.115l-.33 1.652a1 1 0 0 1-.98.804H8.82a1 1 0 0 1-.98-.804l-.331-1.652a6.993 6.993 0 0 1-1.929-1.115l-1.598.54a1 1 0 0 1-1.186-.447l-1.18-2.044a1 1 0 0 1 .205-1.251l1.267-1.114a7.05 7.05 0 0 1 0-2.227L1.821 7.773a1 1 0 0 1-.206-1.25l1.18-2.045a1 1 0 0 1 1.187-.447l1.598.54A6.992 6.992 0 0 1 7.51 3.456l.33-1.652ZM10 13a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z"
											clipRule="evenodd"
										/>
									</svg>
									<span className="font-semibold text-lg">Settings</span>
								</div>
								<p className="text-sm text-base-content/50">
									Adjust your app settings for themes, file paths, and more
								</p>
							</div>
						</button>
					</div>

					{/* Pick up where you left off */}
					{resumeLabel && (
						<button
							type="button"
							className="w-full card bg-primary/10 border border-primary/20 shadow-[0_4px_16px_rgba(0,0,0,0.25)] hover:shadow-[0_8px_28px_rgba(0,0,0,0.35)] hover:bg-primary/15 hover:border-primary/30 transition-all cursor-pointer text-left"
							onClick={resumeLastSession}
						>
							<div className="card-body p-6 gap-3">
								<div className="flex items-center gap-2.5">
									<svg
										xmlns="http://www.w3.org/2000/svg"
										viewBox="0 0 20 20"
										fill="currentColor"
										className="size-5 text-primary"
									>
										<title>Resume</title>
										<path
											fillRule="evenodd"
											d="M2 10a8 8 0 1 1 16 0 8 8 0 0 1-16 0Zm6.39-2.908a.75.75 0 0 1 .766.027l3.5 2.25a.75.75 0 0 1 0 1.262l-3.5 2.25A.75.75 0 0 1 8 12.25v-4.5a.75.75 0 0 1 .39-.658Z"
											clipRule="evenodd"
										/>
									</svg>
									<span className="font-semibold text-base text-primary">
										Pick up where you left off
									</span>
								</div>

								{/* Preview card */}
								<div className="rounded-lg bg-base-100/60 border border-primary/10 px-4 py-3">
									<div className="font-medium text-sm">{resumeLabel}</div>
									{resumePreview && (
										<div className="text-xs text-base-content/50 mt-1">
											{resumePreview.total_documents} document
											{resumePreview.total_documents !== 1 ? "s" : ""}
											{resumePreview.contacts_count > 0 &&
												` \u00B7 ${resumePreview.contacts_count} contact${resumePreview.contacts_count !== 1 ? "s" : ""}`}
											{resumePreview.required_documents.length > 0 && (
												<>
													{" \u00B7 "}
													{
														resumePreview.required_documents.filter(
															(r) =>
																r.status === "complete" ||
																r.status === "executed",
														).length
													}
													/{resumePreview.required_documents.length} complete
												</>
											)}
										</div>
									)}
									{!resumePreview &&
										settings.last_step &&
										!CLIENT_STEPS.has(settings.last_step) && (
											<p className="text-xs text-base-content/40 mt-0.5">
												{settings.last_step === "pipeline"
													? "Templates, processes, and workspace configuration"
													: settings.last_step === "app-settings"
														? "Theme, templates folder, and preferences"
														: ""}
											</p>
										)}
								</div>
							</div>
						</button>
					)}

					{/* Recent pages */}
					{recentPages.length > 0 && (
						<div>
							<SectionHeading className="mb-3">Recent</SectionHeading>
							<div className="rounded-xl border border-base-300 bg-base-100 shadow-[0_4px_16px_rgba(0,0,0,0.25)] divide-y divide-base-200 overflow-hidden">
								{recentPages.map((entry, i) => (
									<button
										key={`${entry.step}-${entry.working_dir}-${i}`}
										type="button"
										className="w-full text-left px-5 py-3 hover:bg-base-200/60 transition-colors flex items-center justify-between gap-3"
										onClick={() => navigateToEntry(entry)}
									>
										<span className="text-sm font-medium truncate">
											{entry.label}
										</span>
										<span className="text-xs text-base-content/30 shrink-0">
											{relativeTime(entry.visited_at)}
										</span>
									</button>
								))}
							</div>
						</div>
					)}
				</div>
			</div>
		</div>
	);
}
