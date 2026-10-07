//! MieoSQL desktop application (Tauri 2).
//!
//! The Rust side of the shell: opens the engine, exposes the API as Tauri
//! commands and streams query events to the webview. The TypeScript frontend
//! lives in `frontend/` and is wired in by the design phase.

// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use mieosql_api::Api;
use mieosql_engine::{Engine, EngineConfig};

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
        ])
        .run(tauri::generate_context!())
        .expect("error while running MieoSQL");
}
