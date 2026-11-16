// SPDX-License-Identifier: AGPL-3.0-or-later
import { open } from "@tauri-apps/plugin-dialog";
import PageHeader from "@/components/ui/PageHeader";
import SectionHeading from "@/components/ui/SectionHeading";
import { useLilyIcon } from "@/hooks/useLilyIcon";
import { useQuestionnaireStore } from "@/stores/questionnaireStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { useToastStore } from "@/stores/toastStore";
import { useWorkflowStore } from "@/stores/workflowStore";

export default function AppSettings() {
	const { settings, save, zoomIn, zoomOut, zoomReset } = useSettingsStore();
	const goToHub = useWorkflowStore((s) => s.goToHub);
	const lilyIcon = useLilyIcon();
	const theme = settings.theme;
	const isDark = theme === "dark";
	const zoom = settings.zoom ?? 100;
	const footerSize = settings.footer_size ?? "medium";

	const setTheme = (value: "light" | "dark") => {
		document.documentElement.setAttribute("data-theme", value);
		save({ theme: value });
	};

	const pickTemplatesDir = async () => {
		const selected = await open({
			directory: true,
			title: "Select Template Library Path",
			defaultPath: settings.templates_dir ?? undefined,
		});
		if (selected) {
			save({ templates_dir: selected });
		}
	};

	const addClientLibraryDir = async () => {
		const selected = await open({
			directory: true,
			title: "Select Client Library Path",
		});
		if (selected) {
			const dirs = settings.client_library_dirs ?? [];
			if (!dirs.includes(selected)) {
				save({ client_library_dirs: [...dirs, selected] });
			}
		}
	};

	const removeClientLibraryDir = (dir: string) => {
		const dirs = (settings.client_library_dirs ?? []).filter((d) => d !== dir);
		save({ client_library_dirs: dirs });
	};

	const pickQuestionnairesDir = async () => {
		const selected = await open({
			directory: true,
			title: "Select Questionnaire Library Path",
			defaultPath: settings.questionnaires_dir ?? undefined,
		});
		if (selected) {
			await save({ questionnaires_dir: selected });
			// Migrate any existing questionnaires from old config-dir storage
			try {
				const count = await useQuestionnaireStore
					.getState()
					.migrateQuestionnaires();
				if (count > 0) {
					useToastStore
						.getState()
						.addToast(
							"success",
							`Migrated ${count} questionnaire${count > 1 ? "s" : ""} to new folder`,
						);
				}
			} catch {
				// Migration is best-effort — folder is still set
			}
		}
	};

	return (
		<div className="flex flex-col h-full">
			<PageHeader title="Settings" onBack={goToHub} />

			<div className="flex-1 overflow-y-auto p-8">
				<div className="max-w-lg mx-auto space-y-8">
					{/* Appearance */}
					<section>
						<SectionHeading className="mb-4">Appearance</SectionHeading>
						<div className="flex items-center justify-between p-4 rounded-xl border border-base-300 bg-base-100">
							<span className="text-sm font-medium">Theme</span>
							<div className="flex rounded-lg overflow-hidden border border-base-300">
								<button
									type="button"
									className={`px-4 py-1.5 text-xs font-semibold transition-colors ${
										!isDark
											? "bg-primary text-primary-content"
											: "bg-base-200 text-base-content/40 hover:bg-base-300"
									}`}
									onClick={() => setTheme("light")}
								>
									Light
								</button>
								<button
									type="button"
									className={`px-4 py-1.5 text-xs font-semibold transition-colors ${
										isDark
											? "bg-primary text-primary-content"
											: "bg-base-200 text-base-content/40 hover:bg-base-300"
									}`}
									onClick={() => setTheme("dark")}
								>
									Dark
								</button>
							</div>
						</div>
					</section>

					{/* Zoom */}
					<section>
						<SectionHeading className="mb-4">Zoom</SectionHeading>
						<div className="flex items-center justify-between p-4 rounded-xl border border-base-300 bg-base-100">
							<div>
								<p className="text-sm font-medium">Interface Scale</p>
								<p className="text-xs text-base-content/40 mt-0.5">
									Ctrl+= / Ctrl+- / Ctrl+0
								</p>
							</div>
							<div className="flex items-center gap-2">
								<button
									type="button"
									className="btn btn-outline btn-xs btn-square"
									onClick={zoomOut}
									disabled={zoom <= 50}
								>
									-
								</button>
								<button
									type="button"
									className="btn btn-ghost btn-xs min-w-12"
									onClick={zoomReset}
								>
									{zoom}%
								</button>
								<button
									type="button"
									className="btn btn-outline btn-xs btn-square"
									onClick={zoomIn}
									disabled={zoom >= 200}
								>
									+
								</button>
							</div>
						</div>
					</section>

					{/* Footer size */}
					<section>
						<div className="flex items-center justify-between p-4 rounded-xl border border-base-300 bg-base-100">
							<p className="text-sm font-medium">Status Bar Size</p>
							<div className="flex rounded-lg overflow-hidden border border-base-300">
								{(
									[
										{ value: "small", label: "Small" },
										{
											value: "medium",
											label: "Medium",
										},
										{ value: "large", label: "Large" },
									] as const
								).map((opt) => (
									<button
										key={opt.value}
										type="button"
										className={`px-3 py-1.5 text-xs font-semibold transition-colors ${
											footerSize === opt.value
												? "bg-primary text-primary-content"
												: "bg-base-200 text-base-content/40 hover:bg-base-300"
										}`}
										onClick={() =>
											save({
												footer_size: opt.value,
											})
										}
									>
										{opt.label}
									</button>
								))}
							</div>
						</div>
					</section>

					{/* Autosave */}
					<section>
						<div className="flex items-center justify-between p-4 rounded-xl border border-base-300 bg-base-100">
							<div>
								<p className="text-sm font-medium">Autosave</p>
								<p className="text-xs text-base-content/40 mt-0.5">
									Automatically save changes as you edit
								</p>
							</div>
							<input
								type="checkbox"
								className="toggle toggle-primary"
								checked={settings.autosave !== false}
								onChange={(e) =>
									save({
										autosave: e.target.checked,
									})
								}
							/>
						</div>
					</section>

					{/* Workspace */}
					<section>
						<SectionHeading className="mb-4">Workspace</SectionHeading>
						<div className="p-4 rounded-xl border border-base-300 bg-base-100 space-y-3">
							<div>
								<p className="text-sm font-medium mb-1">Client Libraries</p>
								{(settings.client_library_dirs ?? []).length > 0 ? (
									<div className="space-y-1.5 mb-2">
										{settings.client_library_dirs.map((dir) => (
											<div
												key={dir}
												className="flex items-center justify-between gap-2 px-3 py-2 rounded-lg bg-base-200/50 text-xs font-mono"
											>
												<span className="truncate">{dir}</span>
												<button
													type="button"
													className="btn btn-ghost btn-xs opacity-40 hover:opacity-100"
													onClick={() => removeClientLibraryDir(dir)}
													title="Remove"
												>
													<svg
														xmlns="http://www.w3.org/2000/svg"
														viewBox="0 0 20 20"
														fill="currentColor"
														className="size-3.5"
													>
														<title>Remove</title>
														<path d="M6.28 5.22a.75.75 0 0 0-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 1 0 1.06 1.06L10 11.06l3.72 3.72a.75.75 0 1 0 1.06-1.06L11.06 10l3.72-3.72a.75.75 0 0 0-1.06-1.06L10 8.94 6.28 5.22Z" />
													</svg>
												</button>
											</div>
										))}
									</div>
								) : (
									<p className="text-xs text-base-content/30 mb-2">
										No client libraries configured
									</p>
								)}
								<button
									type="button"
									className="btn btn-outline btn-sm"
									onClick={addClientLibraryDir}
								>
									Add Client Library Path
								</button>
							</div>
							<div className="border-t border-base-300 my-3" />
							<div>
								<p className="text-sm font-medium mb-1">Template Libraries</p>
								<p className="text-xs text-base-content/40 font-mono break-all">
									{settings.templates_dir ?? "Not configured"}
								</p>
							</div>
							<button
								type="button"
								className="btn btn-outline btn-sm"
								onClick={pickTemplatesDir}
							>
								Add Template Library Path
							</button>
							<div className="border-t border-base-300 my-3" />
							<div>
								<p className="text-sm font-medium mb-1">
									Questionnaire Libraries
								</p>
								<p className="text-xs text-base-content/40 font-mono break-all">
									{settings.questionnaires_dir ?? "Not configured"}
								</p>
							</div>
							<button
								type="button"
								className="btn btn-outline btn-sm"
								onClick={pickQuestionnairesDir}
							>
								Add Questionnaire Library Path
							</button>
						</div>
					</section>

					{/* Drafting Agent */}
					<section>
						<SectionHeading className="mb-4">Drafting Agent</SectionHeading>
						<div className="p-4 rounded-xl border border-base-300 bg-base-100 space-y-3">
							<div>
								<label className="text-sm font-medium mb-1 block">
									Agent Command
								</label>
								<input
									type="text"
									className="input input-bordered input-sm w-full font-mono"
									defaultValue={settings.agent_command ?? "opencode"}
									onBlur={(event) =>
										void save({ agent_command: event.target.value || null })
									}
								/>
								<p className="text-xs text-base-content/40 mt-1">
									The command that starts the external drafting agent.
								</p>
							</div>
							<div>
								<label className="text-sm font-medium mb-1 block">
									Agent Model
								</label>
								<input
									type="text"
									className="input input-bordered input-sm w-full font-mono"
									defaultValue={settings.agent_model ?? ""}
									placeholder="provider/model#variant"
									onBlur={(event) =>
										void save({ agent_model: event.target.value || null })
									}
								/>
								<p className="text-xs text-base-content/40 mt-1">
									Optional model override passed to the agent command.
								</p>
							</div>
							<div>
								<label className="text-sm font-medium mb-1 block">
									Lily Repository
								</label>
								<input
									type="text"
									className="input input-bordered input-sm w-full font-mono"
									defaultValue={settings.agent_repo_dir ?? ""}
									placeholder="~/Software/lily"
									onBlur={(event) =>
										void save({ agent_repo_dir: event.target.value || null })
									}
								/>
								<p className="text-xs text-base-content/40 mt-1">
									The directory where the agent finds the Lily CLI and plan
									tools.
								</p>
							</div>
						</div>
					</section>

					{/* About */}
					<section>
						<SectionHeading className="mb-4">About</SectionHeading>
						<div className="p-4 rounded-xl border border-base-300 bg-base-100">
							<div className="flex items-center gap-3">
								<img src={lilyIcon} alt="" className="size-8 opacity-60" />
								<div>
									<p className="text-sm font-semibold">Lily</p>
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
							</div>
						</div>
					</section>
				</div>
			</div>
		</div>
	);
}
