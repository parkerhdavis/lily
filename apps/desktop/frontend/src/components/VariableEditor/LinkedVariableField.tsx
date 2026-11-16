// SPDX-License-Identifier: AGPL-3.0-or-later
import StatusDot from "@/components/ui/StatusDot";

// ─── Icons (reused from ContactRoleField pattern) ────────────────────────

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

// ─── Component ───────────────────────────────────────────────────────────

export default function LinkedVariableField({
	name,
	value,
	isLinked,
	isSelected,
	isConditional,
	linkedToRole,
	varType = "text",
	schemaEntry,
	isMalformed,
	onToggleLink,
	onChange,
	onSelect,
	scrollToOccurrence,
}: {
	name: string;
	value: string;
	isLinked: boolean;
	isSelected: boolean;
	isConditional: boolean;
	/** If this conditional is derived from a contact-role (e.g., "Has Healthcare POA Agent"), show the role name. */
	linkedToRole?: string;
	varType?: "text" | "date" | "currency";
	schemaEntry?: { required?: boolean; help?: string; var_type?: string };
	isMalformed?: boolean;
	onToggleLink: (linked: boolean) => void;
	onChange: (value: string) => void;
	onSelect: () => void;
	scrollToOccurrence: (name: string, direction: "prev" | "next") => void;
}) {
	const isFilled = isConditional || Boolean(value);
	const linkLabel = linkedToRole ? `Linked to ${linkedToRole}` : "Linked";

	return (
		<div
			data-var-entry={name}
			className={`w-full rounded-lg border bg-base-100 shadow-[0_4px_16px_rgba(0,0,0,0.25)] ${
				isSelected
					? "ring-2 ring-warning border-warning"
					: isMalformed
						? "border-warning/50"
						: "border-base-300"
			}`}
		>
			{/* Name header */}
			<div className="flex items-center justify-between px-3 py-2 bg-base-200/60 border-b border-base-300 rounded-t-lg">
				<div className="text-sm font-bold flex items-center gap-1.5">
					{!isConditional && <StatusDot filled={isFilled} />}
					{name}
					{isMalformed && (
						<span
							className="badge badge-warning badge-xs ml-1"
							title="This variable contains ?? or :: but wasn't parsed as a conditional. Check the template syntax."
						>
							!
						</span>
					)}
				</div>
				<div className="join">
					<button
						type="button"
						className="join-item btn btn-ghost btn-xs px-1"
						onClick={() => scrollToOccurrence(name, "prev")}
						title="Previous occurrence"
					>
						&lsaquo;
					</button>
					<button
						type="button"
						className="join-item btn btn-ghost btn-xs px-1"
						onClick={() => scrollToOccurrence(name, "next")}
						title="Next occurrence"
					>
						&rsaquo;
					</button>
				</div>
			</div>

			{/* Link/unlink bar */}
			<div className="border-b border-base-300 flex">
				{isLinked ? (
					<button
						type="button"
						className="flex-1 btn btn-ghost btn-sm rounded-none border-0 gap-1.5 text-base-content/50"
						onClick={() => onToggleLink(false)}
						title="Unlink to override for this document only"
					>
						<LinkIcon className="size-3" />
						{linkLabel}
					</button>
				) : (
					<button
						type="button"
						className="flex-1 btn btn-sm rounded-none border-0 gap-1.5 btn-warning btn-outline"
						onClick={() => onToggleLink(true)}
						title="Re-link to the client value"
					>
						<LinkSlashIcon className="size-3" />
						Unlinked
					</button>
				)}
			</div>

			{/* Value area */}
			<div className="p-3">
				{isConditional ? (
					<ConditionalToggle
						value={value}
						disabled={isLinked}
						onChange={onChange}
						onSelect={onSelect}
					/>
				) : (
					<VariableInput
						name={name}
						value={value}
						disabled={isLinked}
						varType={varType}
						schemaEntry={schemaEntry}
						onChange={onChange}
						onSelect={onSelect}
					/>
				)}
			</div>
		</div>
	);
}

// ─── Sub-components ──────────────────────────────────────────────────────

function ConditionalToggle({
	value,
	disabled,
	onChange,
	onSelect,
}: {
	value: string;
	disabled: boolean;
	onChange: (value: string) => void;
	onSelect: () => void;
}) {
	const isTrue = value === "true";
	const isFalse = value === "false";

	return (
		<div
			className={`flex rounded-lg overflow-hidden border border-base-300 ${disabled ? "opacity-50 pointer-events-none" : ""}`}
		>
			<button
				type="button"
				className={`flex-1 text-xs font-semibold py-1.5 transition-colors ${
					isTrue
						? "bg-success text-success-content"
						: "bg-base-200 text-base-content/40 hover:bg-base-300"
				}`}
				onClick={() => {
					onSelect();
					onChange("true");
				}}
				onFocus={onSelect}
				disabled={disabled}
			>
				True
			</button>
			<button
				type="button"
				className={`flex-1 text-xs font-semibold py-1.5 transition-colors ${
					isFalse
						? "bg-error text-error-content"
						: "bg-base-200 text-base-content/40 hover:bg-base-300"
				}`}
				onClick={() => {
					onSelect();
					onChange("false");
				}}
				onFocus={onSelect}
				disabled={disabled}
			>
				False
			</button>
		</div>
	);
}

function VariableInput({
	name,
	value,
	disabled,
	varType,
	schemaEntry,
	onChange,
	onSelect,
}: {
	name: string;
	value: string;
	disabled: boolean;
	varType: string;
	schemaEntry?: { required?: boolean; help?: string };
	onChange: (value: string) => void;
	onSelect: () => void;
}) {
	if (varType === "date") {
		return (
			<div>
				<div className="flex gap-2">
					<input
						type="date"
						className={`input input-bordered input-sm flex-1 ${disabled ? "opacity-50 pointer-events-none" : ""}`}
						value={value}
						onChange={(e) => onChange(e.target.value)}
						onFocus={onSelect}
						disabled={disabled}
					/>
					{schemaEntry?.required && !value && (
						<span className="badge badge-error badge-sm self-center">
							required
						</span>
					)}
				</div>
				{schemaEntry?.help && (
					<p className="text-xs text-base-content/40 mt-1">
						{schemaEntry.help}
					</p>
				)}
			</div>
		);
	}

	if (varType === "currency") {
		return (
			<div>
				<div className="flex gap-2">
					<span className="flex items-center text-base-content/50 text-sm pl-1">
						$
					</span>
					<input
						type="text"
						inputMode="decimal"
						className={`input input-bordered input-sm flex-1 ${disabled ? "opacity-50 pointer-events-none" : ""}`}
						placeholder="0.00"
						value={value}
						onChange={(e) => {
							const v = e.target.value.replace(/[^0-9.,]/g, "");
							onChange(v);
						}}
						onFocus={onSelect}
						disabled={disabled}
					/>
					{schemaEntry?.required && !value && (
						<span className="badge badge-error badge-sm self-center">
							required
						</span>
					)}
				</div>
				{schemaEntry?.help && (
					<p className="text-xs text-base-content/40 mt-1">
						{schemaEntry.help}
					</p>
				)}
			</div>
		);
	}

	return (
		<div>
			<div className="flex gap-2">
				<input
					type="text"
					className={`input input-bordered input-sm flex-1 ${disabled ? "opacity-50 pointer-events-none" : ""}`}
					placeholder={`Enter ${name}`}
					value={value}
					onChange={(e) => onChange(e.target.value)}
					onFocus={onSelect}
					disabled={disabled}
				/>
				{schemaEntry?.required && !value && (
					<span className="badge badge-error badge-sm self-center">
						required
					</span>
				)}
			</div>
			{schemaEntry?.help && (
				<p className="text-xs text-base-content/40 mt-1">{schemaEntry.help}</p>
			)}
		</div>
	);
}
