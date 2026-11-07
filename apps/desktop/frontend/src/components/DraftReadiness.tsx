// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import type { PlanAudit, PlanAuditSlot } from "@/types";

interface DraftReadinessProps {
	planPath: string | null;
	clientDir: string | null;
}

export default function DraftReadiness({
	planPath,
	clientDir,
}: DraftReadinessProps) {
	const [audit, setAudit] = useState<PlanAudit | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [open, setOpen] = useState(true);
	const [loading, setLoading] = useState(false);

	const load = useCallback(async () => {
		if (!planPath || !clientDir) return;
		setLoading(true);
		try {
			const result = await invoke<PlanAudit>("audit_plan_document", {
				planPath,
				clientDir,
			});
			setAudit(result);
			setError(null);
		} catch (err) {
			setError(String(err));
		} finally {
			setLoading(false);
		}
	}, [planPath, clientDir]);

	useEffect(() => {
		void load();
	}, [load]);

	if (!planPath || !clientDir) return null;

	const required = audit?.missing_slots.filter((slot) => slot.required) ?? [];
	const optional = audit?.missing_slots.filter((slot) => !slot.required) ?? [];
	const unknown =
		audit?.conditions.filter((condition) => condition.result === null) ?? [];
	const missingRoles = audit?.missing_roles ?? [];
	const openCount =
		required.length + optional.length + unknown.length + missingRoles.length;

	return (
		<div className="mb-4 rounded-lg border border-base-300 bg-base-200/40">
			<button
				type="button"
				className="flex w-full items-center justify-between gap-2 px-3 py-2 text-left"
				onClick={() => setOpen((value) => !value)}
			>
				<span className="text-sm font-semibold">Draft readiness</span>
				<span
					className={`badge badge-sm ${
						required.length > 0 || missingRoles.length > 0
							? "badge-error"
							: openCount > 0
								? "badge-warning"
								: "badge-success"
					}`}
				>
					{required.length > 0
						? `${required.length} required`
						: missingRoles.length > 0
							? `${missingRoles.length} unassigned`
							: openCount > 0
								? `${openCount} open`
								: "ready"}
				</span>
			</button>
			{open && (
				<div className="space-y-2 px-3 pb-3 text-xs">
					{loading && <span className="loading loading-spinner loading-xs" />}
					{error && <p className="text-error">{error}</p>}
					{audit && !loading && (
						<>
							<p className="text-base-content/60">
								{audit.plan_id} · {audit.blocks} blocks · {audit.clauses}{" "}
								clauses
							</p>
							<SlotList label="Required" slots={required} />
							<SlotList label="Optional" slots={optional} />
							{missingRoles.length > 0 && (
								<div>
									<p className="font-medium">Unassigned roles</p>
									<ul className="list-disc pl-4">
										{missingRoles.map((role) => (
											<li key={role}>{role}</li>
										))}
									</ul>
								</div>
							)}
							{unknown.length > 0 && (
								<div>
									<p className="font-medium">Unknown conditions</p>
									<ul className="list-disc pl-4">
										{unknown.slice(0, 8).map((condition) => (
											<li
												key={`${condition.where}:${JSON.stringify(condition.condition)}`}
											>
												{condition.where}
											</li>
										))}
									</ul>
								</div>
							)}
							{openCount === 0 && (
								<p className="text-success">All required data is present.</p>
							)}
							<button
								type="button"
								className="btn btn-ghost btn-xs px-1"
								onClick={() => void load()}
							>
								Refresh
							</button>
						</>
					)}
				</div>
			)}
		</div>
	);
}

function SlotList({ label, slots }: { label: string; slots: PlanAuditSlot[] }) {
	if (slots.length === 0) return null;
	return (
		<div>
			<p className="font-medium">{label}</p>
			<ul className="list-disc pl-4">
				{slots.slice(0, 8).map((slot) => (
					<li key={slot.name}>{slot.name}</li>
				))}
			</ul>
			{slots.length > 8 && (
				<p className="text-base-content/50">…and {slots.length - 8} more</p>
			)}
		</div>
	);
}
