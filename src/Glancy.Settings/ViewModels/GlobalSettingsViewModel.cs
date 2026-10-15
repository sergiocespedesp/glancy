using Glancy.Core.Settings;
using Glancy.Settings.Services;

namespace Glancy.Settings.ViewModels;

/// <summary>System-wide settings (Explorer integration, checkerboard, privacy, diagnostics).</summary>
public sealed class GlobalSettingsViewModel : ObservableObject, IDisposable
{
    private readonly SettingsSession _session = AppServices.Session;
    private readonly List<SettingItem> _items = [];

    public GlobalSettingsViewModel()
    {
        Enabled = Item(Catalog.Enabled);
        ForceDesktopTransparency = Item(Catalog.ForceDesktopTransparency);
        RefreshOnApply = Item(Catalog.RefreshOnApply);
        ContextMenu = Item(Catalog.ContextMenu);
        ContextMenuShift = Item(Catalog.ContextMenuRequireShift);
        FolderRecursion = Item(Catalog.FolderRecursion);
        CheckerAnchor = Item(Catalog.CheckerAnchor);
        CheckerScale = Item(Catalog.CheckerScale);
        CheckerLibrary = Item(Catalog.CheckerLibrary);
        BlockedFolders = Item(Catalog.BlockedFolders);
        DebugLog = Item(Catalog.DebugLog);
        _session.Changed += OnSessionChanged;
    }

    public SettingItem Enabled { get; }
    public SettingItem ForceDesktopTransparency { get; }
    public SettingItem RefreshOnApply { get; }
    public SettingItem ContextMenu { get; }
    public SettingItem ContextMenuShift { get; }
    public SettingItem FolderRecursion { get; }
    public SettingItem CheckerAnchor { get; }
    public SettingItem CheckerScale { get; }
    public SettingItem CheckerLibrary { get; }
    public SettingItem BlockedFolders { get; }
    public SettingItem DebugLog { get; }

    /// <summary>The Shift and recursion options only matter while the context menu is enabled.</summary>
    public bool IsContextMenuEnabled => ContextMenu.SelectedOption?.Value != Values.ContextMenu.Disabled;

    public event EventHandler? Changed;

    public void Dispose() => _session.Changed -= OnSessionChanged;

    private SettingItem Item(SettingDefinition d)
    {
        var item = new SettingItem(_session, d, null);
        _items.Add(item);
        return item;
    }

    private void OnSessionChanged(object? sender, EventArgs e)
    {
        foreach (var item in _items) item.Refresh();
        OnPropertyChanged(nameof(IsContextMenuEnabled));
        Changed?.Invoke(this, EventArgs.Empty);
    }
}
