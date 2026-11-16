// SPDX-License-Identifier: AGPL-3.0-or-later
import StatusDot from "@/components/ui/StatusDot";
import type { Contact, ContactBinding } from "@/types";
import {
	type ContactRoleGroup,
	getContactProperty,
	PROPERTY_LABELS,
} from "./variableHelpers";

function LinkIcon({ className }: { className?: string }) {
	return (
		<svg
			xmlns="http://www.w3.org/2000/svg"
			viewBox="0 0 20 20"
			fill="currentColor"
			className={className ?? "size-4"}
		>
			<title>Linked</title>
			<path d="M12.232 4.232a2.5 2.5 0 0 1 3.536 3.536l-1.225 1.224a.75.75 0 0 0 1.061 1.06l1.224-1.224a4 4 0 0 0-5.656-5.656l-3 3a4 4 0 0 0 .225 5.865.75.75 0 0 0 .977-1.138 2.5 2.5 0 0 1-.142-3.667l3-3Z" />
			<path d="M11.603 7.963a.75.75 0 0 0-.977 1.138 2.5 2.5 0 0 1 .142 3.667l-3 3a2.5 2.5 0 0 1-3.536-3.536l1.225-1.224a.75.75 0 0 0-1.061-1.06l-1.224 1.224a4 4 0 1 0 5.656 5.656l3-3a4 4 0 0 0-.225-5.865Z" />
		</svg>
	);
}

function LinkSlashIcon({ className }: { className?: string }) {
	return (
		<svg
			xmlns="http://www.w3.org/2000/svg"
			viewBox="0 0 20 20"
			fill="currentColor"
			className={className ?? "size-4"}
		>
			<title>Unlinked</title>
			<path d="M.172 2.172a.586.586 0 0 1 .828 0l16.828 16.828a.586.586 0 0 1-.828.828L.172 3a.586.586 0 0 1 0-.828Z" />
			<path d="M12.232 4.232a2.5 2.5 0 0 1 3.536 3.536l-1.225 1.224a.75.75 0 0 0 1.061 1.06l1.224-1.224a4 4 0 0 0-5.656-5.656l-3 3a4 4 0 0 0-.036 5.612.75.75 0 1 0 1.06-1.06 2.5 2.5 0 0 1 .023-3.51l3.013-2.982Z" />
			<path d="M7.768 15.768a2.5 2.5 0 0 1-3.536-3.536l1.225-1.224a.75.75 0 0 0-1.061-1.06l-1.224 1.224a4 4 0 0 0 5.656 5.656l3-3a4 4 0 0 0 .036-5.612.75.75 0 0 0-1.06 1.06 2.5 2.5 0 0 1-.023 3.51l-3.013 2.982Z" />
		</svg>
	);
}

// ─── Contact-role field ─────────────────────────────────────────────────────

export default function ContactRoleField({
	group,
	contacts,
	bindings,
	variableValues,
	isOverridden,
	isSelected,
	onToggleOverride,
	onSelectContact,
	onManualChange,
	onSelect,
	scrollToOccurrence,
}: {
	group: ContactRoleGroup;
	contacts: Contact[];
	bindings: Record<string, ContactBinding>;
	variableValues: Record<string, string>;
	isOverridden: boolean;
	isSelected: boolean;
	onToggleOverride: (overriding: boolean) => Promise<void>;
	onSelectContact: (contactId: string | null) => Promise<void>;
	onManualChange: (varName: string, value: string) => void;
	onSelect: (varName: string) => void;
	scrollToOccurrence: (varName: string, direction: "prev" | "next") => void;
}) {
	// Client-level binding (source of truth when linked)
	const qBinding = bindings[group.role];
	const qContactId = qBinding?.contact_id ?? null;
	const qContact = contacts.find((c) => c.id === qContactId) ?? null;

	// Determine the effective display state
	const allFilled = group.properties.every((p) =>
		variableValues[p.displayName]?.trim(),
	);

	return (
		<div
			className={`w-full rounded-lg border bg-base-100 shadow-[0_4px_16px_rgba(0,0,0,0.25)] ${isSelected ? "ring-2 ring-warning border-warning" : "border-base-300"}`}
			data-var-entry={group.properties[0]?.displayName}
		>
			{/* Name header */}
			<div className="flex items-center justify-between px-3 py-2 bg-base-200/60 border-b border-base-300 rounded-t-lg">
				<span className="text-sm font-bold flex items-center gap-1.5">
					<StatusDot filled={allFilled} />
					{group.role}
				</span>
				<div className="join">
					<button
						type="button"
						className="join-item btn btn-ghost btn-xs px-1"
						onClick={() =>
							scrollToOccurrence(group.properties[0]?.displayName, "prev")
						}
						title="Previous occurrence"
					>
						&lsaquo;
					</button>
					<button
						type="button"
						className="join-item btn btn-ghost btn-xs px-1"
						onClick={() =>
							scrollToOccurrence(group.properties[0]?.displayName, "next")
						}
						title="Next occurrence"
					>
						&rsaquo;
					</button>
				</div>
			</div>

			{/* Link/override controls */}
			<div className="border-b border-base-300 flex">
				{!isOverridden ? (
					<button
						type="button"
						className="flex-1 btn btn-ghost btn-sm rounded-none border-0 gap-1.5 text-base-content/50 border-r border-base-300"
						onClick={() => onToggleOverride(true)}
						title="Unlink from the client value so you can set a different value for this document only"
					>
						<LinkIcon className="size-3" />
						Linked
					</button>
				) : (
					<button
						type="button"
						className="flex-1 btn btn-sm rounded-none border-0 gap-1.5 btn-warning btn-outline border-r border-base-300"
						onClick={() => onToggleOverride(false)}
						title="Re-link this role to the client value and discard the document-specific override"
					>
						<LinkSlashIcon className="size-3" />
						Unlinked
					</button>
				)}
			</div>

			{/* ── Linked state: greyed-out, shows the client value ── */}
			{!isOverridden && (
				<div className="p-3">
					<select
						className="select select-bordered select-sm w-full opacity-50 pointer-events-none"
						value={
							qContactId === "__none__"
								? "__none__"
								: qContact
									? qContact.id
									: ""
						}
						disabled
						tabIndex={-1}
					>
						<option value="">Not assigned</option>
						<option value="__none__">None</option>
						{contacts.map((c) => (
							<option key={c.id} value={c.id}>
								{c.full_name}
								{c.relationship
									? ` (${c.relationship === "Other" && c.other_relationship ? c.other_relationship : c.relationship})`
									: ""}
							</option>
						))}
					</select>
					{qContactId === "__none__" && (
						<div className="mt-2 text-xs text-base-content/50 italic pl-3">
							No one assigned to this role
						</div>
					)}
					{qContact && (
						<div className="mt-2 pl-3 border-l-2 border-primary/30 opacity-75">
							<div className="space-y-1">
								{group.properties.map(({ displayName, property }) => {
									const value = getContactProperty(qContact, property);
									return (
										<div
											key={displayName}
											className="flex items-center gap-2 text-xs"
										>
											<span className="text-base-content/50 min-w-20">
												{PROPERTY_LABELS[property] ?? property}:
											</span>
											<span
												className={
													value
														? "text-base-content"
														: "text-base-content/30 italic"
												}
											>
												{value || "empty"}
											</span>
										</div>
									);
								})}
							</div>
						</div>
					)}
				</div>
			)}

			{/* ── Overridden state: editable dropdown + manual fallback ── */}
			{isOverridden && (
				<div className="p-3">
					<select
						className="select select-bordered select-sm w-full select-warning"
						value={
							// Find if current values match any contact
							contacts.find((c) =>
								group.properties.every(
									(p) =>
										getContactProperty(c, p.property) ===
										(variableValues[p.displayName] ?? ""),
								),
							)?.id ??
							// If any values are filled, show custom; otherwise "not assigned"
							(group.properties.some((p) =>
								(variableValues[p.displayName] ?? "").trim(),
							)
								? "__manual__"
								: "")
						}
						onChange={(e) => {
							const val = e.target.value;
							if (val === "" || val === "__manual__") {
								onSelectContact(null);
							} else {
								onSelectContact(val);
							}
						}}
						onFocus={() => onSelect(group.properties[0]?.displayName)}
					>
						<option value="">Not assigned</option>
						{contacts.map((c) => (
							<option key={c.id} value={c.id}>
								{c.full_name}
								{c.relationship
									? ` (${c.relationship === "Other" && c.other_relationship ? c.other_relationship : c.relationship})`
									: ""}
							</option>
						))}
						<option value="__manual__">Custom values...</option>
					</select>

					{/* Editable fields for the override */}
					<div className="mt-2 pl-3 border-l-2 border-warning/30 space-y-2">
						{group.properties.map(({ displayName, property }) => (
							<div key={displayName}>
								<label className="label pb-0.5">
									<span className="label-text text-xs text-base-content/60">
										{PROPERTY_LABELS[property] ?? property}
									</span>
								</label>
								<input
									type="text"
									className="input input-bordered input-xs w-full"
									placeholder={`Enter ${PROPERTY_LABELS[property] ?? property}`}
									value={variableValues[displayName] ?? ""}
									onChange={(e) => onManualChange(displayName, e.target.value)}
									onFocus={() => onSelect(displayName)}
								/>
							</div>
						))}
					</div>
				</div>
			)}
		</div>
	);
}
