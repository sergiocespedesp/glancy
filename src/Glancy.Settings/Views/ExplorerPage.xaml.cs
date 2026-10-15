using Glancy.Core.Settings;
using Glancy.Settings.Helpers;
using Glancy.Settings.Services;
using Glancy.Settings.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Views;

public sealed partial class ExplorerPage : Page
{
    public ExplorerPage() => InitializeComponent();

    public GlobalSettingsViewModel Global { get; } = new();

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        Global.Changed += OnGlobalChanged;
        RebuildFolders();
        RebuildLibrary();
        base.OnNavigatedTo(e);
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        Global.Changed -= OnGlobalChanged;
        Global.Dispose();
        base.OnNavigatedFrom(e);
    }

    private void OnGlobalChanged(object? sender, EventArgs e)
    {
        RebuildFolders();
        RebuildLibrary();
    }

    // ---- blocked folders

    private void RebuildFolders()
    {
        var folders = Global.BlockedFolders.ListValue;
        FoldersList.Items.Clear();
        NoFoldersText.Visibility = folders.Length == 0 ? Visibility.Visible : Visibility.Collapsed;
        foreach (var folder in folders)
        {
            var row = new Grid { ColumnSpacing = 12, Padding = new Thickness(0, 4, 0, 4) };
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            row.ColumnDefinitions.Add(new ColumnDefinition());
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            row.Children.Add(new FontIcon { Glyph = "", FontSize = 16, VerticalAlignment = VerticalAlignment.Center });
            var text = new TextBlock { Text = folder, TextTrimming = TextTrimming.CharacterEllipsis, VerticalAlignment = VerticalAlignment.Center };
            ToolTipService.SetToolTip(text, folder);
            Grid.SetColumn(text, 1);
            row.Children.Add(text);
            var remove = new Button { Content = new FontIcon { Glyph = "", FontSize = 12 }, Style = (Style)Application.Current.Resources["GlancySubtleIconButtonStyle"] };
            AutomationProperties.SetName(remove, string.Format(AppStrings.Explorer_RemoveFolder_Name, folder));
            ToolTipService.SetToolTip(remove, AppStrings.Explorer_RemoveFromList);
            remove.Click += (_, _) => Global.BlockedFolders.ListValue = Global.BlockedFolders.ListValue.Where(f => f != folder).ToArray();
            Grid.SetColumn(remove, 2);
            row.Children.Add(remove);
            FoldersList.Items.Add(row);
        }
    }

    private async void OnAddFolder(object sender, RoutedEventArgs e)
    {
        var folder = await WindowHelper.PickFolderAsync();
        if (folder is null) return;
        var current = Global.BlockedFolders.ListValue;
        if (current.Contains(folder.Path, StringComparer.OrdinalIgnoreCase))
        {
            App.Current.Window?.Notify(InfoBarSeverity.Informational, AppStrings.Explorer_AlreadyExcluded, folder.Path);
            return;
        }
        Global.BlockedFolders.ListValue = [.. current, folder.Path];
    }

    // ---- checkerboard library

    private void RebuildLibrary()
    {
        LibraryView.Items.Clear();
        foreach (var text in Global.CheckerLibrary.ListValue)
        {
            if (!CheckerColors.TryParse(text, out var pair)) continue;
            var swatch = new Border
            {
                Style = (Style)Application.Current.Resources["GlancySwatchStyle"],
                Child = new Image { Source = CheckerBitmap.Create(pair, 56, 56, 8), Width = 56, Height = 56 },
            };
            var item = new GridViewItem { Content = swatch, Tag = text };
            AutomationProperties.SetName(item, CheckerName(text, pair));
            ToolTipService.SetToolTip(item, $"{ColorHelper.ToHex(pair.Light)} / {ColorHelper.ToHex(pair.Dark)}");
            LibraryView.Items.Add(item);
        }
        RemoveSwatchesButton.IsEnabled = false;
    }

    /// <summary>Readable names for the built-in checkerboards; custom ones are named by their colors.</summary>
    internal static string CheckerName(string text, CheckerColors pair) =>
        Array.IndexOf(Catalog.DefaultCheckerLibrary, text.ToUpperInvariant()) switch
        {
            0 => AppStrings.Checker_WhiteLightGray,
            1 => AppStrings.Checker_MidGrays,
            2 => AppStrings.Checker_DarkGrays,
            _ => string.Format(AppStrings.Checker_Pair, ColorHelper.ToHex(pair.Light), ColorHelper.ToHex(pair.Dark)),
        };

    private void OnSwatchSelectionChanged(object sender, SelectionChangedEventArgs e) =>
        RemoveSwatchesButton.IsEnabled = LibraryView.SelectedItems.Count > 0 && LibraryView.SelectedItems.Count < LibraryView.Items.Count;

    private void OnRemoveSwatches(object sender, RoutedEventArgs e)
    {
        var remove = LibraryView.SelectedItems.Cast<GridViewItem>().Select(i => (string)i.Tag).ToHashSet();
        Global.CheckerLibrary.ListValue = Global.CheckerLibrary.ListValue.Where(t => !remove.Contains(t)).ToArray();
    }

    private void OnRestoreLibrary(object sender, RoutedEventArgs e) => Global.CheckerLibrary.Reset();
}
