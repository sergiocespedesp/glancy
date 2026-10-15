using Glancy.Core.Settings;
using Glancy.Core.Shell;
using Glancy.Settings.Controls;
using Glancy.Settings.Dialogs;
using Glancy.Settings.Helpers;
using Glancy.Settings.Services;
using Glancy.Settings.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Navigation;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Views;

/// <summary>Navigation parameter: "psd", "psd|.psb" or "all" (MainWindow handles an optional "|focus=key").</summary>
public sealed partial class FormatPage : Page
{
    private const double InlinePreviewBelow = 900;

    public FormatPage() => InitializeComponent();

    public FormatPageViewModel ViewModel { get; private set; } = null!;

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        var parts = (e.Parameter as string ?? "all").Split('|');
        var format = Catalog.FindFormat(parts[0]);
        var extension = parts.Skip(1).FirstOrDefault(p => p.StartsWith('.'));

        ViewModel = new FormatPageViewModel(format, extension is not null && format?.Owns(extension) == true ? extension : null);
        ViewModel.PropertyChanged += (_, args) =>
        {
            if (args.PropertyName == nameof(FormatPageViewModel.Preview)) UpdatePreview();
            if (args.PropertyName == nameof(FormatPageViewModel.ShowCustomIconPath)) UpdateCustomIcon();
        };
        ViewModel.CustomIconPath.PropertyChanged += (_, args) =>
        {
            if (args.PropertyName == nameof(SettingItem.TextValue)) UpdateCustomIcon();
        };
        AppServices.EngineStatusChanged += OnEngineStatusChanged;

        Bindings.Update();
        AdvancedSummary.Text = SentenceList(
            new[] { ViewModel.Sampling, ViewModel.Gamma, ViewModel.Alpha }.Where(i => i.IsSupported).Select(i => i.Title).ToList());
        ExtensionsHost.Child = ViewModel.Format is { } f
            ? ExtensionList.Create(f.Extensions)
            : new TextBlock { Text = ViewModel.ExtensionsText, Style = Res<Style>("GlancyCaptionStyle") };
        BuildScopeBar(extension);
        UpdatePreview();
        UpdateEngineStatus();
        UpdateCustomIcon();
        base.OnNavigatedTo(e);
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        AppServices.EngineStatusChanged -= OnEngineStatusChanged;
        ViewModel.Dispose();
        base.OnNavigatedFrom(e);
    }

    /// <summary>Up to this many extensions the scope is a SelectorBar; above, a ComboBox (RAW has 35, Blender 33).</summary>
    private const int MaxSelectorBarExtensions = 5;

    private void BuildScopeBar(string? selectedExtension)
    {
        ScopeBar.Items.Clear();
        ScopeCombo.Items.Clear();
        if (ViewModel.Format is not { } format || format.Extensions.Count < 2) return;

        if (format.Extensions.Count > MaxSelectorBarExtensions)
        {
            ScopeBar.Visibility = Visibility.Collapsed;
            ScopeCombo.Visibility = Visibility.Visible;
            ScopeCombo.Items.Add(new ComboBoxItem { Content = string.Format(AppStrings.Format_AllExtensions_Count, format.Extensions.Count), Tag = null });
            foreach (var ext in format.Extensions) ScopeCombo.Items.Add(new ComboBoxItem { Content = ext, Tag = ext });
            RefreshScopeMarkers();
            ScopeCombo.SelectedItem = ScopeCombo.Items.OfType<ComboBoxItem>().FirstOrDefault(i => (string?)i.Tag == selectedExtension) ?? ScopeCombo.Items[0];
        }
        else
        {
            ScopeBar.Items.Add(new SelectorBarItem { Text = AppStrings.Format_AllExtensions, Tag = null });
            foreach (var ext in format.Extensions) ScopeBar.Items.Add(new SelectorBarItem { Text = ext, Tag = ext });
            RefreshScopeMarkers();
            ScopeBar.SelectedItem = ScopeBar.Items.FirstOrDefault(i => (string?)i.Tag == selectedExtension) ?? ScopeBar.Items[0];
        }
        ViewModel.PropertyChanged += (_, args) =>
        {
            if (args.PropertyName == nameof(FormatPageViewModel.Preview)) RefreshScopeMarkers();
        };
    }

    /// <summary>Extensions with their own values are marked so customizations are visible without opening them.</summary>
    private void RefreshScopeMarkers()
    {
        foreach (var item in ScopeBar.Items) MarkScopeItem(item, item.Tag as string, text => item.Text = text);
        foreach (var item in ScopeCombo.Items.OfType<ComboBoxItem>()) MarkScopeItem(item, item.Tag as string, text => item.Content = text);
    }

    private static void MarkScopeItem(DependencyObject item, string? ext, Action<string> setText)
    {
        if (ext is null) return;
        var custom = AppServices.Session.HasOverrides(ext);
        setText(custom ? string.Format(AppStrings.Format_Extension_Customized, ext) : ext);
        ToolTipService.SetToolTip(item, custom ? string.Format(AppStrings.Format_Extension_Customized_Tooltip, ext) : null);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetItemStatus(item, custom ? AppStrings.Format_Customized : string.Empty);
    }

    private void OnScopeChanged(SelectorBar sender, SelectorBarSelectionChangedEventArgs args) =>
        ChangeScope(sender.SelectedItem, sender.SelectedItem?.Tag as string);

    private void OnScopeComboChanged(object sender, SelectionChangedEventArgs e) =>
        ChangeScope(ScopeCombo.SelectedItem, (ScopeCombo.SelectedItem as ComboBoxItem)?.Tag as string);

    private void ChangeScope(object? selected, string? extension)
    {
        if (ViewModel.Format is not { } format || selected is null) return;
        var scope = extension is not null ? SettingsScope.ForExtension(format, extension) : SettingsScope.ForFormat(format);
        if (scope != ViewModel.Scope) ViewModel.SetScope(scope);
        UpdatePreview();
    }

    private void UpdatePreview() => Preview.Update(ViewModel.Preview, ViewModel.ScopeLabel, ViewModel.SampleFileName);

    private void OnEngineStatusChanged(object? sender, EventArgs e) => UpdateEngineStatus();

    /// <summary>Up to this many extensions the status lists one row per extension; above, one row per provider.</summary>
    private const int MaxHandlerRows = 8;

    private void UpdateEngineStatus()
    {
        HandlersList.Items.Clear();
        if (!AppServices.IsEngineStatusKnown)
        {
            HandlersList.Items.Add(new TextBlock { Text = AppStrings.Common_Checking, Style = Res<Style>("GlancyCaptionStyle") });
            return;
        }

        var engine = AppServices.Engine;
        var extensions = ViewModel.Format?.Extensions ?? Catalog.AllExtensions;
        var handlers = extensions.Select(engine.HandlerFor).ToList();
        if (handlers.Count <= MaxHandlerRows)
        {
            foreach (var handler in handlers) HandlersList.Items.Add(HandlerRow(handler.Extension, handler, null));
            return;
        }

        // Many extensions (all formats, RAW, Blender…): one row per provider, Glancy first, then by size.
        foreach (var group in handlers.GroupBy(h => h.Describe())
                     .OrderBy(g => g.First().Owner switch { HandlerOwner.Glancy => 0, HandlerOwner.Other => 1, _ => 2 })
                     .ThenByDescending(g => g.Count()))
        {
            var exts = group.Select(h => h.Extension).ToList();
            HandlersList.Items.Add(HandlerRow(exts.Count == 1 ? AppStrings.Count_Extension_One : string.Format(AppStrings.Count_Extension_Many, exts.Count), group.First(), exts,
                ViewModel.IsAllFormats ? 24 : int.MaxValue));
        }
    }

    /// <summary>One status row: an extension (or, grouped, an extension count plus the list) and its provider.</summary>
    private static Grid HandlerRow(string label, ThumbnailHandlerInfo handler, IReadOnlyCollection<string>? extensions, int maxListed = int.MaxValue)
    {
        var grouped = extensions is not null;
        var mono = new Microsoft.UI.Xaml.Media.FontFamily("Cascadia Mono, Consolas");
        var row = new Grid { ColumnSpacing = 12, Margin = new Thickness(0, grouped ? 4 : 0, 0, grouped ? 4 : 0) };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(grouped ? 104 : 72) });
        row.ColumnDefinitions.Add(new ColumnDefinition());
        var labelBlock = new TextBlock { Text = label };
        if (!grouped) labelBlock.FontFamily = mono;
        row.Children.Add(labelBlock);

        var details = new StackPanel { Spacing = 2 };
        var status = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
        status.Children.Add(new FontIcon
        {
            Glyph = handler.Owner switch { HandlerOwner.Glancy => "\uE73E", HandlerOwner.Other => "\uE8AB", _ => "\uE711" },
            FontSize = 12,
            Style = Res<Style>(handler.Owner == HandlerOwner.Glancy ? "GlancyIconSuccessStyle" : "GlancyIconSecondaryStyle"),
        });
        status.Children.Add(new TextBlock { Text = handler.Describe(), TextWrapping = TextWrapping.Wrap });
        details.Children.Add(status);
        if (extensions is not null) details.Children.Add(ExtensionList.Create(extensions, maxListed));
        Grid.SetColumn(details, 1);
        row.Children.Add(details);
        return row;
    }

    private static T Res<T>(string key) => (T)Application.Current.Resources[key];

    private void UpdateCustomIcon()
    {
        var location = ViewModel.CustomIconPath.TextValue;
        CustomIconImage.Source = null;
        if (string.IsNullOrWhiteSpace(location)) return;
        var (path, index) = IconExtractor.ParseLocation(location);
        var icon = IconExtractor.Extract(path, index, 24);
        if (icon is not null) CustomIconImage.Source = CheckerBitmap.FromPixels(icon.Width, icon.Height, icon.PremultipliedBgra);
    }

    private void OnLayoutSizeChanged(object sender, SizeChangedEventArgs e)
    {
        var inline = e.NewSize.Width < InlinePreviewBelow;
        if (inline && ReferenceEquals(PreviewHost.Child, Preview))
        {
            PreviewHost.Child = null;
            PreviewHost.Visibility = Visibility.Collapsed;
            InlinePreviewExpander.Content = Preview;
            InlinePreviewExpander.Visibility = Visibility.Visible;
            Preview.IsCompact = true;
        }
        else if (!inline && ReferenceEquals(InlinePreviewExpander.Content, Preview))
        {
            InlinePreviewExpander.Content = null;
            InlinePreviewExpander.Visibility = Visibility.Collapsed;
            PreviewHost.Child = Preview;
            PreviewHost.Visibility = Visibility.Visible;
            Preview.IsCompact = false;
        }
        ContentPanel.Padding = (Thickness)Application.Current.Resources[e.NewSize.Width < 640 ? "GlancyPagePaddingCompact" : "GlancyPagePadding"];
    }

    /// <summary>"Resampling, gamma correction and alpha channel." — a list in the UI language (casing per language).</summary>
    private static string SentenceList(IReadOnlyList<string> titles)
    {
        var lower = AppStrings.List_LowercaseFollowingItems == "true";
        var parts = titles.Select((t, i) => i == 0 || !lower ? t : char.ToLowerInvariant(t[0]) + t[1..]).ToList();
        var text = parts.Count > 1
            ? string.Join(AppStrings.List_Separator, parts.Take(parts.Count - 1)) + AppStrings.List_LastSeparator + parts[^1]
            : string.Concat(parts);
        return text + AppStrings.List_End;
    }

    private void OnAdvancedToggle(object sender, RoutedEventArgs e)
    {
        var open = AdvancedToggle.IsChecked == true;
        AdvancedPanel.Visibility = open ? Visibility.Visible : Visibility.Collapsed;
        AdvancedChevron.Glyph = open ? "" : "";
    }

    private void OnCanvasColorChosen(object? sender, uint color) => ViewModel.CanvasColor.ColorValue = color;

    private void OnCheckerChosen(object? sender, CheckerColors value) => ViewModel.Checker.CheckerValue = value;

    private async void OnChooseIconClick(object sender, RoutedEventArgs e)
    {
        var chosen = await IconPickerDialog.PickAsync(ViewModel.CustomIconPath.TextValue);
        if (chosen is not null) ViewModel.CustomIconPath.TextValue = chosen;
    }

    // ---- "More" menu

    private void OnMoreMenuOpening(object sender, object e)
    {
        PasteItem.IsEnabled = AppServices.Clipboard is not null;
        PasteItem.Text = AppServices.Clipboard is { } clip ? string.Format(AppStrings.Format_PasteFrom, clip.SourceLabel) : AppStrings.Format_PasteSettings;
        RemoveOverridesItem.Visibility = ViewModel.IsExtensionScope ? Visibility.Visible : Visibility.Collapsed;
        RemoveOverridesItem.IsEnabled = ViewModel.HasExtensionOverrides;
        RemoveOverridesItem.Text = ViewModel.IsExtensionScope ? string.Format(AppStrings.Format_RemoveCustomizationOf, ViewModel.Scope.Extension) : string.Empty;
        ResetItem.Text = ViewModel.IsAllFormats ? AppStrings.Format_ResetAll : string.Format(AppStrings.Format_ResetScope, ViewModel.ScopeLabel);
    }

    private void OnCopyClick(object sender, RoutedEventArgs e) => CopySettings();

    private void OnPasteClick(object sender, RoutedEventArgs e) => PasteSettings();

    private void OnCopyAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        CopySettings();
    }

    private void OnPasteAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        PasteSettings();
    }

    private void CopySettings()
    {
        AppServices.Clipboard = ViewModel.Copy();
        App.Current.Window?.Notify(InfoBarSeverity.Success, AppStrings.Format_Copied_Title,
            AppStrings.Format_Copied_Message);
    }

    private void PasteSettings()
    {
        if (AppServices.Clipboard is not { } clip) return;
        var count = ViewModel.Paste(clip);
        App.Current.Window?.Notify(InfoBarSeverity.Success, AppStrings.Format_Pasted_Title,
            string.Format(AppStrings.Format_Pasted_Message, count, clip.SourceLabel, ViewModel.ScopeLabel));
    }

    private void OnRemoveOverridesClick(object sender, RoutedEventArgs e) => ViewModel.ResetScope();

    private async void OnResetScopeClick(object sender, RoutedEventArgs e)
    {
        if (await WindowHelper.ConfirmAsync(
                string.Format(AppStrings.Format_Reset_Confirm, ViewModel.ScopeLabel),
                AppStrings.Format_Reset_Confirm_Message,
                AppStrings.Format_Reset))
        {
            ViewModel.ResetScope();
        }
    }
}
