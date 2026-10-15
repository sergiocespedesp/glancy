using System.Runtime.InteropServices.WindowsRuntime;
using Glancy.Core.Settings;
using Glancy.Core.Shell;
using Glancy.Core.Storage;
using Glancy.Settings.Helpers;
using Glancy.Settings.Services;
using Glancy.Settings.Views;
using Microsoft.UI;
using Microsoft.UI.Composition;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Hosting;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.UI.Xaml.Navigation;
using Windows.Graphics;
using Windows.Graphics.Imaging;
using Windows.Storage.Streams;
using Windows.System;
using Glancy.Settings.Resources;

namespace Glancy.Settings;

/// <summary>A search result in the title-bar search box.</summary>
public sealed record SearchEntry(string Title, string Subtitle, string Glyph, string Target, string Haystack)
{
    /// <summary>Normalized terms that make this entry an exact hit (its title and its file extensions).</summary>
    public IReadOnlyList<string> ExactKeys { get; init; } = [];

    public override string ToString() => Title;
}

public sealed partial class MainWindow : Window
{
    private static readonly Dictionary<string, Type> Pages = new()
    {
        ["home"] = typeof(HomePage),
        ["explorer"] = typeof(ExplorerPage),
        ["maintenance"] = typeof(MaintenancePage),
        ["preferences"] = typeof(PreferencesPage),
        ["about"] = typeof(AboutPage),
    };

    private readonly LaunchOptions _options;
    private readonly List<SearchEntry> _searchIndex;
    private string? _pendingFocusKey;
    private bool _closeConfirmed;
    private bool _applying;
    private Task<bool>? _applyTask;
    private uint _captureBackground = 0xFFF3F3F3;

    public MainWindow(LaunchOptions options)
    {
        _options = options;
        InitializeComponent();

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.SetIcon(Path.Combine(AppContext.BaseDirectory, "Assets", "Glancy.ico"));
        AppWindow.Title = "Glancy";
        if (AppWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.PreferredMinimumWidth = 500;
            presenter.PreferredMinimumHeight = 480;
        }
        RestoreBounds();

        AddFormatNavigationItems();
        _searchIndex = BuildSearchIndex();
        AddCommaAccelerator();
        SetupPendingBarAnimations();

        AppServices.Session.Changed += (_, _) => UpdatePendingBar();
        RootGrid.SizeChanged += (_, e) =>
        {
            // Narrow windows: shorter search hint and a one-line pending bar instead of clipped text.
            var narrow = e.NewSize.Width < 760;
            SearchBox.PlaceholderText = narrow ? AppStrings.Search_PlaceholderShort : AppStrings.Search_Placeholder;
            PendingCaption.Visibility = narrow ? Visibility.Collapsed : Visibility.Visible;
        };
        AppWindow.Closing += OnClosing;

        var theme = options.Theme?.ToLowerInvariant() switch
        {
            "light" => AppTheme.Light,
            "dark" => AppTheme.Dark,
            _ => AppServices.Preferences.Theme,
        };
        ApplyTheme(theme);
        if (!options.IsCapture) SetAlwaysOnTop(AppServices.Preferences.AlwaysOnTop);

        NavigateTo(options.Page ?? "home");
        UpdatePendingBar();

        if (options.IsCapture)
        {
            RootGrid.Loaded += async (_, _) => await RunCaptureAsync();
        }
    }

    // ======================================================================== navigation

    /// <summary>Navigates to a tag: "home", "all", "psd", "psd|.psb", "explorer", ... with an optional "|focus=key".</summary>
    public void NavigateTo(string target)
    {
        var parts = target.Split('|');
        var focus = parts.FirstOrDefault(p => p.StartsWith("focus=", StringComparison.Ordinal));
        _pendingFocusKey = focus?["focus=".Length..];
        var pageParameter = string.Join('|', parts.Where(p => p != focus));
        var key = parts[0];

        if (Pages.TryGetValue(key, out var pageType))
        {
            if (ContentFrame.CurrentSourcePageType != pageType) ContentFrame.Navigate(pageType, pageParameter);
            else FocusPendingSetting();
        }
        else if (key == "all" || Catalog.FindFormat(key) is not null)
        {
            ContentFrame.Navigate(typeof(FormatPage), pageParameter);
        }
        else if (ContentFrame.Content is null)
        {
            ContentFrame.Navigate(typeof(HomePage), "home");
        }
    }

    public void BringToFront(string? page)
    {
        if (page is not null) NavigateTo(page);
        AppWindow.Show();
        WindowHelper.ForceForeground(WindowHelper.Hwnd);
    }

    /// <summary>Segoe Fluent Icons glyph per format category (one glyph per concept, see docs/design-system.md).</summary>
    private static readonly Dictionary<string, string> CategoryGlyphs = new()
    {
        ["design"] = "\uE790", ["vector"] = "\uE70F", ["photo"] = "\uE722", ["image"] = "\uEB9F",
        ["hdr"] = "\uE706", ["texture"] = "\uE7FC", ["3d"] = "\uF158", ["document"] = "\uE8A5",
        ["text"] = "\uE8D2", ["retro"] = "\uE7F4", ["other"] = "\uE8F1",
    };

    /// <summary>Format id → its navigation item (nested inside a category item).</summary>
    private readonly Dictionary<string, NavigationViewItem> _formatNavItems = new(StringComparer.OrdinalIgnoreCase);

    private const string CategoryTagPrefix = "category:";

    /// <summary>
    /// Formats are grouped by category as expandable items. A category item has no page of its own: invoking it
    /// toggles its formats (SelectsOnInvoked = false). That keeps one destination per format and avoids a second,
    /// overlapping "list of formats" page next to Home (which already groups every format by category).
    /// </summary>
    private void AddFormatNavigationItems()
    {
        var insertAt = NavView.MenuItems.IndexOf(FormatsEndSeparator);
        var quick = Catalog.QuickFormats.Select(f => f.Id).ToList();
        foreach (var category in Catalog.Categories)
        {
            var formats = Catalog.FormatsIn(category).ToList();
            var parent = new NavigationViewItem
            {
                Content = category.Name,
                Tag = CategoryTagPrefix + category.Id,
                SelectsOnInvoked = false,
                Icon = new FontIcon { Glyph = CategoryGlyphs.GetValueOrDefault(category.Id, "\uE8F1") },
            };
            ToolTipService.SetToolTip(parent, formats.Count == 1 ? string.Format(AppStrings.Nav_Category_Tooltip_One, category.Name) : string.Format(AppStrings.Nav_Category_Tooltip_Many, category.Name, formats.Count));
            foreach (var format in formats)
            {
                var item = new NavigationViewItem { Content = format.DisplayName, Tag = format.Id };
                var shortcut = quick.IndexOf(format.Id);
                ToolTipService.SetToolTip(item, shortcut >= 0
                    ? $"{format.DisplayName} (Ctrl+{shortcut + 1})"
                    : $"{format.DisplayName} · {ExtensionSummary(format, 6)}");
                parent.MenuItems.Add(item);
                _formatNavItems[format.Id] = item;
            }
            NavView.MenuItems.Insert(insertAt++, parent);
        }
    }

    /// <summary>".cr2 .nef .arw … (35)": the first extensions of a format, for tooltips and search subtitles.</summary>
    internal static string ExtensionSummary(FormatDefinition format, int max) =>
        format.Extensions.Count <= max
            ? string.Join(" ", format.Extensions)
            : $"{string.Join(" ", format.Extensions.Take(max))} … ({format.Extensions.Count})";

    private NavigationViewItem? FindNavItem(string? tag)
    {
        if (tag is null) return null;
        if (_formatNavItems.TryGetValue(tag, out var formatItem)) return formatItem;
        return NavView.MenuItems.Concat(NavView.FooterMenuItems).OfType<NavigationViewItem>().FirstOrDefault(i => (string?)i.Tag == tag);
    }

    private NavigationViewItem? ParentCategoryItem(NavigationViewItem item) =>
        NavView.MenuItems.OfType<NavigationViewItem>().FirstOrDefault(p => p.MenuItems.Contains(item));

    private void OnNavSelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItemContainer?.Tag is not string tag || tag.StartsWith(CategoryTagPrefix, StringComparison.Ordinal)) return;
        if (CurrentTag() == tag) return;
        NavigateTo(tag);
    }

    private string? _currentTag;

    private string? CurrentTag() => _currentTag;

    private void OnFrameNavigated(object sender, NavigationEventArgs e)
    {
        // The page's own OnNavigatedTo runs after this event, so derive the tag from the navigation itself.
        _currentTag = e.SourcePageType == typeof(FormatPage)
            ? (e.Parameter as string ?? "all").Split('|')[0]
            : Pages.FirstOrDefault(p => p.Value == e.SourcePageType).Key;
        var item = FindNavItem(_currentTag);
        if (item is not null && !ReferenceEquals(NavView.SelectedItem, item))
        {
            // A format lives inside its (possibly collapsed) category: expand that one and fold the others, so the
            // selection is visible without scrolling through 87 formats (accordion, like Windows Settings).
            if (ParentCategoryItem(item) is { } parent)
            {
                foreach (var other in NavView.MenuItems.OfType<NavigationViewItem>().Where(i => i.MenuItems.Count > 0 && !ReferenceEquals(i, parent)))
                    other.IsExpanded = false;
                parent.IsExpanded = true;
            }
            NavView.SelectedItem = item;
        }
        AppTitleBar.IsBackButtonEnabled = ContentFrame.CanGoBack;
        FocusPendingSetting();
    }

    private void FocusPendingSetting()
    {
        if (_pendingFocusKey is not { } key) return;
        _pendingFocusKey = null;
        DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, () =>
        {
            if (ContentFrame.Content is DependencyObject page) VisualTree.FocusSetting(page, key);
        });
    }

    private void OnTitleBarBackRequested(TitleBar sender, object args) => GoBack();

    private void OnTitleBarPaneToggleRequested(TitleBar sender, object args) => NavView.IsPaneOpen = !NavView.IsPaneOpen;

    private void OnNavDisplayModeChanged(NavigationView sender, NavigationViewDisplayModeChangedEventArgs args) =>
        AppTitleBar.IsPaneToggleButtonVisible = true;

    private void GoBack()
    {
        if (ContentFrame.CanGoBack) ContentFrame.GoBack();
    }

    // ======================================================================== pending changes

    private void UpdatePendingBar()
    {
        var changes = AppServices.Session.GetChanges();
        var show = changes.Count > 0;
        PendingBadge.Value = changes.Count;
        UndoButton.IsEnabled = AppServices.Session.CanUndo && !_applying;
        PendingTitle.Text = changes.Count == 1 ? AppStrings.Pending_Count_One : string.Format(AppStrings.Pending_Count_Many, changes.Count);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(PendingBar, PendingTitle.Text);
        if (show && PendingBar.Visibility == Visibility.Collapsed)
        {
            PendingBar.Visibility = Visibility.Visible;
            MaybeShowPendingTip();
        }
        else if (!show && PendingBar.Visibility == Visibility.Visible)
        {
            PendingBar.Visibility = Visibility.Collapsed;
        }
    }

    private void OnReviewOpening(object? sender, object e)
    {
        ChangesList.Items.Clear();
        foreach (var group in AppServices.Session.GetChanges().GroupBy(c => c.ScopeLabel))
        {
            ChangesList.Items.Add(new TextBlock
            {
                Text = group.Key,
                Style = (Style)Application.Current.Resources["GlancyOverlineStyle"],
                Margin = new Thickness(0, 8, 0, 2),
            });
            foreach (var change in group)
            {
                var row = new StackPanel { Spacing = 0, Margin = new Thickness(0, 2, 0, 2) };
                row.Children.Add(new TextBlock { Text = change.Setting.Title, Style = (Style)Application.Current.Resources["BodyTextBlockStyle"] });
                row.Children.Add(new TextBlock
                {
                    Text = $"{change.Setting.FormatValue(change.From)}  →  {change.Setting.FormatValue(change.To)}",
                    Style = (Style)Application.Current.Resources["GlancyCaptionStyle"],
                });
                ChangesList.Items.Add(row);
            }
        }
    }

    private async void OnApplyClick(object sender, RoutedEventArgs e) => await ApplyAsync();

    private async void OnDiscardClick(object sender, RoutedEventArgs e) => await DiscardAsync();

    private async void OnApplyAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        await ApplyAsync();
    }

    private void OnUndoAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        // Inside a text field Ctrl+Z belongs to the field.
        if (Microsoft.UI.Xaml.Input.FocusManager.GetFocusedElement(RootGrid.XamlRoot) is TextBox or AutoSuggestBox or PasswordBox)
            return;
        args.Handled = true;
        Undo();
    }

    private void OnUndoClick(object sender, RoutedEventArgs e) => Undo();

    private void Undo()
    {
        if (_applying || !AppServices.Session.Undo()) return;
        Microsoft.UI.Xaml.Automation.Peers.FrameworkElementAutomationPeer.FromElement(PendingBar)?
            .RaiseNotificationEvent(Microsoft.UI.Xaml.Automation.Peers.AutomationNotificationKind.ActionCompleted,
                Microsoft.UI.Xaml.Automation.Peers.AutomationNotificationProcessing.MostRecent, AppStrings.Undo_Announcement, "glancy-undo");
    }

    private void OnHelpAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        NavigateTo("about");
    }

    /// <summary>Writes pending changes, then refreshes open Explorer windows if enabled. Returns false on failure.</summary>
    public async Task<bool> ApplyAsync()
    {
        // An apply may still be refreshing Explorer: wait for it, then apply whatever was edited meanwhile.
        while (_applyTask is { IsCompleted: false } running)
        {
            if (!await running) return false;
        }
        var task = ApplyCoreAsync();
        _applyTask = task;
        return await task;
    }

    private async Task<bool> ApplyCoreAsync()
    {
        var session = AppServices.Session;
        if (!session.IsDirty) return true;
        _applying = true;
        SetApplying(true);
        try
        {
            var affected = session.GetAffectedExtensions();
            var count = session.GetChanges().Count;
            session.Apply();
            Log.Info($"Applied {count} changes (settings revision updated).");

            var refresh = session.ReadGlobal(Catalog.RefreshOnApply).AsBool();
            if (!AppServices.Engine.IsInstalled)
            {
                Notify(InfoBarSeverity.Success, AppStrings.Apply_Saved_Title,
                    AppStrings.Apply_Saved_EngineMissing);
            }
            else if (refresh && affected.Count > 0)
            {
                if (await RefreshExplorerAsync(affected, announce: false))
                    Notify(InfoBarSeverity.Success, AppStrings.Apply_Applied_Title, AppStrings.Apply_Applied_Refreshed);
            }
            else
            {
                Notify(InfoBarSeverity.Success, AppStrings.Apply_Applied_Title,
                    AppStrings.Apply_Applied_NoRefresh);
            }
            return true;
        }
        catch (SettingsStoreException ex)
        {
            Log.Error("Apply failed", ex);
            Notify(InfoBarSeverity.Error, AppStrings.Apply_Failed_Title, string.Format(AppStrings.Apply_Failed_Message, ex.Message));
            return false;
        }
        finally
        {
            _applying = false;
            SetApplying(false);
            UpdatePendingBar();
        }
    }

    /// <summary>Refreshes open Explorer windows. Returns false (after telling the user) if it failed or timed out.</summary>
    public async Task<bool> RefreshExplorerAsync(IReadOnlyCollection<string> extensions, bool announce)
    {
        try
        {
            var recursive = AppServices.Session.ReadGlobal(Catalog.FolderRecursion).AsBool();
            // A hung thumbnail provider must not block applying forever.
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(90));
            // WaitAsync abandons a refresh stuck inside a thumbnail provider; the worker is a background thread.
            var result = await ExplorerRefresher.RefreshAsync(extensions, recursive, forceExtraction: AppServices.Engine.IsInstalled, timeout.Token)
                .WaitAsync(timeout.Token);
            if (announce)
            {
                var limit = result.LimitReached ? AppStrings.Sentence_Separator + string.Format(AppStrings.Refresh_LimitReached, ExplorerRefresher.MaxFiles) : string.Empty;
                Notify(InfoBarSeverity.Success, AppStrings.Refresh_Done_Title,
                    string.Format(AppStrings.Refresh_Done_Message, result.FileCount, result.FolderCount) + limit);
            }
            return true;
        }
        catch (OperationCanceledException)
        {
            Notify(InfoBarSeverity.Warning, AppStrings.Refresh_Stopped_Title,
                AppStrings.Refresh_Stopped_Message);
            return false;
        }
        catch (Exception ex)
        {
            // Refreshing is best effort (COM, file system, shell automation): it must never fail the apply.
            Log.Error("File Explorer refresh failed", ex);
            Notify(InfoBarSeverity.Warning, AppStrings.Refresh_Partial_Title,
                AppStrings.Refresh_Partial_Message);
            return false;
        }
    }

    private async Task<bool> DiscardAsync()
    {
        var count = AppServices.Session.GetChanges().Count;
        if (count == 0) return true;
        var confirmed = await WindowHelper.ConfirmAsync(
            count == 1 ? AppStrings.Discard_Confirm_One : string.Format(AppStrings.Discard_Confirm_Many, count),
            AppStrings.Discard_Confirm_Message,
            AppStrings.Pending_Discard, destructive: true);
        if (confirmed) AppServices.Session.Discard();
        return confirmed;
    }

    private void SetApplying(bool busy)
    {
        ApplyProgress.IsActive = busy;
        ApplyProgress.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
        ApplyText.Text = busy ? AppStrings.Pending_Applying : AppStrings.Pending_Apply;
        ApplyButton.IsEnabled = DiscardButton.IsEnabled = ReviewButton.IsEnabled = !busy;
        UndoButton.IsEnabled = !busy && AppServices.Session.CanUndo;
    }

    private void SetupPendingBarAnimations()
    {
        if (!new Windows.UI.ViewManagement.UISettings().AnimationsEnabled) return;
        ElementCompositionPreview.SetIsTranslationEnabled(PendingBar, true);
        var compositor = ElementCompositionPreview.GetElementVisual(PendingBar).Compositor;
        var duration = TimeSpan.FromMilliseconds((double)Application.Current.Resources["GlancyMotionNormal"]);

        CompositionAnimationGroup Build(float fromOpacity, float toOpacity, float fromY, float toY)
        {
            var easing = compositor.CreateCubicBezierEasingFunction(new(0.1f, 0.9f), new(0.2f, 1f));
            var opacity = compositor.CreateScalarKeyFrameAnimation();
            opacity.Target = "Opacity";
            opacity.InsertKeyFrame(0, fromOpacity);
            opacity.InsertKeyFrame(1, toOpacity, easing);
            opacity.Duration = duration;
            var move = compositor.CreateVector3KeyFrameAnimation();
            move.Target = "Translation";
            move.InsertKeyFrame(0, new(0, fromY, 0));
            move.InsertKeyFrame(1, new(0, toY, 0), easing);
            move.Duration = duration;
            var group = compositor.CreateAnimationGroup();
            group.Add(opacity);
            group.Add(move);
            return group;
        }

        ElementCompositionPreview.SetImplicitShowAnimation(PendingBar, Build(0, 1, 24, 0));
        ElementCompositionPreview.SetImplicitHideAnimation(PendingBar, Build(1, 0, 0, 24));
    }

    private void MaybeShowPendingTip()
    {
        var prefs = AppServices.Preferences;
        if (_options.IsCapture || !prefs.ShowTips || prefs.FirstRunCompleted) return;
        DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, () => PendingTip.IsOpen = true);
    }

    private void OnPendingTipClosed(TeachingTip sender, TeachingTipClosedEventArgs args) =>
        AppServices.Preferences.FirstRunCompleted = true;

    // ======================================================================== notifications

    public void Notify(InfoBarSeverity severity, string title, string message)
    {
        var bar = new InfoBar
        {
            Severity = severity,
            Title = title,
            Message = message,
            IsOpen = true,
            IsClosable = true,
            Style = (Style)Application.Current.Resources["GlancyToastStyle"],
        };
        bar.Closed += (s, _) => NotificationHost.Children.Remove(s);
        NotificationHost.Children.Add(bar);
        while (NotificationHost.Children.Count > 3) NotificationHost.Children.RemoveAt(0);

        // Errors stay until dismissed; everything else fades after a reading-time delay.
        if (severity is InfoBarSeverity.Success or InfoBarSeverity.Informational)
        {
            var timer = DispatcherQueue.CreateTimer();
            timer.Interval = TimeSpan.FromSeconds(Math.Clamp(3 + message.Length / 20.0, 5, 12));
            timer.IsRepeating = false;
            timer.Tick += (_, _) => bar.IsOpen = false;
            timer.Start();
        }
    }

    // ======================================================================== search

    private List<SearchEntry> BuildSearchIndex()
    {
        var list = new List<SearchEntry>();
        void Add(string title, string subtitle, string glyph, string target, string extra = "") =>
            list.Add(new SearchEntry(title, subtitle, glyph, target, TextSearch.Normalize($"{title} {subtitle} {extra}")));

        Add(AppStrings.Nav_Home, AppStrings.Search_Home_Sub, "", "home");
        Add(AppStrings.Nav_AllFormats, AppStrings.Search_AllFormats_Sub, "", "all");
        foreach (var f in Catalog.Formats)
        {
            var category = Catalog.FindCategory(f.Category)?.Name ?? AppStrings.Search_Format;
            // The haystack holds every extension, so ".nef" or "blend12" find their format.
            list.Add(new SearchEntry(f.DisplayName, $"{category} · {ExtensionSummary(f, 5)}", "", f.Id,
                TextSearch.Normalize($"{f.DisplayName} {category} {f.Monogram} {string.Join(" ", f.Extensions)} {f.Description}"))
            {
                ExactKeys = [TextSearch.Normalize(f.DisplayName), TextSearch.Normalize(f.Monogram), .. f.Extensions],
            });
            // Formats with several extensions also get one entry per extension, which opens that extension's scope.
            if (f.Extensions.Count > 1)
                foreach (var ext in f.Extensions)
                    list.Add(new SearchEntry(ext, string.Format(AppStrings.Search_ExtensionOf, f.DisplayName), "", $"{f.Id}|{ext}", TextSearch.Normalize($"{ext} {f.DisplayName}"))
                    {
                        ExactKeys = [ext],
                    });
        }
        foreach (var s in Catalog.FormatSettings)
            Add(s.Title, string.Format(AppStrings.Search_FormatSetting, s.Group), s.Glyph, $"all|focus={s.Key}", s.Description);
        foreach (var s in Catalog.GlobalSettings)
        {
            var page = s == Catalog.DebugLog ? "maintenance" : "explorer";
            Add(s.Title, $"{(page == "explorer" ? AppStrings.Nav_Explorer : AppStrings.Nav_Maintenance)} · {s.Group}", s.Glyph, $"{page}|focus={s.Key}", s.Description);
        }
        Add(AppStrings.Nav_Explorer, AppStrings.Search_Explorer_Sub, "", "explorer");
        Add(AppStrings.Nav_Maintenance, AppStrings.Search_Maintenance_Sub, "", "maintenance");
        Add(AppStrings.Maintenance_ClearThumbs, AppStrings.Nav_Maintenance, "", "maintenance", AppStrings.Search_ClearCache_Keywords);
        Add(AppStrings.Maintenance_Export, AppStrings.Search_Backup_Sub, "", "maintenance", AppStrings.Search_Export_Keywords);
        Add(AppStrings.Maintenance_Import, AppStrings.Search_Backup_Sub, "", "maintenance", AppStrings.Search_Import_Keywords);
        Add(AppStrings.Maintenance_Reset, AppStrings.Nav_Maintenance, "", "maintenance", AppStrings.Search_Reset_Keywords);
        Add(AppStrings.Search_Theme, AppStrings.Nav_Preferences, "", "preferences", AppStrings.Search_Theme_Keywords);
        Add(AppStrings.Search_TopMost, AppStrings.Search_TopMost_Sub, "", "preferences", AppStrings.Search_TopMost_Keywords);
        Add(AppStrings.Preferences_Language, AppStrings.Nav_Preferences, "", "preferences", AppStrings.Search_Language_Keywords);
        Add(AppStrings.About_Shortcuts, AppStrings.Nav_About, "", "about", AppStrings.Search_Shortcuts_Keywords);
        return list;
    }

    private void OnSearchTextChanged(AutoSuggestBox sender, AutoSuggestBoxTextChangedEventArgs args)
    {
        if (args.Reason != AutoSuggestionBoxTextChangeReason.UserInput) return;
        var query = sender.Text.Trim();
        if (query.Length == 0)
        {
            sender.ItemsSource = null;
            return;
        }
        var results = Search(query).Take(8).ToList();
        sender.ItemsSource = results.Count > 0
            ? results
            : new List<SearchEntry> { new(AppStrings.Search_NoResults, AppStrings.Search_NoResults_Hint, "", string.Empty, string.Empty) };
    }

    private void OnSearchQuerySubmitted(AutoSuggestBox sender, AutoSuggestBoxQuerySubmittedEventArgs args)
    {
        var entry = args.ChosenSuggestion as SearchEntry ?? Search(args.QueryText).FirstOrDefault();
        if (entry is null || entry.Target.Length == 0) return;
        sender.Text = string.Empty;
        sender.ItemsSource = null;
        NavigateTo(entry.Target);
    }

    /// <summary>
    /// Matching entries, best first: an exact extension or name (".nef", "png"), then titles that start with the
    /// query, then everything else in index order. With 265 extensions, ranking keeps the obvious hit on top.
    /// </summary>
    private IEnumerable<SearchEntry> Search(string query)
    {
        var q = TextSearch.Normalize(query.Trim());
        if (q.Length == 0) return [];
        var dotted = q.StartsWith('.') ? q : "." + q;
        return _searchIndex
            .Where(e => TextSearch.Matches(e.Haystack, q))
            .Select((e, index) => (Entry: e, Index: index, Rank:
                e.ExactKeys.Contains(q) || e.ExactKeys.Contains(dotted) ? 0
                : TextSearch.Normalize(e.Title).StartsWith(q, StringComparison.Ordinal) ? 1
                : 2))
            .OrderBy(x => x.Rank).ThenBy(x => x.Index)
            .Select(x => x.Entry);
    }

    private void OnSearchAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        SearchBox.Focus(FocusState.Keyboard);
    }

    // ======================================================================== shortcuts

    private void AddCommaAccelerator()
    {
        var comma = new KeyboardAccelerator { Key = (VirtualKey)188, Modifiers = VirtualKeyModifiers.Control };
        comma.Invoked += (_, a) => { a.Handled = true; NavigateTo("preferences"); };
        RootGrid.KeyboardAccelerators.Add(comma);
        RootGrid.KeyboardAcceleratorPlacementMode = KeyboardAcceleratorPlacementMode.Hidden;
    }

    private void OnNavigateAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        var target = sender.Key switch
        {
            VirtualKey.E => "explorer",
            VirtualKey.Number0 => "all",
            >= VirtualKey.Number1 and <= VirtualKey.Number5 => Catalog.QuickFormats[sender.Key - VirtualKey.Number1].Id,
            _ => null,
        };
        if (target is not null) NavigateTo(target);
    }

    private void OnBackAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        GoBack();
    }

    private void OnTopMostAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        SetAlwaysOnTop(!AppServices.Preferences.AlwaysOnTop);
    }

    private void OnPinClick(object sender, RoutedEventArgs e) => SetAlwaysOnTop(PinButton.IsChecked == true);

    // ======================================================================== window preferences

    public void SetTheme(AppTheme theme)
    {
        AppServices.Preferences.Theme = theme;
        ApplyTheme(theme);
    }

    private void ApplyTheme(AppTheme theme)
    {
        RootGrid.RequestedTheme = theme switch
        {
            AppTheme.Light => ElementTheme.Light,
            AppTheme.Dark => ElementTheme.Dark,
            _ => ElementTheme.Default,
        };
        AppWindow.TitleBar.PreferredTheme = theme switch
        {
            AppTheme.Light => TitleBarTheme.Light,
            AppTheme.Dark => TitleBarTheme.Dark,
            _ => TitleBarTheme.UseDefaultAppMode,
        };

        if (_options.IsCapture)
        {
            // RenderTargetBitmap cannot see the Mica backdrop; paint Mica's base tint for the requested theme.
            var dark = theme == AppTheme.Dark
                       || (theme == AppTheme.System && Application.Current.RequestedTheme == ApplicationTheme.Dark);
            _captureBackground = dark ? 0xFF202020u : 0xFFF3F3F3u;
            RootGrid.Background = Helpers.ColorHelper.Brush(_captureBackground);
        }
    }

    public void SetAlwaysOnTop(bool value)
    {
        AppServices.Preferences.AlwaysOnTop = value;
        if (AppWindow.Presenter is OverlappedPresenter p) p.IsAlwaysOnTop = value;
        PinButton.IsChecked = value;
        ToolTipService.SetToolTip(PinButton, value ? AppStrings.TopMost_Off_Tooltip : AppStrings.TopMost_Tooltip);
    }

    private void RestoreBounds()
    {
        var scale = GetScale();
        if (_options.IsCapture)
        {
            AppWindow.Resize(new SizeInt32((int)(_options.Width * scale), (int)(_options.Height * scale)));
            return;
        }

        var saved = AppServices.Preferences.WindowBounds;
        var work = DisplayArea.GetFromWindowId(AppWindow.Id, DisplayAreaFallback.Primary).WorkArea;
        if (saved is { } b && b[2] >= 500 && b[3] >= 480
            && b[0] < work.X + work.Width - 100 && b[1] < work.Y + work.Height - 100 && b[0] + b[2] > work.X + 100 && b[1] >= work.Y - 10)
        {
            AppWindow.MoveAndResize(new RectInt32(b[0], b[1], b[2], b[3]));
        }
        else
        {
            var w = (int)Math.Min(1240 * scale, work.Width * 0.9);
            var h = (int)Math.Min(840 * scale, work.Height * 0.9);
            AppWindow.MoveAndResize(new RectInt32(work.X + (work.Width - w) / 2, work.Y + (work.Height - h) / 2, w, h));
        }
    }

    private double GetScale()
    {
        var dpi = GetDpiForWindow(WinRT.Interop.WindowNative.GetWindowHandle(this));
        return dpi > 0 ? dpi / 96.0 : 1.0;
    }

    [System.Runtime.InteropServices.LibraryImport("user32.dll")]
    private static partial uint GetDpiForWindow(IntPtr hwnd);

    private async void OnClosing(AppWindow sender, AppWindowClosingEventArgs args)
    {
        if (!_options.IsCapture && AppWindow.Presenter is OverlappedPresenter { State: OverlappedPresenterState.Restored })
        {
            AppServices.Preferences.WindowBounds = [AppWindow.Position.X, AppWindow.Position.Y, AppWindow.Size.Width, AppWindow.Size.Height];
        }

        if (_applyTask is { IsCompleted: false } running && !_options.IsCapture)
        {
            args.Cancel = true;
            await running;
            Close();
            return;
        }

        if (_closeConfirmed || _options.IsCapture || !AppServices.Session.IsDirty)
        {
            if (_restartRequested) RestartNow();
            return;
        }

        args.Cancel = true;
        if (WindowHelper.IsDialogOpen)
        {
            // Another dialog (e.g. a confirmation) is open; WinUI cannot stack a second one.
            _restartRequested = false;
            Notify(InfoBarSeverity.Informational, AppStrings.Close_DialogOpen_Title, AppStrings.Close_DialogOpen_Message);
            return;
        }
        var count = AppServices.Session.GetChanges().Count;
        var dialog = new ContentDialog
        {
            Title = count == 1 ? AppStrings.Close_Pending_One : string.Format(AppStrings.Close_Pending_Many, count),
            Content = new TextBlock { Text = AppStrings.Close_Pending_Message, TextWrapping = TextWrapping.Wrap },
            PrimaryButtonText = AppStrings.Close_ApplyAndClose,
            SecondaryButtonText = AppStrings.Close_WithoutApplying,
            CloseButtonText = AppStrings.Common_Cancel,
            DefaultButton = ContentDialogButton.Primary,
        };
        var result = await WindowHelper.ShowDialogAsync(dialog);
        if (result == ContentDialogResult.None || (result == ContentDialogResult.Primary && !await ApplyAsync()))
        {
            _restartRequested = false;   // staying open also cancels a pending language restart
            return;
        }

        _closeConfirmed = true;
        Close();
    }

    private bool _restartRequested;

    /// <summary>
    /// Restarts Glancy (to apply a new UI language). Closing runs first, so pending changes get the usual
    /// apply / close without applying / cancel choice; cancelling also cancels the restart.
    /// </summary>
    public void Restart()
    {
        _restartRequested = true;
        Close();
    }

    private void RestartNow()
    {
        var failure = Microsoft.Windows.AppLifecycle.AppInstance.Restart(string.Empty);
        // Restart only returns when it failed; the window then closes normally and the user reopens Glancy.
        Log.Error($"Restart failed: {failure}", null);
    }

    // ======================================================================== QA capture mode

    private async Task RunCaptureAsync()
    {
        try
        {
            await Task.Delay(1500);
            if (_options.CapturePath is { } single)
            {
                await CaptureToFileAsync(single);
            }
            else if (_options.CaptureSetDirectory is { } dir)
            {
                Directory.CreateDirectory(dir);
                string[] pages = ["home", "all", "psd", "psd|.psb", "eps", "svg", "raw", "blender", "jpeg", "explorer", "maintenance", "preferences", "about"];
                foreach (var theme in new[] { AppTheme.Light, AppTheme.Dark })
                {
                    ApplyTheme(theme);
                    foreach (var page in pages)
                    {
                        NavigateTo(page);
                        await Task.Delay(900);
                        var name = $"{theme.ToString().ToLowerInvariant()}-{page.Replace('|', '_').Replace(".", "")}";
                        await CaptureToFileAsync(Path.Combine(dir, name + ".png"));
                        // Full-height render of the scrolling content, to review everything below the fold.
                        if (ContentFrame.Content is DependencyObject root
                            && VisualTree.Descendants<ScrollViewer>(root).FirstOrDefault()?.Content is UIElement full)
                        {
                            await CaptureToFileAsync(Path.Combine(dir, name + "-full.png"), full);
                        }
                    }
                }
                // Narrow layout: preview moves inline, navigation collapses.
                var scale = GetScale();
                AppWindow.Resize(new SizeInt32((int)(600 * scale), (int)(900 * scale)));
                ApplyTheme(AppTheme.Light);
                foreach (var page in new[] { "home", "psd", "raw" })
                {
                    NavigateTo(page);
                    await Task.Delay(1200);
                    await CaptureToFileAsync(Path.Combine(dir, $"narrow-{page}.png"));
                }
            }
        }
        catch (Exception ex)
        {
            Log.Error("Capture mode failed", ex);
            Environment.ExitCode = 1;
        }
        finally
        {
            Application.Current.Exit();
        }
    }

    private async Task CaptureToFileAsync(string path, UIElement? element = null)
    {
        var rtb = new RenderTargetBitmap();
        await rtb.RenderAsync(element ?? RootGrid);
        var pixels = (await rtb.GetPixelsAsync()).ToArray();
        if (element is not null)
        {
            // Partial renders have no window background: composite them over the theme surface.
            byte bb = (byte)_captureBackground, bg = (byte)(_captureBackground >> 8), br = (byte)(_captureBackground >> 16);
            for (var i = 0; i < pixels.Length; i += 4)
            {
                var inv = 255 - pixels[i + 3];
                pixels[i] = (byte)(pixels[i] + bb * inv / 255);
                pixels[i + 1] = (byte)(pixels[i + 1] + bg * inv / 255);
                pixels[i + 2] = (byte)(pixels[i + 2] + br * inv / 255);
                pixels[i + 3] = 255;
            }
        }
        using var stream = new InMemoryRandomAccessStream();
        var encoder = await BitmapEncoder.CreateAsync(BitmapEncoder.PngEncoderId, stream);
        encoder.SetPixelData(BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied,
            (uint)rtb.PixelWidth, (uint)rtb.PixelHeight, 96, 96, pixels);
        await encoder.FlushAsync();
        stream.Seek(0);
        var bytes = new byte[stream.Size];
        await stream.ReadAsync(bytes.AsBuffer(), (uint)stream.Size, InputStreamOptions.None);
        await File.WriteAllBytesAsync(path, bytes);
    }
}
