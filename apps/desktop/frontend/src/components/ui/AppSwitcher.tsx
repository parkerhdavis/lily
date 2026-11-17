// SPDX-License-Identifier: AGPL-3.0-or-later
import { useEffect, useRef, useState } from "react";
import { useLilyIcon } from "@/hooks/useLilyIcon";
import { useWorkflowStore } from "@/stores/workflowStore";
import type { WorkflowStep } from "@/types";
import { extractFolderName } from "@/utils/path";

/** Steps that belong to the client management branch. */
const CLIENT_STEPS = new Set<WorkflowStep>(["clients", "select-template"]);

/**
 * App switcher dropdown (like Google's grid icon).
 * Provides navigation between the three top-level branches:
 * Lily Hub, Pipeline Management, and App Settings.
 */
export default function AppSwitcher({ className }: { className?: string }) {
	const lilyIcon = useLilyIcon();
	const step = useWorkflowStore((s) => s.step);
	const workingDir = useWorkflowStore((s) => s.workingDir);
	const goToHub = useWorkflowStore((s) => s.goToHub);
	const goToClients = useWorkflowStore((s) => s.goToClients);
	const goToPipeline = useWorkflowStore((s) => s.goToPipeline);
	const goToSettings = useWorkflowStore((s) => s.goToSettings);
	const setWorkingDir = useWorkflowStore((s) => s.setWorkingDir);
	const [open, setOpen] = useState(false);
	const containerRef = useRef<HTMLDivElement>(null);

	// Close on click outside
	useEffect(() => {
		if (!open) return;
		const handleClick = (e: MouseEvent) => {
			if (
				containerRef.current &&
				!containerRef.current.contains(e.target as Node)
			) {
				setOpen(false);
			}
		};
		document.addEventListener("mousedown", handleClick);
		return () => document.removeEventListener("mousedown", handleClick);
	}, [open]);

	const navigate = (action: () => void) => {
		setOpen(false);
		action();
	};

	// Show "Resume client" when on a non-client step with a client loaded
	const showResume = !CLIENT_STEPS.has(step) && step !== "hub" && workingDir;

	return (
		<div ref={containerRef} className={`relative ${className ?? ""}`}>
			<button
				type="button"
				className="btn btn-ghost btn-sm btn-square"
				onClick={() => setOpen((v) => !v)}
			>
				<svg
					xmlns="http://www.w3.org/2000/svg"
					viewBox="0 0 20 20"
					fill="currentColor"
					className="size-4 opacity-60"
				>
					<title>App switcher</title>
					<path
						fillRule="evenodd"
						d="M2 3.5A1.5 1.5 0 0 1 3.5 2h2A1.5 1.5 0 0 1 7 3.5v2A1.5 1.5 0 0 1 5.5 7h-2A1.5 1.5 0 0 1 2 5.5v-2Zm6 0A1.5 1.5 0 0 1 9.5 2h2a1.5 1.5 0 0 1 1.5 1.5v2A1.5 1.5 0 0 1 11.5 7h-2A1.5 1.5 0 0 1 8 5.5v-2Zm6 0A1.5 1.5 0 0 1 15.5 2H17a1.5 1.5 0 0 1 1.5 1.5v2A1.5 1.5 0 0 1 17 7h-1.5A1.5 1.5 0 0 1 14 5.5v-2ZM2 9.5A1.5 1.5 0 0 1 3.5 8h2A1.5 1.5 0 0 1 7 9.5v2A1.5 1.5 0 0 1 5.5 13h-2A1.5 1.5 0 0 1 2 11.5v-2Zm6 0A1.5 1.5 0 0 1 9.5 8h2a1.5 1.5 0 0 1 1.5 1.5v2a1.5 1.5 0 0 1-1.5 1.5h-2A1.5 1.5 0 0 1 8 11.5v-2Zm6 0A1.5 1.5 0 0 1 15.5 8H17a1.5 1.5 0 0 1 1.5 1.5v2A1.5 1.5 0 0 1 17 13h-1.5A1.5 1.5 0 0 1 14 11.5v-2ZM2 15.5A1.5 1.5 0 0 1 3.5 14h2A1.5 1.5 0 0 1 7 15.5v2A1.5 1.5 0 0 1 5.5 19h-2A1.5 1.5 0 0 1 2 17.5v-2Zm6 0A1.5 1.5 0 0 1 9.5 14h2a1.5 1.5 0 0 1 1.5 1.5v2a1.5 1.5 0 0 1-1.5 1.5h-2A1.5 1.5 0 0 1 8 17.5v-2Zm6 0a1.5 1.5 0 0 1 1.5-1.5H17a1.5 1.5 0 0 1 1.5 1.5v2A1.5 1.5 0 0 1 17 19h-1.5a1.5 1.5 0 0 1-1.5-1.5v-2Z"
						clipRule="evenodd"
					/>
				</svg>
			</button>

			{open && (
				<ul className="absolute right-0 top-full mt-1 menu bg-base-100 rounded-box shadow-lg border border-base-300 z-[60] w-56 p-2">
					<li>
						<button
							type="button"
							className={step === "hub" ? "active" : ""}
							onClick={() => navigate(goToHub)}
						>
							<img src={lilyIcon} alt="" className="size-4" />
							Lily Hub
						</button>
					</li>
					<li>
						<button
							type="button"
							className={step === "clients" ? "active" : ""}
							onClick={() => navigate(goToClients)}
						>
							<svg
								xmlns="http://www.w3.org/2000/svg"
								viewBox="0 0 20 20"
								fill="currentColor"
								className="size-4"
							>
								<title>Clients</title>
								<path d="M7 8a3 3 0 1 0 0-6 3 3 0 0 0 0 6ZM14.5 9a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5ZM1.615 16.428a1.224 1.224 0 0 1-.569-1.175 6.002 6.002 0 0 1 11.908 0c.058.467-.172.92-.57 1.174A9.953 9.953 0 0 1 7 18a9.953 9.953 0 0 1-5.385-1.572ZM14.5 16h-.106c.07-.297.088-.611.048-.933a7.47 7.47 0 0 0-1.588-3.755 4.502 4.502 0 0 1 5.874 2.636.818.818 0 0 1-.36.98A7.465 7.465 0 0 1 14.5 16Z" />
							</svg>
							Clients
						</button>
					</li>
					<li>
						<button
							type="button"
							className={step === "pipeline" ? "active" : ""}
							onClick={() => navigate(goToPipeline)}
						>
							<svg
								xmlns="http://www.w3.org/2000/svg"
								viewBox="0 0 20 20"
								fill="currentColor"
								className="size-4"
							>
								<title>Pipeline</title>
								<path d="M3.75 3A1.75 1.75 0 0 0 2 4.75v3.26a3.235 3.235 0 0 1 1.75-.51h12.5c.644 0 1.245.188 1.75.51V6.75A1.75 1.75 0 0 0 16.25 5h-4.836a.25.25 0 0 1-.177-.073L9.823 3.513A1.75 1.75 0 0 0 8.586 3H3.75ZM3.75 9A1.75 1.75 0 0 0 2 10.75v4.5c0 .966.784 1.75 1.75 1.75h12.5A1.75 1.75 0 0 0 18 15.25v-4.5A1.75 1.75 0 0 0 16.25 9H3.75Z" />
							</svg>
							Pipeline
						</button>
					</li>
					<li>
						<button
							type="button"
							className={step === "app-settings" ? "active" : ""}
							onClick={() => navigate(goToSettings)}
						>
							<svg
								xmlns="http://www.w3.org/2000/svg"
								viewBox="0 0 20 20"
								fill="currentColor"
								className="size-4"
							>
								<title>Settings</title>
								<path
									fillRule="evenodd"
									d="M7.84 1.804A1 1 0 0 1 8.82 1h2.36a1 1 0 0 1 .98.804l.331 1.652a6.993 6.993 0 0 1 1.929 1.115l1.598-.54a1 1 0 0 1 1.186.447l1.18 2.044a1 1 0 0 1-.205 1.251l-1.267 1.113a7.047 7.047 0 0 1 0 2.228l1.267 1.113a1 1 0 0 1 .206 1.25l-1.18 2.045a1 1 0 0 1-1.187.447l-1.598-.54a6.993 6.993 0 0 1-1.929 1.115l-.33 1.652a1 1 0 0 1-.98.804H8.82a1 1 0 0 1-.98-.804l-.331-1.652a6.993 6.993 0 0 1-1.929-1.115l-1.598.54a1 1 0 0 1-1.186-.447l-1.18-2.044a1 1 0 0 1 .205-1.251l1.267-1.114a7.05 7.05 0 0 1 0-2.227L1.821 7.773a1 1 0 0 1-.206-1.25l1.18-2.045a1 1 0 0 1 1.187-.447l1.598.54A6.992 6.992 0 0 1 7.51 3.456l.33-1.652ZM10 13a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z"
									clipRule="evenodd"
								/>
							</svg>
							Settings
						</button>
					</li>
					{showResume && (
						<>
							<div className="divider my-0.5" />
							<li>
								<button
									type="button"
									className="text-primary"
									onClick={() => navigate(() => setWorkingDir(workingDir!))}
								>
									<svg
										xmlns="http://www.w3.org/2000/svg"
										viewBox="0 0 20 20"
										fill="currentColor"
										className="size-4"
									>
										<title>Resume</title>
										<path
											fillRule="evenodd"
											d="M15.312 11.424a5.5 5.5 0 0 1-9.201 2.466l-.312-.311h2.433a.75.75 0 0 0 0-1.5H4.598a.75.75 0 0 0-.75.75v3.634a.75.75 0 0 0 1.5 0v-2.033l.312.311a7 7 0 0 0 11.712-3.138.75.75 0 0 0-1.449-.39Zm1.06-7.358a.75.75 0 0 0-1.5 0v2.033l-.312-.31a7 7 0 0 0-11.712 3.137.75.75 0 0 0 1.449.39 5.5 5.5 0 0 1 9.201-2.466l.312.311H11.42a.75.75 0 1 0 0 1.5h3.634a.75.75 0 0 0 .75-.75V4.066Z"
											clipRule="evenodd"
										/>
									</svg>
									Resume: {extractFolderName(workingDir)}
								</button>
							</li>
						</>
					)}
				</ul>
			)}
		</div>
	);
}
