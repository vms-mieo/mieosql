//! MieoSQL desktop application (Tauri 2).
//!
//! The Rust side of the shell: opens the engine, exposes the API as Tauri
//! commands and streams query events to the webview.
//!
//! ## Frontend loading
//!
//! The frontend bundle (`frontend/dist`) is **always embedded** in the binary
//! (debug and release alike), so the app never depends on a dev server.
//!
//! For frontend work with hot-reload, use the separate dev config:
//!
//! ```text
//! cd apps/mieosql-desktop
//! cargo tauri dev --config tauri.dev.conf.json
//! ```
//!
//! which points the webview at the vite dev server on `http://localhost:5173`.

// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use mieosql_api::Api;
use mieosql_engine::{Engine, EngineConfig};
use tauri::Manager;

/// Shared application state injected into every command.
pub struct AppState {
    pub api: Api,
}

fn main() {
    let engine = match Engine::open(EngineConfig::default()) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("failed to open MieoSQL engine: {err}");
            std::process::exit(1);
        }
    };
    let state = AppState {
        api: Api::new(engine),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::api_version,
            commands::list_profiles,
            commands::get_profile,
            commands::save_profile,
            commands::delete_profile,
            commands::test_profile,
            commands::test_connection,
            commands::open_session,
            commands::close_session,
            commands::list_sessions,
            commands::session_info,
            commands::capabilities,
            commands::server_info,
            commands::list_databases,
            commands::list_schemas,
            commands::list_objects,
            commands::table_columns,
            commands::table_indexes,
            commands::foreign_keys,
            commands::object_ddl,
            commands::use_database,
            commands::begin_transaction,
            commands::commit_transaction,
            commands::rollback_transaction,
            commands::start_query,
            commands::preview_table,
            commands::export_result,
            commands::cancel_query,
            commands::list_history,
            commands::clear_history,
            commands::smoke_report,
            // Feature pack v1: search / explain / docs / datagen / masking /
            // sync / transfer / snapshot / workspace / files
            commands::search_database,
            commands::explain_query,
            commands::export_documentation,
            commands::generate_data,
            commands::export_result_masked,
            commands::structure_sync_plan,
            commands::apply_sync_statements,
            commands::data_sync_plan,
            commands::apply_data_sync,
            commands::transfer_table,
            commands::schema_snapshot,
            commands::export_workspace,
            commands::import_workspace,
            commands::fetch_rows,
            commands::write_text_file,
            commands::write_binary_file,
            // Maintenance pack: batch execute / file import / grid edits /
            // script split / backup
            commands::execute_batch,
            commands::import_file,
            commands::apply_grid_edits,
            commands::script_statements,
            commands::split_sql_text,
            commands::dump_backup,
            commands::list_db_users,
            commands::list_db_roles,
            commands::db_role_members,
            commands::db_user_grants,
            commands::create_db_user,
            commands::drop_db_principal,
            commands::grant,
            commands::revoke,
            commands::set_user_password,
            commands::db_principal_detail,
        ])
        .setup(move |app| {
            // `WebviewUrl::App` resolves against the app URL: the embedded
            // assets normally, or the configured devUrl when running with
            // `cargo tauri dev --config tauri.dev.conf.json`.
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("MieoSQL")
            .inner_size(1360.0, 860.0)
            .min_inner_size(1024.0, 640.0)
            .resizable(true)
            .decorations(false)
            // macOS: transparent window so the frontend shell can draw
            // Apple-style rounded corners (see styles.css, `data-window-shape`).
            // The native window shadow follows the rounded opaque region.
            // No-op on other platforms, which keep square opaque windows.
            .transparent(cfg!(target_os = "macos"))
            .build()?;

            // Optional end-to-end render probe (`MIEOSQL_SMOKE_CHECK=1`):
            // asks the frontend to report what it rendered back over IPC.
            if std::env::var("MIEOSQL_SMOKE_CHECK").is_ok() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    for attempt in 1..=4 {
                        let wait = if attempt == 1 { 2 } else { 3 };
                        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                        match handle.get_webview_window("main") {
                            Some(window) => {
                                let _ = window.eval(SMOKE_SCRIPT);
                            }
                            None => {
                                println!("[smoke] no main window");
                                break;
                            }
                        }
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running MieoSQL");
}

/// JS probe: reports render state back through the `smoke_report` command.
const SMOKE_SCRIPT: &str = r#"(function () {
  function report(info) {
    try {
      window.__TAURI_INTERNALS__.invoke("smoke_report", {
        text: JSON.stringify(info),
      });
    } catch (_) {}
  }
  try {
    var root = document.getElementById("root");
    var header = document.querySelector("header");
    var footer = document.querySelector("footer");
    report({
      href: location.href,
      rootLength: root ? root.innerHTML.length : -1,
      headerText: header ? header.innerText.split("\n").slice(0, 2).join(" / ") : "",
      footerText: footer ? footer.innerText.split("\n").slice(0, 3).join(" / ") : "",
      title: document.title,
    });
  } catch (e) {
    report({ error: String(e) });
  }
})();"#;
