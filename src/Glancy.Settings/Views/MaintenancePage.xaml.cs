using System.Diagnostics;
using Glancy.Core;
using Glancy.Core.Settings;
using Glancy.Core.Shell;
using Glancy.Core.Storage;
using Glancy.Settings.Helpers;
using Glancy.Settings.Services;
using Glancy.Settings.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Windows.Storage;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Views;

public sealed partial class MaintenancePage : Page
{
    public MaintenancePage() => InitializeComponent();

    public GlobalSettingsViewModel Global { get; } = new();

    public string LogDirectory => Log.Directory;

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        AppServices.EngineStatusChanged += OnEngineChanged;
        UpdateEngineCard();
        LoadHistory();
        base.OnNavigatedTo(e);
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        AppServices.EngineStatusChanged -= OnEngineChanged;
        Global.Dispose();
        base.OnNavigatedFrom(e);
    }

    private void OnEngineChanged(object? sender, EventArgs e) => UpdateEngineCard();

    private void UpdateEngineCard()
    {
        var engine = AppServices.Engine;
        EngineCard.Description = !AppServices.IsEngineStatusKnown ? AppStrings.Common_Checking
            : engine.IsInstalled
            ? string.Format(AppStrings.Maintenance_Engine_Installed, engine.DllPath, engine.ActiveExtensionCount, engine.Handlers.Count)
            : AppStrings.Maintenance_Engine_Missing;
    }

    private async void OnClearThumbnails(object sender, RoutedEventArgs e)
    {
        if (!await WindowHelper.ConfirmAsync(AppStrings.Maintenance_ClearThumbs_Confirm,
                AppStrings.Maintenance_ClearThumbs_Confirm_Message,
                AppStrings.Maintenance_ClearCache))
            return;

        ClearThumbsButton.IsEnabled = false;
        ClearThumbsProgress.IsActive = true;
        ClearThumbsProgress.Visibility = Visibility.Visible;
        try
        {
            var result = await CacheCleaner.ClearThumbnailCacheAsync();
            var detail = result.BytesFreed > 0 ? AppStrings.Sentence_Separator + string.Format(AppStrings.Cache_Freed, result.BytesFreed / 1024d / 1024d) : string.Empty;
            App.Current.Window?.Notify(result.Completed ? InfoBarSeverity.Success : InfoBarSeverity.Warning,
                result.Completed ? AppStrings.Cache_Cleared_Title : AppStrings.Cache_Failed_Title, result.Message + detail);
        }
        finally
        {
            ClearThumbsButton.IsEnabled = true;
            ClearThumbsProgress.IsActive = false;
            ClearThumbsProgress.Visibility = Visibility.Collapsed;
        }
    }

    private async void OnClearIcons(object sender, RoutedEventArgs e)
    {
        ClearIconsButton.IsEnabled = false;
        try
        {
            var result = await CacheCleaner.ClearIconCacheAsync();
            App.Current.Window?.Notify(result.Completed ? InfoBarSeverity.Success : InfoBarSeverity.Warning,
                result.Completed ? AppStrings.Icons_Refreshed_Title : AppStrings.Icons_Failed_Title, result.Message);
        }
        catch (System.ComponentModel.Win32Exception ex)
        {
            App.Current.Window?.Notify(InfoBarSeverity.Error, AppStrings.Icons_Failed_Title, ex.Message);
        }
        finally
        {
            ClearIconsButton.IsEnabled = true;
        }
    }

    private void OnOpenDiskCleanup(object sender, RoutedEventArgs e)
    {
        try { CacheCleaner.OpenDiskCleanup(); }
        catch (System.ComponentModel.Win32Exception ex)
        {
            App.Current.Window?.Notify(InfoBarSeverity.Error, AppStrings.DiskCleanup_Failed_Title, ex.Message);
        }
    }

    private void OnRefreshHistory(object sender, RoutedEventArgs e) => LoadHistory();

    private void LoadHistory()
    {
        var entries = EngineHistory.ReadRecent(12);
        HistoryList.Items.Clear();
        NoHistoryText.Visibility = entries.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        foreach (var entry in entries)
        {
            var row = new Grid { ColumnSpacing = 10, Padding = new Thickness(0, 3, 0, 3) };
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            row.ColumnDefinitions.Add(new ColumnDefinition());
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            row.Children.Add(new FontIcon
            {
                Glyph = entry.Succeeded ? "\uE73E" : "\uE783",
                FontSize = 12,
                Style = (Style)Application.Current.Resources[entry.Succeeded ? "GlancyIconSuccessStyle" : "GlancyIconCautionStyle"],
                VerticalAlignment = VerticalAlignment.Center,
            });
            var name = new TextBlock { Text = Path.GetFileName(entry.Path), TextTrimming = TextTrimming.CharacterEllipsis };
            ToolTipService.SetToolTip(name, entry.Detail is null ? entry.Path : $"{entry.Path}\n{entry.Detail}");
            Grid.SetColumn(name, 1);
            row.Children.Add(name);
            var meta = new TextBlock
            {
                Text = $"{entry.Time.ToLocalTime():HH:mm:ss} · {entry.Milliseconds} ms",
                Style = (Style)Application.Current.Resources["GlancyCaptionStyle"],
                VerticalAlignment = VerticalAlignment.Center,
            };
            Grid.SetColumn(meta, 2);
            row.Children.Add(meta);
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(row,
                string.Format(AppStrings.Maintenance_History_ItemName, Path.GetFileName(entry.Path), entry.Succeeded ? AppStrings.Common_Succeeded : AppStrings.Common_Failed, entry.Milliseconds));
            HistoryList.Items.Add(row);
        }
    }

    private async void OnExport(object sender, RoutedEventArgs e)
    {
        var file = await WindowHelper.PickSaveFileAsync($"Glancy {DateTime.Now:yyyy-MM-dd}", SettingsFile.FileTypeName, SettingsFile.Extension);
        if (file is null) return;
        try
        {
            await FileIO.WriteTextAsync(file, SettingsFile.Serialize(AppServices.Session.Working));
            App.Current.Window?.Notify(InfoBarSeverity.Success, AppStrings.Export_Done_Title, file.Path);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            App.Current.Window?.Notify(InfoBarSeverity.Error, AppStrings.Export_Failed_Title, ex.Message);
        }
    }

    private async void OnImport(object sender, RoutedEventArgs e)
    {
        var file = await WindowHelper.PickOpenFileAsync(SettingsFile.Extension, ".json");
        if (file is null) return;
        try
        {
            var snapshot = SettingsFile.Deserialize(await FileIO.ReadTextAsync(file), out var ignored);
            var pendingNote = AppServices.Session.IsDirty ? AppStrings.Sentence_Separator + AppStrings.Import_ReplacesPending : string.Empty;
            if (!await WindowHelper.ConfirmAsync(AppStrings.Import_Confirm,
                    string.Format(AppStrings.Import_Confirm_Message, file.Name, pendingNote),
                    AppStrings.Import_Button))
                return;

            AppServices.Session.LoadWorking(snapshot);
            var note = ignored > 0 ? AppStrings.Sentence_Separator + string.Format(AppStrings.Import_Ignored, ignored) : string.Empty;
            App.Current.Window?.Notify(InfoBarSeverity.Success, AppStrings.Import_Done_Title,
                AppStrings.Import_Done_Message + note);
        }
        catch (SettingsStoreException ex)
        {
            App.Current.Window?.Notify(InfoBarSeverity.Error, AppStrings.Import_Failed_Title, ex.Message);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or System.Runtime.InteropServices.COMException)
        {
            // COMException: ReadTextAsync rejects files that are not valid UTF-8/UTF-16 text.
            App.Current.Window?.Notify(InfoBarSeverity.Error, AppStrings.Import_Unreadable_Title,
                AppStrings.Import_Unreadable_Message);
        }
    }

    private async void OnResetAll(object sender, RoutedEventArgs e)
    {
        if (await WindowHelper.ConfirmAsync(AppStrings.Reset_Confirm,
                AppStrings.Reset_Confirm_Message,
                AppStrings.Reset_Button, destructive: true))
        {
            AppServices.Session.ResetAll();
        }
    }

    private void OnOpenLogs(object sender, RoutedEventArgs e)
    {
        Directory.CreateDirectory(Log.Directory);
        Process.Start(new ProcessStartInfo("explorer.exe", $"\"{Log.Directory}\"") { UseShellExecute = true });
    }

    private async void OnRecheck(object sender, RoutedEventArgs e)
    {
        await AppServices.RefreshEngineStatusAsync();
        App.Current.Window?.Notify(InfoBarSeverity.Informational, AppStrings.Status_Updated_Title,
            AppServices.Engine.IsInstalled ? AppStrings.Status_EngineInstalled : AppStrings.Status_EngineMissing);
    }
}
