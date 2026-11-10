import { useState, useEffect, useMemo } from 'react';
import { Search, RefreshCw, Download, Trash2, LayoutGrid, List, ToggleLeft, ToggleRight, Eye, EyeOff } from 'lucide-react';
import { useAccountStore, type NewAccount } from '../stores/useAccountStore';
import AccountCard from '../components/accounts/AccountCard';
import AccountDetailsDialog from '../components/accounts/AccountDetailsDialog';
import AddAccountDialog from '../components/accounts/AddAccountDialog';
import ModalDialog from '../components/common/ModalDialog';
import Pagination from '../components/common/Pagination';
import { showToast } from '../components/common/ToastContainer';
import { OpenAIAccount } from '../types/account';
import { cn } from '../utils/cn';
import { maskApiKey } from '../utils/format';
import { formatPercent, isLowQuota, ratioToPercent, requestQuotaRatio } from '../utils/quota';
import {
    clampPage,
    matchesSearch,
    pageCount,
    pruneSelection,
    safeFileName,
    serializeAccountsForExport,
} from '../utils/accountList';

const PAGE_SIZE_OPTIONS = [12, 24, 48, 96];

/** Save text as a file through a temporary link that is attached to the page. */
function downloadTextFile(fileName: string, text: string) {
    const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
    const link = document.createElement('a');
    link.href = url;
    link.download = fileName;
    document.body.appendChild(link);
    link.click();
    link.remove();
    // Give the download a moment to start before the URL is released.
    setTimeout(() => URL.revokeObjectURL(url), 1000);
}

type FilterType = 'all' | 'active' | 'disabled' | 'low-quota';
type ViewMode = 'list' | 'grid';

export default function Accounts() {
    const {
        accounts,
        currentAccount,
        fetchAccounts,
        addAccount,
        deleteAccount,
        deleteAccounts,
        switchAccount,
        loading,
        refreshQuota,
        toggleAccountStatus,
    } = useAccountStore();

    const [searchQuery, setSearchQuery] = useState('');
    const [filter, setFilter] = useState<FilterType>('all');
    const [viewMode, setViewMode] = useState<ViewMode>('grid');
    const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
    const [detailsAccount, setDetailsAccount] = useState<OpenAIAccount | null>(null);
    const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);
    const [isBatchDelete, setIsBatchDelete] = useState(false);
    const [toggleConfirm, setToggleConfirm] = useState<{ accountId: string; enable: boolean } | null>(null);
    const [hideDetails, setHideDetails] = useState(false);
    const [currentPage, setCurrentPage] = useState(1);
    const [itemsPerPage, setItemsPerPage] = useState(12);
    const [switchingId, setSwitchingId] = useState<string | null>(null);

    useEffect(() => {
        fetchAccounts();
    }, [fetchAccounts]);

    // Search and filter logic
    const searchedAccounts = useMemo(() => {
        if (!searchQuery) return accounts;
        return accounts.filter(a => matchesSearch(a, searchQuery));
    }, [accounts, searchQuery]);

    const filterCounts = useMemo(() => ({
        all: searchedAccounts.length,
        active: searchedAccounts.filter(a => a.isActive).length,
        disabled: searchedAccounts.filter(a => a.disabled).length,
        'low-quota': searchedAccounts.filter(isLowQuota).length,
    }), [searchedAccounts]);

    const filteredAccounts = useMemo(() => {
        switch (filter) {
            case 'active':
                return searchedAccounts.filter(a => a.isActive);
            case 'disabled':
                return searchedAccounts.filter(a => a.disabled);
            case 'low-quota':
                return searchedAccounts.filter(isLowQuota);
            default:
                return searchedAccounts;
        }
    }, [searchedAccounts, filter]);

    // The page is clamped to the current list, so narrowing the list by a
    // search, a filter or a delete can never leave an empty page on screen.
    const safePage = clampPage(currentPage, filteredAccounts.length, itemsPerPage);

    const paginatedAccounts = useMemo(() => {
        const startIndex = (safePage - 1) * itemsPerPage;
        return filteredAccounts.slice(startIndex, startIndex + itemsPerPage);
    }, [filteredAccounts, safePage, itemsPerPage]);

    // Batch actions only ever apply to accounts that still exist and are in
    // the list the user is looking at (not deleted, not hidden by a filter).
    const activeSelection = useMemo(
        () => pruneSelection(selectedIds, filteredAccounts.map(a => a.id)),
        [selectedIds, filteredAccounts],
    );

    const handleToggleSelect = (id: string) => {
        const newSet = new Set(selectedIds);
        if (newSet.has(id)) {
            newSet.delete(id);
        } else {
            newSet.add(id);
        }
        setSelectedIds(newSet);
    };

    const handleToggleAll = () => {
        const currentIds = paginatedAccounts.map(a => a.id);
        const allSelected = currentIds.every(id => selectedIds.has(id));
        
        const newSet = new Set(selectedIds);
        if (allSelected) {
            currentIds.forEach(id => newSet.delete(id));
        } else {
            currentIds.forEach(id => newSet.add(id));
        }
        setSelectedIds(newSet);
    };

    const handleAddAccount = async (account: NewAccount) => {
        await addAccount(account);
    };

    const handleSwitch = async (accountId: string) => {
        setSwitchingId(accountId);
        try {
            await switchAccount(accountId);
            showToast('Account switched successfully', 'success');
        } catch (error) {
            showToast(`Failed to switch: ${error}`, 'error');
        } finally {
            setSwitchingId(null);
        }
    };

    const handleRefresh = async (accountId: string) => {
        try {
            await refreshQuota(accountId);
            showToast('Usage refresh is not connected to the backend yet', 'info');
        } catch (error) {
            showToast(`Failed to refresh: ${error}`, 'error');
        }
    };

    const handleBatchDelete = () => {
        if (activeSelection.size === 0) return;
        setIsBatchDelete(true);
    };

    const executeBatchDelete = async () => {
        setIsBatchDelete(false);
        try {
            const ids = Array.from(activeSelection);
            await deleteAccounts(ids);
            setSelectedIds(new Set());
            showToast(`Deleted ${ids.length} accounts`, 'success');
        } catch (error) {
            showToast(`Failed to delete: ${error}`, 'error');
        }
    };

    const handleDelete = (accountId: string) => {
        setDeleteConfirmId(accountId);
    };

    const executeDelete = async () => {
        if (!deleteConfirmId) return;
        try {
            await deleteAccount(deleteConfirmId);
            const deletedId = deleteConfirmId;
            setSelectedIds(prev => new Set([...prev].filter(id => id !== deletedId)));
            showToast('Account deleted', 'success');
        } catch (error) {
            showToast(`Failed to delete: ${error}`, 'error');
        } finally {
            setDeleteConfirmId(null);
        }
    };

    const handleToggleStatus = (accountId: string, currentlyDisabled: boolean) => {
        setToggleConfirm({ accountId, enable: currentlyDisabled });
    };

    const executeToggleStatus = async () => {
        if (!toggleConfirm) return;
        try {
            await toggleAccountStatus(
                toggleConfirm.accountId,
                toggleConfirm.enable,
                toggleConfirm.enable ? undefined : 'User manually disabled'
            );
            showToast(toggleConfirm.enable ? 'Account enabled' : 'Account disabled', 'success');
        } catch (error) {
            showToast(`Failed to toggle: ${error}`, 'error');
        } finally {
            setToggleConfirm(null);
        }
    };

    const handleBatchToggleStatus = async (enable: boolean) => {
        const ids = Array.from(activeSelection);
        if (ids.length === 0) return;
        try {
            const promises = ids.map(id =>
                toggleAccountStatus(id, enable, enable ? undefined : 'Batch disabled')
            );
            await Promise.all(promises);
            showToast(
                enable 
                    ? `Enabled ${ids.length} accounts` 
                    : `Disabled ${ids.length} accounts`,
                'success'
            );
            setSelectedIds(new Set());
        } catch (error) {
            showToast(`Failed to toggle: ${error}`, 'error');
        }
    };

    const handleExport = () => {
        const accountsToExport = activeSelection.size > 0
            ? accounts.filter(a => activeSelection.has(a.id))
            : accounts;
        if (accountsToExport.length === 0) {
            showToast('There are no accounts to export', 'info');
            return;
        }
        downloadTextFile(
            `codex-accounts-${new Date().toISOString().split('T')[0]}.json`,
            serializeAccountsForExport(accountsToExport),
        );
        showToast(`Exported ${accountsToExport.length} account(s). The file contains API keys in plain text.`, 'warning', 6000);
    };

    const handleExportOne = (accountId: string) => {
        const account = accounts.find(a => a.id === accountId);
        if (!account) return;
        downloadTextFile(
            `account-${safeFileName(account.name)}-${new Date().toISOString().split('T')[0]}.json`,
            serializeAccountsForExport([account]),
        );
        showToast('Exported 1 account. The file contains the API key in plain text.', 'warning', 6000);
    };

    return (
        <div className="h-full flex flex-col gap-4 max-w-7xl mx-auto">
            {/* Toolbar */}
            <div className="flex flex-wrap items-center gap-3">
                {/* Search */}
                <div className="relative w-64">
                    <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 w-4 h-4 text-gray-400" />
                    <input
                        type="text"
                        placeholder="Search accounts..."
                        className="w-full pl-10 pr-4 py-2 bg-white dark:bg-gray-900 text-sm text-gray-900 dark:text-white border border-gray-200 dark:border-gray-800 rounded-xl focus:outline-none focus:ring-2 focus:ring-blue-500"
                        value={searchQuery}
                        onChange={(e) => setSearchQuery(e.target.value)}
                    />
                </div>

                {/* View Toggle */}
                <div className="flex gap-1 bg-gray-100 dark:bg-gray-800 p-1 rounded-xl">
                    <button
                        className={cn(
                            'p-2 rounded-lg transition-all',
                            viewMode === 'list'
                                ? 'bg-white dark:bg-gray-700 text-blue-600 dark:text-blue-400 shadow-sm'
                                : 'text-gray-500 dark:text-gray-400 hover:text-gray-900 dark:hover:text-gray-200'
                        )}
                        onClick={() => setViewMode('list')}
                    >
                        <List className="w-4 h-4" />
                    </button>
                    <button
                        className={cn(
                            'p-2 rounded-lg transition-all',
                            viewMode === 'grid'
                                ? 'bg-white dark:bg-gray-700 text-blue-600 dark:text-blue-400 shadow-sm'
                                : 'text-gray-500 dark:text-gray-400 hover:text-gray-900 dark:hover:text-gray-200'
                        )}
                        onClick={() => setViewMode('grid')}
                    >
                        <LayoutGrid className="w-4 h-4" />
                    </button>
                </div>

                {/* Filters */}
                <div className="flex gap-1 bg-gray-100 dark:bg-gray-800 p-1 rounded-xl">
                    {(['all', 'active', 'disabled', 'low-quota'] as FilterType[]).map((f) => (
                        <button
                            key={f}
                            className={cn(
                                'px-3 py-1.5 rounded-lg text-xs font-medium transition-all flex items-center gap-1.5',
                                filter === f
                                    ? 'bg-white dark:bg-gray-700 text-blue-600 dark:text-blue-400 shadow-sm'
                                    : 'text-gray-500 dark:text-gray-400 hover:text-gray-900 dark:hover:text-gray-200'
                            )}
                            onClick={() => setFilter(f)}
                        >
                            {f.charAt(0).toUpperCase() + f.slice(1).replace('-', ' ')}
                            <span className={cn(
                                'px-1.5 py-0.5 rounded-md text-[10px] font-bold',
                                filter === f
                                    ? 'bg-blue-100 dark:bg-blue-900/30 text-blue-600 dark:text-blue-400'
                                    : 'bg-gray-200 dark:bg-gray-700 text-gray-500 dark:text-gray-400'
                            )}>
                                {filterCounts[f]}
                            </span>
                        </button>
                    ))}
                </div>

                <div className="flex-1" />

                {/* Actions */}
                <div className="flex items-center gap-2">
                    <button
                        onClick={() => setHideDetails(!hideDetails)}
                        className={cn(
                            'px-3 py-2 text-xs font-medium rounded-xl transition-colors flex items-center gap-1.5',
                            hideDetails
                                ? 'bg-gray-200 dark:bg-gray-800 text-gray-600 dark:text-gray-400'
                                : 'bg-gray-100 dark:bg-gray-800 text-gray-600 dark:text-gray-400 hover:bg-gray-200 dark:hover:bg-gray-700'
                        )}
                    >
                        {hideDetails ? <EyeOff className="w-3.5 h-3.5" /> : <Eye className="w-3.5 h-3.5" />}
                        {hideDetails ? 'Show' : 'Hide'}
                    </button>

                    <AddAccountDialog onAdd={handleAddAccount} />

                    {activeSelection.size > 0 && (
                        <>
                            <button
                                className="px-3 py-2 bg-rose-500 text-white text-xs font-medium rounded-xl hover:bg-rose-600 transition-colors flex items-center gap-1.5"
                                onClick={handleBatchDelete}
                            >
                                <Trash2 className="w-3.5 h-3.5" />
                                Delete ({activeSelection.size})
                            </button>
                            <button
                                className="px-3 py-2 bg-amber-500 text-white text-xs font-medium rounded-xl hover:bg-amber-600 transition-colors flex items-center gap-1.5"
                                onClick={() => handleBatchToggleStatus(false)}
                            >
                                <ToggleLeft className="w-3.5 h-3.5" />
                                Disable ({activeSelection.size})
                            </button>
                            <button
                                className="px-3 py-2 bg-emerald-500 text-white text-xs font-medium rounded-xl hover:bg-emerald-600 transition-colors flex items-center gap-1.5"
                                onClick={() => handleBatchToggleStatus(true)}
                            >
                                <ToggleRight className="w-3.5 h-3.5" />
                                Enable ({activeSelection.size})
                            </button>
                        </>
                    )}

                    <button
                        className="px-3 py-2 bg-blue-500 text-white text-xs font-medium rounded-xl hover:bg-blue-600 transition-colors flex items-center gap-1.5"
                        onClick={() => fetchAccounts()}
                        disabled={loading}
                    >
                        <RefreshCw className={cn('w-3.5 h-3.5', loading && 'animate-spin')} />
                        Refresh
                    </button>

                    <button
                        className="px-3 py-2 border border-gray-200 dark:border-gray-800 text-gray-700 dark:text-gray-300 text-xs font-medium rounded-xl hover:bg-gray-50 dark:hover:bg-gray-800 transition-colors flex items-center gap-1.5"
                        onClick={handleExport}
                    >
                        <Download className="w-3.5 h-3.5" />
                        Export
                    </button>
                </div>
            </div>

            {/* Account Grid */}
            <div className="flex-1 min-h-0 overflow-auto">
                {viewMode === 'grid' ? (
                    <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4">
                        {paginatedAccounts.map((account) => (
                            <AccountCard
                                key={account.id}
                                account={account}
                                selected={selectedIds.has(account.id)}
                                onSelect={() => handleToggleSelect(account.id)}
                                isCurrent={currentAccount?.id === account.id}
                                isRefreshing={false}
                                isSwitching={switchingId === account.id}
                                onSwitch={() => handleSwitch(account.id)}
                                onRefresh={() => handleRefresh(account.id)}
                                onViewDetails={() => setDetailsAccount(account)}
                                onExport={() => handleExportOne(account.id)}
                                onDelete={() => handleDelete(account.id)}
                                onToggleStatus={() => handleToggleStatus(account.id, !!account.disabled)}
                            />
                        ))}
                    </div>
                ) : (
                    <div className="bg-white dark:bg-gray-900 rounded-2xl border border-gray-200 dark:border-gray-800 overflow-hidden">
                        <table className="w-full">
                            <thead className="bg-gray-50 dark:bg-gray-800">
                                <tr>
                                    <th className="px-4 py-3 text-left">
                                        <input
                                            type="checkbox"
                                            checked={paginatedAccounts.length > 0 && paginatedAccounts.every(a => selectedIds.has(a.id))}
                                            onChange={handleToggleAll}
                                            className="w-4 h-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                                        />
                                    </th>
                                    <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Name</th>
                                    <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Status</th>
                                    <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Quota</th>
                                    <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Last Used</th>
                                    <th className="px-4 py-3 text-right text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Actions</th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-gray-200 dark:divide-gray-800">
                                {paginatedAccounts.map((account) => (
                                    <tr key={account.id} className="hover:bg-gray-50 dark:hover:bg-gray-800/50">
                                        <td className="px-4 py-3">
                                            <input
                                                type="checkbox"
                                                checked={selectedIds.has(account.id)}
                                                onChange={() => handleToggleSelect(account.id)}
                                                className="w-4 h-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                                            />
                                        </td>
                                        <td className="px-4 py-3">
                                            <div className="font-medium text-gray-900 dark:text-white">{account.name}</div>
                                            <div className="text-xs text-gray-500">{maskApiKey(account.apiKey)}</div>
                                        </td>
                                        <td className="px-4 py-3">
                                            <div className="flex gap-2">
                                                {account.isActive && (
                                                    <span className="px-2 py-0.5 text-xs font-medium bg-emerald-100 dark:bg-emerald-900/30 text-emerald-700 dark:text-emerald-400 rounded-full">Active</span>
                                                )}
                                                {account.disabled && (
                                                    <span className="px-2 py-0.5 text-xs font-medium bg-rose-100 dark:bg-rose-900/30 text-rose-700 dark:text-rose-400 rounded-full">Disabled</span>
                                                )}
                                                {account.quota?.tier && (
                                                    <span className="px-2 py-0.5 text-xs font-medium bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-400 rounded-full uppercase">
                                                        {account.quota.tier}
                                                    </span>
                                                )}
                                            </div>
                                        </td>
                                        <td className="px-4 py-3">
                                            <div className="text-sm text-gray-900 dark:text-white">
                                                {formatPercent(ratioToPercent(requestQuotaRatio(account.quota)))}
                                            </div>
                                            <div className="text-xs text-gray-500">
                                                {account.quota?.remainingRequests || 0} / {account.quota?.totalRequests || 0}
                                            </div>
                                        </td>
                                        <td className="px-4 py-3 text-sm text-gray-500">
                                            {account.lastUsed 
                                                ? new Date(account.lastUsed * 1000).toLocaleDateString() 
                                                : 'Never'}
                                        </td>
                                        <td className="px-4 py-3 text-right">
                                            <div className="flex items-center justify-end gap-1">
                                                <button
                                                    onClick={() => setDetailsAccount(account)}
                                                    className="p-1.5 text-gray-400 hover:text-blue-600 rounded-lg hover:bg-blue-50"
                                                >
                                                    Details
                                                </button>
                                                <button
                                                    onClick={() => handleSwitch(account.id)}
                                                    disabled={switchingId === account.id}
                                                    className="p-1.5 text-gray-400 hover:text-emerald-600 rounded-lg hover:bg-emerald-50 disabled:opacity-50"
                                                >
                                                    Switch
                                                </button>
                                                <button
                                                    onClick={() => handleDelete(account.id)}
                                                    className="p-1.5 text-gray-400 hover:text-rose-600 rounded-lg hover:bg-rose-50"
                                                >
                                                    Delete
                                                </button>
                                            </div>
                                        </td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                )}
            </div>

            {/* Pagination */}
            {filteredAccounts.length > 0 && (
                <Pagination
                    currentPage={safePage}
                    totalPages={pageCount(filteredAccounts.length, itemsPerPage)}
                    onPageChange={setCurrentPage}
                    totalItems={filteredAccounts.length}
                    itemsPerPage={itemsPerPage}
                    onPageSizeChange={(size) => {
                        setItemsPerPage(size);
                        setCurrentPage(1);
                    }}
                    pageSizeOptions={PAGE_SIZE_OPTIONS}
                />
            )}

            {/* Dialogs */}
            <AccountDetailsDialog
                account={detailsAccount}
                onClose={() => setDetailsAccount(null)}
            />

            <ModalDialog
                isOpen={!!deleteConfirmId || isBatchDelete}
                title={isBatchDelete ? 'Delete Multiple Accounts' : 'Delete Account'}
                message={isBatchDelete 
                    ? `Are you sure you want to delete ${activeSelection.size} accounts? This action cannot be undone.`
                    : 'Are you sure you want to delete this account? This action cannot be undone.'}
                type="confirm"
                confirmText="Delete"
                isDestructive={true}
                onConfirm={isBatchDelete ? executeBatchDelete : executeDelete}
                onCancel={() => { setDeleteConfirmId(null); setIsBatchDelete(false); }}
            />

            {toggleConfirm && (
                <ModalDialog
                    isOpen={!!toggleConfirm}
                    onCancel={() => setToggleConfirm(null)}
                    onConfirm={executeToggleStatus}
                    title={toggleConfirm.enable ? 'Enable Account' : 'Disable Account'}
                    message={toggleConfirm.enable 
                        ? 'Are you sure you want to enable this account?'
                        : 'Are you sure you want to disable this account?'}
                />
            )}
        </div>
    );
}
