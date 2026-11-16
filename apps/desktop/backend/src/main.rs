// SPDX-License-Identifier: AGPL-3.0-or-later
// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod docx_ops;
mod history;
mod lily_file;
mod plan;
mod provenance;
mod questionnaire;
mod settings;

use docx_ops::{
    copy_template, get_document_html, load_template_schema, load_template_variables,
    rename_document,
};
use lily_file::{
    add_client_variable, add_contact, add_required_document, backfill_required_documents,
    copy_from_spouse_lily,
    delete_contact, delete_document, check_document_consistency, detect_document_statuses,
    ensure_required_document, export_client_data, export_client_archive, import_client_data,
    list_clients_in_library,
    apply_variable_migration, create_lily_file, has_lily_file,
    list_library_tree, load_client_summaries, load_lily_file_cmd, new_version_document,
    open_file_in_os, remove_client_variable, remove_required_document, resolve_contact_variables,
    save_client_variables, save_contact_bindings, save_questionnaire_note, save_variables,
    set_client_questionnaire, set_document_variables, set_role_override, set_variable_overrides,
    set_workflow_stage, set_workflow_waiting_on, update_contact, update_required_document_notes,
    update_required_document_status,
};
use questionnaire::{
    create_questionnaire, delete_questionnaire, duplicate_questionnaire, load_questionnaire,
    load_questionnaire_index, migrate_questionnaires, save_questionnaire,
};
use plan::{
    approve_plan_candidate, audit_plan_document, discard_plan_candidate, render_plan_candidate,
    render_plan_document,
};
use agent::{approve_agent_candidate, run_draft_agent};
use history::{
    diff_document_versions, list_document_versions, restore_document_version, snapshot_document,
};
use settings::{load_settings, save_settings};
use tauri::Manager;
use tracing_subscriber::EnvFilter;

fn init_logging() {
    let log_dir = dirs::config_dir()
        .map(|d| d.join("lily").join("logs"))
        .expect("Could not determine config directory");
    std::fs::create_dir_all(&log_dir).ok();

    let file_appender = tracing_appender::rolling::daily(&log_dir, "lily.log");

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("lily=info")),
        )
        .with_writer(file_appender)
        .with_ansi(false)
        .init();

    tracing::info!("Lily starting up");
}

fn main() {
    init_logging();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            // Restore saved window size from settings, if available. The window
            // is created hidden (`visible: false`), so this resize happens
            // off-screen and is never seen as an intermediate frame.
            if let Ok(settings) = settings::load_settings() {
                if let (Some(w), Some(h)) = (settings.window_width, settings.window_height) {
                    if let Some(window) = app.get_webview_window("main") {
                        let size = tauri::LogicalSize::new(w as f64, h as f64);
                        let _ = window.set_size(size);
                    }
                }
            }

            // Safety net for the hidden-until-painted window. The frontend calls
            // `show_main_window` once the splash has painted; if it never does
            // (bundle failed to load/parse, render threw before mount, etc.) we'd
            // otherwise strand an invisible-but-running window. Reveal it
            // unconditionally after a grace period. `show()` is idempotent, so
            // racing the frontend's call is harmless.
            let safety_handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                if let Some(window) = safety_handle.get_webview_window("main") {
                    if !window.is_visible().unwrap_or(true) {
                        tracing::warn!(
                            "frontend never called show_main_window; revealing via safety net"
                        );
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                // Save current window size to settings before closing.
                if let Ok(size) = window.inner_size() {
                    if let Ok(scale) = window.scale_factor() {
                        let logical_w = (size.width as f64 / scale).round() as u32;
                        let logical_h = (size.height as f64 / scale).round() as u32;
                        if let Ok(mut current) = settings::load_settings() {
                            current.window_width = Some(logical_w);
                            current.window_height = Some(logical_h);
                            let _ = settings::save_settings(current);
                        }
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            show_main_window,
            copy_template,

            rename_document,
            get_document_html,
            load_template_schema,
            load_template_variables,
            load_settings,
            save_settings,
            list_templates,
            render_plan_document,
            render_plan_candidate,
            approve_plan_candidate,
            run_draft_agent,
            approve_agent_candidate,
            discard_plan_candidate,
            audit_plan_document,
            snapshot_document,
            list_document_versions,
            restore_document_version,
            diff_document_versions,
            load_lily_file_cmd,
            save_variables,
            save_client_variables,
            add_client_variable,
            remove_client_variable,
            set_document_variables,
            delete_document,
            new_version_document,
            open_file_in_os,
            add_contact,
            update_contact,
            delete_contact,
            save_contact_bindings,
            resolve_contact_variables,
            save_questionnaire_note,
            set_role_override,
            set_client_questionnaire,
            export_client_data,
            export_client_archive,
            import_client_data,
            load_questionnaire_index,
            load_questionnaire,
            save_questionnaire,
            create_questionnaire,
            duplicate_questionnaire,
            delete_questionnaire,
            migrate_questionnaires,
            add_required_document,
            backfill_required_documents,
            ensure_required_document,
            set_workflow_stage,
            set_workflow_waiting_on,
            update_required_document_status,
            update_required_document_notes,
            remove_required_document,
            detect_document_statuses,
            load_client_summaries,
            list_clients_in_library,
            list_library_tree,
            set_variable_overrides,
            check_document_consistency,
            has_lily_file,
            create_lily_file,
            apply_variable_migration,
            copy_from_spouse_lily,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Reveal the main window. It is created hidden (`visible: false` in
/// tauri.conf.json) so the OS never shows the unpainted webview, the static
/// `index.html` fallback, or the live `set_size` resize in `.setup()`. The
/// frontend calls this once the splash has actually painted, so the window's
/// first on-screen frame is the splash itself. A setup-side safety timer
/// (see `main`) reveals the window anyway if the frontend never calls.
#[tauri::command]
fn show_main_window(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// List all .docx and .dotx template files in a given directory, recursively.
#[tauri::command]
fn list_templates(templates_dir: String) -> Result<Vec<String>, String> {
    let path = std::path::Path::new(&templates_dir);
    if !path.exists() {
        return Err(format!(
            "Templates directory does not exist: {}",
            templates_dir
        ));
    }
    let mut results = Vec::new();
    collect_template_files(path, path, &mut results)?;
    results.sort();
    Ok(results)
}

fn collect_template_files(
    base: &std::path::Path,
    dir: &std::path::Path,
    results: &mut Vec<String>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {}", e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();
        if path.is_dir() {
            if dir == base && entry.file_name() == "00 - Archive" {
                continue;
            }
            collect_template_files(base, &path, results)?;
        } else if let Some(ext) = path.extension() {
            if ext.eq_ignore_ascii_case("docx") || ext.eq_ignore_ascii_case("dotx") {
                // Store the path relative to the templates base directory
                if let Ok(relative) = path.strip_prefix(base) {
                    results.push(relative.to_string_lossy().to_string());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod template_listing_tests {
    use super::collect_template_files;

    #[test]
    fn archive_folder_is_not_listed() {
        let root = std::env::temp_dir().join(format!(
            "lily-template-list-{}",
            std::process::id()
        ));
        let live = root.join("01 - Live");
        let archive = root.join("00 - Archive");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::write(live.join("Live.docx"), b"").unwrap();
        std::fs::write(archive.join("Retired.dotx"), b"").unwrap();

        let mut results = Vec::new();
        collect_template_files(&root, &root, &mut results).unwrap();
        assert_eq!(results, vec!["01 - Live/Live.docx".to_string()]);

        std::fs::remove_dir_all(&root).unwrap();
    }
}
