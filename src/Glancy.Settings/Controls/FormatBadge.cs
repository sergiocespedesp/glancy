using Glancy.Core.Settings;
using Glancy.Settings.Helpers;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Glancy.Settings.Controls;

/// <summary>Rounded monogram tile identifying a format ("Ps", "Ai"...). Decorative: hidden from screen readers.</summary>
public sealed partial class FormatBadge : UserControl
{
    private readonly Border _border = new();
    private readonly FontIcon _allIcon = new() { Glyph = "" };
    private readonly TextBlock _text = new()
    {
        HorizontalAlignment = HorizontalAlignment.Center,
        VerticalAlignment = VerticalAlignment.Center,
        FontWeight = Microsoft.UI.Text.FontWeights.Bold,
    };

    public FormatBadge()
    {
        IsTabStop = false;
        _border.Child = _text;
        Content = _border;
        AutomationProperties.SetAccessibilityView(this, AccessibilityView.Raw);
        Update();
    }

    public static readonly DependencyProperty FormatIdProperty =
        DependencyProperty.Register(nameof(FormatId), typeof(string), typeof(FormatBadge), new PropertyMetadata("", (d, _) => ((FormatBadge)d).Update()));

    public static readonly DependencyProperty SizeProperty =
        DependencyProperty.Register(nameof(Size), typeof(double), typeof(FormatBadge), new PropertyMetadata(32.0, (d, _) => ((FormatBadge)d).Update()));

    /// <summary>Format id, or "all" for the all-formats tile.</summary>
    public string FormatId { get => (string)GetValue(FormatIdProperty); set => SetValue(FormatIdProperty, value); }

    public double Size { get => (double)GetValue(SizeProperty); set => SetValue(SizeProperty, value); }

    /// <summary>Font size factor for a monogram: "Ps" fills the tile, "HEIC" must still fit inside it.</summary>
    public static double MonogramScale(string monogram, double twoLetters) => monogram.Length switch
    {
        <= 2 => twoLetters,
        3 => twoLetters * 0.75,
        _ => twoLetters * 0.6,
    };

    private void Update()
    {
        var format = Catalog.FindFormat(FormatId ?? string.Empty);
        var color = format is null ? 0xFF5B5FC7u : ColorHelper.Darken(format.BadgeColor, 0.78);
        _border.Width = _border.Height = Size;
        _border.CornerRadius = new CornerRadius(Size * 0.24);
        _border.Background = ColorHelper.Brush(color);
        _text.Text = format?.Monogram ?? string.Empty;
        _text.FontSize = Size * MonogramScale(_text.Text, 0.4);
        _text.Foreground = new SolidColorBrush(ColorHelper.ContrastingText(color));
        _allIcon.FontSize = Size * 0.42;
        _allIcon.Foreground = _text.Foreground;
        _border.Child = format is null ? _allIcon : _text;
    }
}
