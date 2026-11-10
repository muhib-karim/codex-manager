/**
 * Typed contract for the Tauri backend commands (src-tauri/src/commands.rs).
 *
 * The UI does not call these yet; this file exists so that when it does,
 * command names and argument shapes cannot drift from the Rust side.
 * tests/backendContract.test.ts compares BACKEND_COMMANDS with the commands
 * registered in src-tauri/src/lib.rs.
 *
 * Naming rules (Tauri v2): top-level invoke() argument keys are camelCase
 * (`apiKey` maps to the Rust parameter `api_key`), while the fields inside a
 * struct argument or a returned value keep their Rust names (snake_case).
 */
import { invoke } from '@tauri-apps/api/core';

export type BackendRoutingStrategy = 'least_utilized' | 'round_robin' | 'priority' | 'sticky';

/** Account as returned to the UI: `api_key` is masked except in exports. */
export interface BackendAccount {
    id: string;
    label: string;
    api_key: string;
    org_id: string | null;
    model_scope: string[];
    daily_limit: number | null;
    monthly_limit: number | null;
    priority: number;
    enabled: boolean;
    created_at: string;
    updated_at: string;
    last_used: string | null;
}

export interface BackendUsageSnapshot {
    account_id: string;
    tokens_used: number;
    cost_estimate: number;
    hard_limit: number | null;
    soft_limit: number | null;
    remaining_budget: number | null;
    daily_usage: number;
    monthly_usage: number;
    timestamp: string;
}

export interface BackendAccountStatus {
    account: BackendAccount;
    usage: BackendUsageSnapshot;
    is_available: boolean;
    disable_reason: string | null;
}

export interface CreateAccountRequest {
    label: string;
    api_key: string;
    org_id?: string | null;
    model_scope?: string[] | null;
    daily_limit?: number | null;
    monthly_limit?: number | null;
    priority?: number | null;
}

export interface UpdateAccountRequest {
    id: string;
    label?: string | null;
    org_id?: string | null;
    model_scope?: string[] | null;
    daily_limit?: number | null;
    monthly_limit?: number | null;
    priority?: number | null;
    enabled?: boolean | null;
}

export interface BackendProxyConfig {
    bind_addr: string;
    api_key: string;
    openai_base_url: string;
}

export interface BackendRoutingConfig {
    strategy: BackendRoutingStrategy;
    min_request_interval_ms: number;
}

export interface BackendAppConfig {
    proxy: BackendProxyConfig;
    routing: BackendRoutingConfig;
}

export interface BackendProxyStatus {
    running: boolean;
    bind_addr: string;
    request_count: number;
    uptime_seconds: number;
}

export interface BackendRoutingStats {
    total_accounts: number;
    available_accounts: number;
    strategy: BackendRoutingStrategy;
    open_circuits: number;
    active_sessions: number;
}

export interface BackendAccountExport {
    version: string;
    exported_at: string;
    accounts: BackendAccount[];
}

export interface BackendValidationResult {
    valid: boolean;
    org_id: string | null;
    error: string | null;
}

export interface BackendUsageRefreshResult {
    account_id: string;
    usage: BackendUsageSnapshot | null;
    error: string | null;
}

type NoArgs = Record<string, never>;

/** Every backend command with its invoke() arguments and its result. */
export interface BackendCommandMap {
    add_account: { args: { request: CreateAccountRequest }; result: BackendAccount };
    update_account: { args: { request: UpdateAccountRequest }; result: BackendAccount };
    remove_account: { args: { id: string }; result: boolean };
    get_account: { args: { id: string }; result: BackendAccount | null };
    list_accounts: { args: NoArgs; result: BackendAccount[] };
    list_account_statuses: { args: NoArgs; result: BackendAccountStatus[] };
    toggle_account_enabled: { args: { id: string }; result: BackendAccount };
    get_account_usage: { args: { id: string }; result: BackendUsageSnapshot | null };
    refresh_all_usage: { args: NoArgs; result: BackendUsageRefreshResult[] };
    refresh_account_usage: { args: { id: string }; result: BackendUsageSnapshot };
    get_routing_stats: { args: NoArgs; result: BackendRoutingStats };
    set_routing_strategy: { args: { strategy: BackendRoutingStrategy }; result: null };
    clear_routing_sessions: { args: NoArgs; result: null };
    start_proxy_server: { args: { config: BackendProxyConfig }; result: BackendProxyStatus };
    stop_proxy_server: { args: NoArgs; result: null };
    get_proxy_status: { args: NoArgs; result: BackendProxyStatus };
    export_accounts: { args: NoArgs; result: BackendAccountExport };
    import_accounts: { args: { export: BackendAccountExport }; result: BackendAccount[] };
    validate_api_key: { args: { apiKey: string; orgId?: string | null }; result: BackendValidationResult };
    load_app_config: { args: NoArgs; result: BackendAppConfig };
    save_app_config: { args: { config: BackendAppConfig }; result: null };
    update_proxy_configuration: { args: { proxy: BackendProxyConfig }; result: null };
    update_routing_configuration: { args: { routing: BackendRoutingConfig }; result: null };
    get_data_directory: { args: NoArgs; result: string };
}

export type BackendCommand = keyof BackendCommandMap;

/** Runtime list of the command names, checked against the Rust handler list. */
export const BACKEND_COMMANDS = [
    'add_account',
    'update_account',
    'remove_account',
    'get_account',
    'list_accounts',
    'list_account_statuses',
    'toggle_account_enabled',
    'get_account_usage',
    'refresh_all_usage',
    'refresh_account_usage',
    'get_routing_stats',
    'set_routing_strategy',
    'clear_routing_sessions',
    'start_proxy_server',
    'stop_proxy_server',
    'get_proxy_status',
    'export_accounts',
    'import_accounts',
    'validate_api_key',
    'load_app_config',
    'save_app_config',
    'update_proxy_configuration',
    'update_routing_configuration',
    'get_data_directory',
] as const satisfies readonly BackendCommand[];

/** invoke() with the command name, arguments and result type checked. */
export function invokeBackend<C extends BackendCommand>(
    command: C,
    ...args: BackendCommandMap[C]['args'] extends NoArgs ? [] : [BackendCommandMap[C]['args']]
): Promise<BackendCommandMap[C]['result']> {
    return invoke<BackendCommandMap[C]['result']>(command, args[0]);
}
