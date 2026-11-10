// Codex Manager - Tauri v2 backend

pub mod commands;
pub mod config;
pub mod models;
pub mod proxy;
pub mod routing;
pub mod storage;
mod tray;
pub mod usage;

use std::collections::HashMap;
use std::sync::Arc;

use tauri::Manager;
use tracing::{info, warn};

use crate::proxy::ProxyServer;
use crate::routing::RoutingEngine;
use crate::storage::EncryptedStore;
use crate::usage::UsagePoller;

/// Application state shared across Tauri commands
pub struct AppState {
    pub store: Arc<EncryptedStore>,
    pub routing_engine: Arc<RoutingEngine>,
    pub usage_poller: Arc<UsagePoller>,
    /// The running proxy server, if any
    pub proxy: tokio::sync::Mutex<Option<ProxyServer>>,
}

/// Initialize and run the Tauri application
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Regular)
                .unwrap_or(());
        }))
        .setup(|app| {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .init();

            let handle = app.handle().clone();

            // Storage: encrypted with the per-install master key
            let db_path = config::get_db_path(&handle).map_err(|e| format!("{e:#}"))?;
            let master_key = config::get_master_key(&handle).map_err(|e| format!("{e:#}"))?;
            let store = EncryptedStore::open(&db_path, &master_key)
                .map_err(|e| format!("Failed to open database: {e:#}"))?;
            let store = Arc::new(store);

            let accounts = store
                .load_accounts()
                .map_err(|e| format!("Failed to load accounts: {e:#}"))?;
            let mut usage_map = HashMap::new();
            for account in &accounts {
                if let Ok(Some(usage)) = store.load_latest_usage(account.id) {
                    usage_map.insert(account.id, usage);
                }
            }

            // Routing engine with the saved strategy
            let app_config = config::load_config(&handle).map_err(|e| format!("{e:#}"))?;
            let routing_engine = Arc::new(RoutingEngine::new(app_config.routing.strategy));
            routing_engine.update_accounts(accounts, usage_map);

            app.manage(AppState {
                store,
                routing_engine,
                usage_poller: Arc::new(UsagePoller::new()),
                proxy: tokio::sync::Mutex::new(None),
            });

            // The window hides on close, so a tray icon is how it comes back.
            if let Err(e) = tray::create_tray(&handle) {
                warn!("System tray is unavailable: {e}");
            }

            // The window is created hidden; show it unless the app was
            // started by the autostart entry (`--minimized`).
            let start_minimized = std::env::args().any(|arg| arg == "--minimized");
            if !start_minimized {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }

            info!("Codex Manager v{} initialized", env!("CARGO_PKG_VERSION"));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                #[cfg(target_os = "macos")]
                window
                    .app_handle()
                    .set_activation_policy(tauri::ActivationPolicy::Accessory)
                    .unwrap_or(());
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            // Account management
            commands::add_account,
            commands::update_account,
            commands::remove_account,
            commands::get_account,
            commands::list_accounts,
            commands::list_account_statuses,
            commands::toggle_account_enabled,
            // Usage data
            commands::get_account_usage,
            commands::refresh_all_usage,
            commands::refresh_account_usage,
            // Routing
            commands::get_routing_stats,
            commands::set_routing_strategy,
            commands::clear_routing_sessions,
            // Proxy server
            commands::start_proxy_server,
            commands::stop_proxy_server,
            commands::get_proxy_status,
            // Import/Export
            commands::export_accounts,
            commands::import_accounts,
            commands::validate_api_key,
            // Configuration
            commands::load_app_config,
            commands::save_app_config,
            commands::update_proxy_configuration,
            commands::update_routing_configuration,
            commands::get_data_directory,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // macOS: clicking the dock icon reopens the window
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                    app_handle
                        .set_activation_policy(tauri::ActivationPolicy::Regular)
                        .unwrap_or(());
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app_handle, event);
        });
}
