using Glancy.Core.Settings;
using Glancy.Core.Shell;
using Glancy.Core.Storage;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Services;

public sealed record QuickActionResult(bool Succeeded, string Title, string Message);

/// <summary>
/// The taskbar Jump List: navigation tasks (<c>--page</c>) and quick actions (<c>--action</c>) that run without
/// opening the window and report through a Windows notification. With the window open, the running instance runs
/// the action so its pending-changes session stays in sync (see <see cref="Program"/>).
/// </summary>
public static class QuickActions
{
    public const string Enable = "enable";
    public const string Disable = "disable";
    public const string ClearCache = "clear-cache";
    public const string RefreshExplorer = "refresh-explorer";

    private static readonly SemaphoreSlim PublishGate = new(1, 1);
    private static bool? _publishedEnabled;

    public static bool IsKnown(string? action) => action is Enable or Disable or ClearCache or RefreshExplorer;

    /// <summary>Runs an action in a process without a window (no settings window was open).</summary>
    public static async Task<bool> RunHeadlessAsync(string action)
    {
        SettingsSession session;
        try
        {
            session = new SettingsSession(new RegistrySettingsStore());
        }
        catch (SettingsStoreException ex)
        {
            Log.Error("Quick action: couldn't read the settings.", ex);
            Toast.Show(AppStrings.QuickAction_Failed, ex.Message);
            return false;
        }
        var result = await RunAsync(action, session);
        Toast.Show(result.Title, result.Message);
        await PublishAsync(session);
        return result.Succeeded;
    }

    public static async Task<QuickActionResult> RunAsync(string action, SettingsSession session)
    {
        Log.Info($"Quick action: {action}");
        try
        {
            return action switch
            {
                Enable => await SetEnabledAsync(session, true),
                Disable => await SetEnabledAsync(session, false),
                ClearCache => await ClearCacheAsync(),
                RefreshExplorer => await RefreshAsync(session),
                _ => new QuickActionResult(false, AppStrings.QuickAction_Unknown, action),
            };
        }
        catch (Exception ex) when (ex is SettingsStoreException or System.Runtime.InteropServices.COMException or IOException or UnauthorizedAccessException)
        {
            Log.Error($"Quick action '{action}' failed", ex);
            return new QuickActionResult(false, AppStrings.QuickAction_Failed, ex.Message);
        }
    }

    /// <summary>Writes the Jump List if the on/off task's wording changed. Failures are logged, never thrown.</summary>
    public static async Task PublishAsync(SettingsSession session)
    {
        var enabled = IsOn(session.Applied, Catalog.Enabled);
        await PublishGate.WaitAsync();
        try
        {
            if (_publishedEnabled == enabled || Environment.ProcessPath is not { } exe) return;
            await JumpList.SetTasksAsync(exe, BuildTasks(enabled));
            _publishedEnabled = enabled;
        }
        catch (Exception ex) when (ex is System.Runtime.InteropServices.COMException or InvalidCastException or UnauthorizedAccessException)
        {
            Log.Error("Couldn't update the Jump List", ex);
        }
        finally
        {
            PublishGate.Release();
        }
    }

    public static IReadOnlyList<JumpListTask> BuildTasks(bool enabled) =>
    [
        enabled
            ? new(AppStrings.JumpList_Disable, $"--action {Disable}", AppStrings.JumpList_Disable_Desc)
            : new(AppStrings.JumpList_Enable, $"--action {Enable}", AppStrings.JumpList_Enable_Desc),
        new(AppStrings.JumpList_Refresh, $"--action {RefreshExplorer}", AppStrings.JumpList_Refresh_Desc),
        new(AppStrings.JumpList_ClearCache, $"--action {ClearCache}", AppStrings.JumpList_ClearCache_Desc),
        JumpListTask.Separator,
        new(AppStrings.Nav_AllFormats, "--page all"),
        new(AppStrings.JumpList_ExplorerOptions, "--page explorer"),
        new(AppStrings.Nav_Maintenance, "--page maintenance"),
    ];

    private static async Task<QuickActionResult> SetEnabledAsync(SettingsSession session, bool enabled)
    {
        session.ApplyGlobalNow(Catalog.Enabled, enabled ? 1 : 0);
        Log.Info($"Glancy thumbnails turned {(enabled ? "on" : "off")} from the Jump List.");
        _ = PublishAsync(session);

        var title = enabled ? AppStrings.QuickAction_On : AppStrings.QuickAction_Off;
        if (!(await EngineStatusService.GetStatusAsync()).IsInstalled)
            return new(true, title, AppStrings.QuickAction_EngineMissing);
        if (!IsOn(session.Applied, Catalog.RefreshOnApply))
            return new(true, title, AppStrings.QuickAction_NoRefresh);

        var refreshed = await RefreshCoreAsync(session, forceExtraction: true);
        return new(true, title, refreshed is null
            ? AppStrings.QuickAction_PressF5
            : AppStrings.Apply_Applied_Refreshed);
    }

    private static async Task<QuickActionResult> ClearCacheAsync()
    {
        var result = await CacheCleaner.ClearThumbnailCacheAsync();
        var detail = result.BytesFreed > 0 ? AppStrings.Sentence_Separator + string.Format(AppStrings.Cache_Freed, result.BytesFreed / 1024d / 1024d) : string.Empty;
        return new(result.Completed, result.Completed ? AppStrings.Cache_Cleared_Title : AppStrings.Cache_Failed_Title, result.Message + detail);
    }

    private static async Task<QuickActionResult> RefreshAsync(SettingsSession session)
    {
        var installed = (await EngineStatusService.GetStatusAsync()).IsInstalled;
        if (await RefreshCoreAsync(session, installed) is not { } result)
            return new(false, AppStrings.Refresh_Stopped_Title, AppStrings.QuickAction_RefreshStopped);
        var limit = result.LimitReached ? AppStrings.Sentence_Separator + string.Format(AppStrings.Refresh_LimitReached, ExplorerRefresher.MaxFiles) : string.Empty;
        return new(true, AppStrings.Refresh_Done_Title, string.Format(AppStrings.Refresh_Done_Message, result.FileCount, result.FolderCount) + limit);
    }

    /// <summary>Same refresh as applying in the window; null if it timed out (a hung provider must not hang us).</summary>
    private static async Task<RefreshResult?> RefreshCoreAsync(SettingsSession session, bool forceExtraction)
    {
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(90));
        try
        {
            return await ExplorerRefresher.RefreshAsync(Catalog.AllExtensions, IsOn(session.Applied, Catalog.FolderRecursion),
                forceExtraction, timeout.Token).WaitAsync(timeout.Token);
        }
        catch (OperationCanceledException)
        {
            return null;
        }
        catch (Exception ex)
        {
            // Refreshing is best effort (COM, file system, shell automation), as when applying in the window.
            Log.Error("Quick action: File Explorer refresh failed", ex);
            return null;
        }
    }

    private static bool IsOn(SettingsSnapshot snapshot, SettingDefinition setting) =>
        snapshot.Global[setting.Key] is int i && i != 0;
}
