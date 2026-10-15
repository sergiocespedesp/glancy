using Glancy.Core.Settings;
using Glancy.Settings.Helpers;
using Glancy.Settings.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Controls;

/// <summary>
/// Picks a checkerboard: a gallery of the saved library plus editing of the two colors, swap, and saving the
/// current pair to the library, all in one flyout.
/// </summary>
public sealed partial class CheckerButton : Button
{
    private readonly Image _swatch = new() { Width = 20, Height = 20, Stretch = Stretch.None };
    private readonly TextBlock _label = new() { VerticalAlignment = VerticalAlignment.Center };
    private readonly Flyout _flyout = new();
    private readonly GridView _library = new() { SelectionMode = ListViewSelectionMode.None, IsItemClickEnabled = true, MaxHeight = 180 };
    private readonly ColorButton _light = new() { HorizontalAlignment = HorizontalAlignment.Stretch };
    private readonly ColorButton _dark = new() { HorizontalAlignment = HorizontalAlignment.Stretch };

    public CheckerButton()
    {
        var content = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10 };
        content.Children.Add(new Border { CornerRadius = new CornerRadius(4), Child = _swatch });
        content.Children.Add(_label);
        content.Children.Add(new FontIcon { Glyph = "", FontSize = 10, Margin = new Thickness(4, 0, 0, 0) });
        Content = content;
        MinWidth = 160;
        HorizontalAlignment = HorizontalAlignment.Right;

        var panel = new StackPanel { Spacing = 10, Width = 300 };
        panel.Children.Add(new TextBlock { Text = AppStrings.Checker_Library, Style = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"] });
        panel.Children.Add(_library);
        _library.ItemClick += (_, e) => Commit((CheckerColors)((FrameworkElement)e.ClickedItem).Tag);

        panel.Children.Add(new TextBlock { Text = AppStrings.Checker_Customize, Style = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"], Margin = new Thickness(0, 6, 0, 0) });
        var colors = new Grid { ColumnSpacing = 8 };
        colors.ColumnDefinitions.Add(new ColumnDefinition());
        colors.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        colors.ColumnDefinitions.Add(new ColumnDefinition());
        AutomationProperties.SetName(_light, AppStrings.Checker_LightColor);
        AutomationProperties.SetName(_dark, AppStrings.Checker_DarkColor);
        _light.MinWidth = _dark.MinWidth = 0;
        _light.ColorChosen += (_, c) => Commit(new CheckerColors(c, Value.Dark), keepOpen: true);
        _dark.ColorChosen += (_, c) => Commit(new CheckerColors(Value.Light, c), keepOpen: true);
        var swap = new Button { Content = new FontIcon { Glyph = "", FontSize = 14 }, Padding = new Thickness(8) };
        ToolTipService.SetToolTip(swap, AppStrings.Checker_Swap);
        AutomationProperties.SetName(swap, AppStrings.Checker_Swap);
        swap.Click += (_, _) => Commit(Value.Swapped(), keepOpen: true);
        Grid.SetColumn(swap, 1);
        Grid.SetColumn(_dark, 2);
        colors.Children.Add(_light);
        colors.Children.Add(swap);
        colors.Children.Add(_dark);
        panel.Children.Add(colors);

        var actions = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, HorizontalAlignment = HorizontalAlignment.Right };
        var random = new Button { Content = AppStrings.Checker_Random };
        random.Click += (_, _) => Commit(RandomPair(), keepOpen: true);
        var save = new Button { Content = AppStrings.Checker_SaveToLibrary };
        save.Click += (_, _) => SaveToLibrary();
        actions.Children.Add(random);
        actions.Children.Add(save);
        panel.Children.Add(actions);

        _flyout.Content = panel;
        _flyout.Opening += (_, _) => { FillLibrary(); _light.Color = Value.Light; _dark.Color = Value.Dark; };
        Flyout = _flyout;
        UpdateVisual();
    }

    public static readonly DependencyProperty ValueProperty =
        DependencyProperty.Register(nameof(Value), typeof(CheckerColors), typeof(CheckerButton),
            new PropertyMetadata(new CheckerColors(0xFFFFFFFF, 0xFFCCCCCC), (d, _) => ((CheckerButton)d).UpdateVisual()));

    public CheckerColors Value { get => (CheckerColors)GetValue(ValueProperty); set => SetValue(ValueProperty, value); }

    public event EventHandler<CheckerColors>? ValueChosen;

    public static IEnumerable<CheckerColors> Library =>
        (AppServices.Session.Working.Global[Catalog.CheckerLibrary.Key] as string[] ?? [])
            .Select(s => CheckerColors.TryParse(s, out var c) ? c : (CheckerColors?)null)
            .Where(c => c.HasValue).Select(c => c!.Value);

    private void Commit(CheckerColors value, bool keepOpen = false)
    {
        if (!keepOpen) _flyout.Hide();
        Value = value;
        _light.Color = value.Light;
        _dark.Color = value.Dark;
        ValueChosen?.Invoke(this, value);
    }

    private void SaveToLibrary()
    {
        var list = (AppServices.Session.Working.Global[Catalog.CheckerLibrary.Key] as string[] ?? []).ToList();
        var text = Value.ToString();
        if (!list.Contains(text, StringComparer.OrdinalIgnoreCase)) list.Add(text);
        AppServices.Session.WriteGlobal(Catalog.CheckerLibrary, list.ToArray());
        FillLibrary();
    }

    private void FillLibrary()
    {
        _library.Items.Clear();
        foreach (var pair in Library)
        {
            var item = new Border
            {
                Tag = pair,
                Style = (Style)Application.Current.Resources[pair == Value ? "GlancySwatchSelectedStyle" : "GlancySwatchStyle"],
                Child = new Image { Source = CheckerBitmap.Create(pair, 40, 40, 8), Width = 40, Height = 40 },
            };
            AutomationProperties.SetName(item, Views.ExplorerPage.CheckerName(pair.ToString(), pair));
            if (pair == Value) AutomationProperties.SetItemStatus(item, AppStrings.Common_Selected);
            _library.Items.Add(item);
        }
    }

    private void UpdateVisual()
    {
        _swatch.Source = CheckerBitmap.Create(Value, 20, 20, 5);
        _label.Text = $"{ColorHelper.ToHex(Value.Light)} / {ColorHelper.ToHex(Value.Dark)}";
    }

    private static CheckerColors RandomPair()
    {
        var hue = Random.Shared.NextDouble() * 360;
        return new CheckerColors(Hsl(hue, 0.35, 0.85), Hsl(hue, 0.30, 0.65));
    }

    private static uint Hsl(double h, double s, double l)
    {
        double c = (1 - Math.Abs(2 * l - 1)) * s, x = c * (1 - Math.Abs(h / 60 % 2 - 1)), m = l - c / 2;
        var (r, g, b) = h switch
        {
            < 60 => (c, x, 0d), < 120 => (x, c, 0d), < 180 => (0d, c, x),
            < 240 => (0d, x, c), < 300 => (x, 0d, c), _ => (c, 0d, x),
        };
        return 0xFF000000 | (uint)((r + m) * 255) << 16 | (uint)((g + m) * 255) << 8 | (uint)((b + m) * 255);
    }
}
