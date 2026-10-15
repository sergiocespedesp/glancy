using Glancy.Core.Shell;
using Glancy.Settings.Helpers;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Dialogs;

/// <summary>Choose an icon from an .ico, .exe or .dll file. Returns "path,index" or null if cancelled.</summary>
public static class IconPickerDialog
{
    private static readonly string DefaultLibrary = Path.Combine(Environment.SystemDirectory, "imageres.dll");

    public static async Task<string?> PickAsync(string current)
    {
        var (path, index) = string.IsNullOrWhiteSpace(current) ? (DefaultLibrary, 0) : IconExtractor.ParseLocation(current);

        var pathBox = new TextBox { Header = AppStrings.IconPicker_File, Text = path, IsReadOnly = true };
        var browse = new Button { Content = AppStrings.IconPicker_Browse, VerticalAlignment = VerticalAlignment.Bottom };
        var status = new TextBlock { Style = (Style)Application.Current.Resources["GlancyCaptionStyle"] };
        var grid = new GridView
        {
            SelectionMode = ListViewSelectionMode.Single,
            Height = 300,
            IsItemClickEnabled = false,
        };
        AutomationProperties.SetName(grid, AppStrings.IconPicker_Available);

        var top = new Grid { ColumnSpacing = 8 };
        top.ColumnDefinitions.Add(new ColumnDefinition());
        top.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(browse, 1);
        top.Children.Add(pathBox);
        top.Children.Add(browse);

        var content = new StackPanel { Spacing = 12, Width = 460 };
        content.Children.Add(top);
        content.Children.Add(grid);
        content.Children.Add(status);

        var dialog = new ContentDialog
        {
            Title = AppStrings.IconPicker_Title,
            Content = content,
            PrimaryButtonText = AppStrings.IconPicker_Use,
            CloseButtonText = AppStrings.Common_Cancel,
            DefaultButton = ContentDialogButton.Primary,
        };

        var loadVersion = 0;
        async Task Load(string file, int select)
        {
            var version = ++loadVersion;
            grid.Items.Clear();
            status.Text = AppStrings.IconPicker_Loading;
            var icons = await Task.Run(() => IconExtractor.ExtractAll(file, 32));
            if (version != loadVersion) return; // the user chose another file meanwhile
            foreach (var icon in icons)
            {
                var image = new Image { Width = 32, Height = 32, Source = CheckerBitmap.FromPixels(icon.Width, icon.Height, icon.PremultipliedBgra) };
                var item = new GridViewItem { Content = image, Tag = icon.Index, Padding = new Thickness(8) };
                AutomationProperties.SetName(item, string.Format(AppStrings.IconPicker_Item, icon.Index));
                grid.Items.Add(item);
            }
            grid.SelectedItem = grid.Items.Cast<GridViewItem>().FirstOrDefault(i => (int)i.Tag == select) ?? grid.Items.FirstOrDefault();
            status.Text = icons.Count == 0 ? AppStrings.IconPicker_None : string.Format(AppStrings.IconPicker_Count, icons.Count);
            dialog.IsPrimaryButtonEnabled = icons.Count > 0;
        }

        var chosenByDoubleTap = false;
        grid.DoubleTapped += (_, _) =>
        {
            if (grid.SelectedItem is null) return;
            chosenByDoubleTap = true;
            dialog.Hide();
        };
        browse.Click += async (_, _) =>
        {
            var file = await WindowHelper.PickOpenFileAsync(".ico", ".exe", ".dll");
            if (file is null) return;
            pathBox.Text = file.Path;
            await Load(file.Path, 0);
        };

        _ = Load(path, index);
        var result = await WindowHelper.ShowDialogAsync(dialog);
        var accepted = result == ContentDialogResult.Primary || chosenByDoubleTap;
        return accepted && grid.SelectedItem is GridViewItem selected
            ? IconExtractor.FormatLocation(pathBox.Text, (int)selected.Tag)
            : null;
    }
}
