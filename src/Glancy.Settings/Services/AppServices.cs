using Glancy.Core.Settings;
using Glancy.Core.Shell;
using Glancy.Core.Storage;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Services;

/// <summary>Application-wide services. Created once at startup; all UI runs on one thread.</summary>
public static class AppServices
{
    public static SettingsSession Session { get; private set; } = null!;
    public static AppPreferences Preferences { get; } = new();
    public static EngineStatus Engine { get; private set; } = new(false, null, []);

    /// <summary>False until the first engine status read finishes (it runs off the UI thread at startup).</summary>
    public static bool IsEngineStatusKnown { get; private set; }

    /// <summary>Settings copied with "Copy settings", available to paste into another format.</summary>
    public static SettingsClipboard? Clipboard { get; set; }

    public static event EventHandler? EngineStatusChanged;

    public static void Initialize(bool demo)
    {
        ISettingsStore store = new RegistrySettingsStore();
        try
        {
            Session = new SettingsSession(store);
        }
        catch (SettingsStoreException ex)
        {
            Log.Error("Couldn't read the settings; using defaults.", ex);
            Session = new SettingsSession(new MemorySettingsStore());
        }

        if (demo) ApplyDemoEdits();
        _ = RefreshEngineStatusAsync();
    }

    /// <summary>
    /// Re-reads the engine registration for every catalog extension. That is a few hundred registry reads, so it
    /// runs on a worker thread; the event is raised back on the UI thread (the caller's synchronization context).
    /// </summary>
    public static async Task RefreshEngineStatusAsync()
    {
        try
        {
            Engine = await EngineStatusService.GetStatusAsync();
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or System.Security.SecurityException or IOException)
        {
            Log.Error("Couldn't read the thumbnail engine status.", ex);
        }
        IsEngineStatusKnown = true;
        EngineStatusChanged?.Invoke(null, EventArgs.Empty);
    }

    /// <summary>QA/demo mode: a few pending edits so screenshots show the pending-changes bar. Never applied.</summary>
    private static void ApplyDemoEdits()
    {
        Session.Write(SettingsScope.ForFormat(Catalog.Photoshop), Catalog.Transparency, Values.Transparency.Checkerboard);
        Session.Write(SettingsScope.ForExtension(Catalog.Photoshop, ".psb"), Catalog.MaxFileSize, 1024);
        Session.Write(SettingsScope.ForFormat(Catalog.Svg), Catalog.ScaleMode, Values.Scale.ScaleToFit);
    }

    /// <summary>Fallback store when the registry is unreadable: the app still opens and explains the problem.</summary>
    private sealed class MemorySettingsStore : ISettingsStore
    {
        private SettingsSnapshot _snapshot = SettingsSnapshot.CreateDefaults();
        public SettingsSnapshot Load() => _snapshot.Clone();
        public void Save(SettingsSnapshot snapshot) =>
            throw new SettingsStoreException(AppStrings.Store_RegistryUnavailable);
    }
}
