using Glancy.Core.Settings;
using Glancy.Core.Shell;
using Glancy.Settings.Controls;
using Glancy.Settings.Services;
using Glancy.Settings.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Microsoft.UI.Xaml.Shapes;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Views;

public sealed partial class HomePage : Page
{
    private const double StackedHeroBelow = 640;

    public HomePage()
    {
        InitializeComponent();
        SizeChanged += (_, e) => UpdateHeroLayout(e.NewSize.Width);
        var formats = Catalog.Formats.Count;
        HeroSubtitle.Text = string.Format(AppStrings.Home_Subtitle, formats);
        FormatsCaption.Text = string.Format(AppStrings.Home_FormatsCaption, formats, Catalog.AllExtensions.Count);
    }

    public GlobalSettingsViewModel Global { get; } = new();

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        AppServices.Session.Changed += OnSessionChanged;
        AppServices.EngineStatusChanged += OnSessionChanged;
        Refresh();
        base.OnNavigatedTo(e);
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        AppServices.Session.Changed -= OnSessionChanged;
        AppServices.EngineStatusChanged -= OnSessionChanged;
        Global.Dispose();
        base.OnNavigatedFrom(e);
    }

    private void OnSessionChanged(object? sender, EventArgs e) => Refresh();

    private void Refresh()
    {
        var engine = AppServices.Engine;
        var known = AppServices.IsEngineStatusKnown;
        var enabled = Global.Enabled.IsOn;
        EngineInfoBar.IsOpen = known && !engine.IsInstalled;

        // Neutral icon unless something is actually wrong: the accent color (which may be red) is not used here.
        (StatusIcon.Glyph, StatusIcon.Style, StatusText.Text) = (known, engine.IsInstalled, enabled) switch
        {
            (false, _, _) => ("", Res<Style>("GlancyIconSecondaryStyle"), AppStrings.Home_Status_Checking),
            (_, true, true) => ("", Res<Style>("GlancyIconSuccessStyle"),
                string.Format(AppStrings.Home_ActiveIn, engine.ActiveExtensionCount, engine.Handlers.Count)),
            (_, true, false) => ("", Res<Style>("GlancyIconCautionStyle"), AppStrings.Home_Status_Paused),
            (_, false, true) => ("", Res<Style>("GlancyIconSecondaryStyle"), AppStrings.Home_Status_EngineMissing),
            _ => ("", Res<Style>("GlancyIconCautionStyle"), AppStrings.Home_Status_EngineMissingPaused),
        };

        BuildCategories();
    }

    private void UpdateHeroLayout(double width)
    {
        // Narrow windows: the master switch moves under the text instead of squeezing it.
        var stacked = width < StackedHeroBelow;
        Grid.SetRow(MasterSwitchPanel, stacked ? 1 : 0);
        Grid.SetColumn(MasterSwitchPanel, stacked ? 1 : 2);
    }

    /// <summary>
    /// One group per category: a level-3 heading with counts, then a grid of compact tiles. Pending changes are
    /// computed once per refresh (not per tile): with 87 formats that keeps a refresh to a single pass.
    /// </summary>
    private void BuildCategories()
    {
        var session = AppServices.Session;
        var pendingFormats = session.GetChanges()
            .Select(c => c.ScopeLabel.Split(" › ")[0])
            .ToHashSet(StringComparer.Ordinal);

        CategoriesPanel.Children.Clear();
        foreach (var category in Catalog.Categories)
        {
            var formats = Catalog.FormatsIn(category).ToList();
            var extensions = formats.Sum(f => f.Extensions.Count);

            var header = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Margin = new Thickness(0, 20, 0, 8) };
            var title = new TextBlock { Text = category.Name, Style = Res<Style>("BodyStrongTextBlockStyle") };
            AutomationProperties.SetHeadingLevel(title, Microsoft.UI.Xaml.Automation.Peers.AutomationHeadingLevel.Level3);
            header.Children.Add(title);
            header.Children.Add(new TextBlock
            {
                Text = $"{Plural(formats.Count, AppStrings.Count_Format_One, AppStrings.Count_Format_Many)} · {Plural(extensions, AppStrings.Count_Extension_One, AppStrings.Count_Extension_Many)}",
                Style = Res<Style>("GlancyCaptionStyle"),
                VerticalAlignment = VerticalAlignment.Center,
            });
            CategoriesPanel.Children.Add(header);

            var repeater = new ItemsRepeater
            {
                Layout = new UniformGridLayout
                {
                    MinItemWidth = 232,
                    MinItemHeight = 64,
                    MinRowSpacing = 8,
                    MinColumnSpacing = 8,
                    ItemsStretch = UniformGridLayoutItemsStretch.Fill,
                },
                ItemsSource = formats.Select(f => BuildFormatTile(f, pendingFormats.Contains(f.DisplayName))).ToList(),
            };
            CategoriesPanel.Children.Add(repeater);
        }
    }

    private Button BuildFormatTile(FormatDefinition format, bool pending)
    {
        var session = AppServices.Session;
        var scope = SettingsScope.ForFormat(format);
        var mode = session.Read(scope, Catalog.Mode).AsInt();
        var configured = mode switch
        {
            Values.Generation.Enabled => $"Glancy · {Catalog.Transparency.FormatValue(session.Read(scope, Catalog.Transparency).Value)}",
            Values.Generation.Fallback => Catalog.Mode.FindOption(Values.Generation.Fallback)!.Label,
            _ => Catalog.Mode.FindOption(Values.Generation.Disabled)!.Label,
        };
        var custom = format.Extensions.Where(session.HasOverrides).ToList();

        // What Explorer really uses today. The tile never claims "Glancy" unless Glancy is the registered provider.
        var engine = AppServices.Engine;
        var handlers = format.Extensions.Select(engine.HandlerFor).ToList();
        var glancyActive = engine.IsInstalled && handlers.All(h => h.Owner == HandlerOwner.Glancy);
        var explorerLine = !AppServices.IsEngineStatusKnown
            ? AppStrings.Home_Tile_Checking
            : glancyActive
                ? AppStrings.Home_Tile_UsesGlancy
                : handlers.FirstOrDefault(h => h.Owner == HandlerOwner.Other) is { } other
                    ? string.Format(AppStrings.Home_Tile_UsesOther, other.Describe())
                    : AppStrings.Home_Tile_NoProvider;
        var live = glancyActive && mode == Values.Generation.Enabled && Global.Enabled.IsOn;

        var grid = new Grid { ColumnSpacing = 12 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition());
        grid.Children.Add(new FormatBadge { FormatId = format.Id, Size = 32, VerticalAlignment = VerticalAlignment.Center });

        var text = new StackPanel { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        text.Children.Add(new TextBlock { Text = format.DisplayName, Style = Res<Style>("BodyStrongTextBlockStyle"), TextTrimming = TextTrimming.CharacterEllipsis, TextWrapping = TextWrapping.NoWrap });
        // Dot · value (trimmed when narrow) · "Not applied" pill (always whole).
        var status = new Grid { ColumnSpacing = 6 };
        status.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        status.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        status.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        status.Children.Add(new Ellipse { Style = Res<Style>(live ? "GlancyDotSuccessStyle" : "GlancyDotNeutralStyle") });
        var value = new TextBlock { Text = configured, Style = Res<Style>("GlancyCaptionStyle"), TextTrimming = TextTrimming.CharacterEllipsis, TextWrapping = TextWrapping.NoWrap, VerticalAlignment = VerticalAlignment.Center };
        Grid.SetColumn(value, 1);
        status.Children.Add(value);
        if (pending)
        {
            var pill = new Border
            {
                Style = Res<Style>("GlancyPillStyle"),
                Padding = new Thickness(6, 0, 6, 1),
                Child = new TextBlock { Text = AppStrings.Home_Tile_NotApplied, Style = Res<Style>("CaptionTextBlockStyle") },
            };
            Grid.SetColumn(pill, 2);
            status.Children.Add(pill);
        }
        text.Children.Add(status);
        Grid.SetColumn(text, 1);
        grid.Children.Add(text);

        var customLine = custom.Count > 0 ? string.Format(AppStrings.Home_Tile_Customized, string.Join(", ", custom)) : string.Empty;
        var button = new Button { Style = Res<Style>("GlancyTileButtonStyle"), Padding = new Thickness(12, 10, 12, 10), Content = grid, Tag = format.Id };
        ToolTipService.SetToolTip(button, string.Join(Environment.NewLine,
            new[] { string.Join("  ", format.Extensions), explorerLine, customLine }.Where(s => s.Length > 0)));
        AutomationProperties.SetName(button,
            string.Format(AppStrings.Home_Tile_Name, format.DisplayName, configured, explorerLine) + (customLine.Length > 0 ? AppStrings.Sentence_Separator + customLine : string.Empty) + (pending ? AppStrings.Sentence_Separator + AppStrings.Home_Tile_Pending : string.Empty));
        button.Click += (_, _) => App.Current.Window?.NavigateTo(format.Id);
        return button;
    }

    private static string Plural(int n, string one, string many) => n == 1 ? one : string.Format(many, n);

    private async void OnRefreshExplorer(object sender, RoutedEventArgs e)
    {
        RefreshButton.IsEnabled = false;
        try { await App.Current.Window!.RefreshExplorerAsync(Catalog.AllExtensions, announce: true); }
        finally { RefreshButton.IsEnabled = true; }
    }

    private void OnOpenMaintenance(object sender, RoutedEventArgs e) => App.Current.Window?.NavigateTo("maintenance");

    private async void OnRecheckEngine(object sender, RoutedEventArgs e) => await AppServices.RefreshEngineStatusAsync();

    private static T Res<T>(string key) => (T)Application.Current.Resources[key];
}
