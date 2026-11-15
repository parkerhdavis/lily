// SPDX-License-Identifier: AGPL-3.0-or-later
import { useCallback, useState } from "react";
import { useWorkflowStore } from "@/stores/workflowStore";
import type { Contact, ContactBinding, RoleAssignment } from "@/types";
import type { QuestionDef } from "@/types/questionnaire";

type ContactRoleQuestion = Extract<QuestionDef, { kind: "contact-role" }>;

/** Read the ordered assignments from a binding, falling back to the legacy
 *  single-contact and contact-list shapes. */
function bindingAssignments(binding?: ContactBinding): RoleAssignment[] {
	if (binding?.assignments?.length) return binding.assignments;
	if (binding?.contact_ids?.length) {
		return [{ contact_ids: binding.contact_ids }];
	}
	if (binding?.contact_id && binding.contact_id !== "__none__") {
		return [{ contact_ids: [binding.contact_id] }];
	}
	return [];
}

function contactLabel(contact: Contact): string {
	const relationship =
		contact.relationship === "Other" && contact.other_relationship
			? contact.other_relationship
			: contact.relationship;
	return relationship
		? `${contact.full_name} (${relationship})`
		: contact.full_name;
}

/** Ordered, shared-or-successor assignment editor for one role. */
export default function AssignmentPicker({
	question,
}: {
	question: ContactRoleQuestion;
}) {
	const { lilyFile, setContactBinding, clearContactBinding } =
		useWorkflowStore();

	const contacts = lilyFile?.contacts ?? [];
	const bindings = lilyFile?.contact_bindings ?? {};
	const [draft, setDraft] = useState<RoleAssignment[] | null>(null);
	const assignments = draft ?? bindingAssignments(bindings[question.role]);

	const save = useCallback(
		async (next: RoleAssignment[]) => {
			const cleaned = next
				.map((assignment) => ({
					...assignment,
					contact_ids: assignment.contact_ids.filter(Boolean),
				}))
				.filter(
					(assignment) =>
						assignment.contact_ids.length > 0 ||
						Object.values(assignment.values ?? {}).some(Boolean),
				);
			setDraft(cleaned);
			if (cleaned.length === 0) {
				await clearContactBinding(question.role);
				return;
			}
			await setContactBinding(question.role, {
				contact_id: null,
				variable_mappings: question.variableMappings,
				assignments: cleaned,
			});
		},
		[
			question.role,
			question.variableMappings,
			setContactBinding,
			clearContactBinding,
		],
	);

	const updateAssignment = useCallback(
		(
			index: number,
			updater: (assignment: RoleAssignment) => RoleAssignment,
		) => {
			const next = assignments.map((assignment, current) =>
				current === index ? updater(assignment) : assignment,
			);
			void save(next);
		},
		[assignments, save],
	);

	return (
		<div className="form-control w-full">
			<label className="label pb-1">
				<span className="label-text text-sm font-medium">{question.label}</span>
			</label>

			{assignments.length === 0 && (
				<div className="rounded-lg border border-base-300 bg-base-200/40 px-3 py-2 text-sm text-base-content/50 italic">
					No one assigned to this role
				</div>
			)}

			<div className="space-y-2">
				{assignments.map((assignment, index) => (
					<div
						key={index}
						className="rounded-lg border border-base-300 bg-base-100 px-3 py-2"
					>
						<div className="flex items-center justify-between gap-2">
							<span className="text-xs font-semibold text-base-content/60">
								{index === 0 ? "First assignment" : `Successor ${index}`}
								{assignment.contact_ids.length > 1 ? " — serve together" : ""}
							</span>
							<button
								type="button"
								className="btn btn-ghost btn-xs text-error"
								onClick={() =>
									void save(
										assignments.filter((_, current) => current !== index),
									)
								}
							>
								Remove
							</button>
						</div>

						<div className="mt-1 space-y-1">
							{assignment.contact_ids.map((id, personIndex) => {
								const contact = contacts.find(
									(candidate) => candidate.id === id,
								);
								return (
									<div
										key={`${id}-${personIndex}`}
										className="flex items-center justify-between gap-2 rounded bg-base-200/60 px-2 py-1 text-sm"
									>
										<span>
											{contact ? contactLabel(contact) : "Unknown contact"}
										</span>
										<button
											type="button"
											className="btn btn-ghost btn-xs text-error"
											onClick={() =>
												void updateAssignment(index, (current) => ({
													...current,
													contact_ids: current.contact_ids.filter(
														(_, currentPerson) => currentPerson !== personIndex,
													),
												}))
											}
										>
											&times;
										</button>
									</div>
								);
							})}
						</div>

						<select
							className="select select-bordered select-sm mt-2 w-full"
							value=""
							onChange={(event) => {
								const id = event.target.value;
								if (!id) return;
								void updateAssignment(index, (current) => ({
									...current,
									contact_ids: [...current.contact_ids, id],
								}));
							}}
						>
							<option value="">Add person to this assignment...</option>
							{contacts
								.filter(
									(contact) => !assignment.contact_ids.includes(contact.id),
								)
								.map((contact) => (
									<option key={contact.id} value={contact.id}>
										{contactLabel(contact)}
									</option>
								))}
						</select>
					</div>
				))}
			</div>

			{!question.sharedOnly && (
				<button
					type="button"
					className="btn btn-ghost btn-xs mt-2 self-start"
					onClick={() => setDraft([...assignments, { contact_ids: [] }])}
				>
					+ Add successor
				</button>
			)}
		</div>
	);
}
