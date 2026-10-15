using Glancy.Settings.Helpers;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Controls;

/// <summary>A button showing a color swatch and its hex value; opens a flyout with quick swatches and a full picker.</summary>
public sealed partial class ColorButton : Button
{
    private static readonly uint[] QuickColors = [0xFFFFFFFF, 0xFFF3F3F3, 0xFFCCCCCC, 0xFF808080, 0xFF2B2B2B, 0xFF000000, 0xFFFFF4E0, 0xFFE8EEFF];

    private readonly Border _swatch = new()
    {
        Width = 20, Height = 20, CornerRadius = new CornerRadius(4), BorderThickness = new Thickness(1),
    };
    private readonly TextBlock _label = new() { VerticalAlignment = VerticalAlignment.Center, MinWidth = 64 };
    private readonly ColorPicker _picker = new()
    {
        IsAlphaEnabled = false, IsMoreButtonVisible = true, IsColorChannelTextInputVisible = true, IsHexInputVisible = true,
        ColorSpectrumShape = ColorSpectrumShape.Box,
    };
    private readonly Flyout _flyout = new();

    public ColorButton()
    {
        _swatch.Style = (Style)Application.Current.Resources["GlancySwatchStyle"];
        _swatch.CornerRadius = new CornerRadius(4);
        var content = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 10 };
        content.Children.Add(_swatch);
        content.Children.Add(_label);
        content.Children.Add(new FontIcon { Glyph = "", FontSize = 10, Margin = new Thickness(4, 0, 0, 0) });
        Content = content;
        MinWidth = 160;
        HorizontalAlignment = HorizontalAlignment.Right;

        var panel = new StackPanel { Spacing = 12, Width = 300 };
        var quick = new GridView { SelectionMode = ListViewSelectionMode.None, IsItemClickEnabled = true };
        foreach (var c in QuickColors)
        {
            quick.Items.Add(new Border
            {
                Width = 26, Height = 26, Tag = c,
                Style = (Style)Application.Current.Resources["GlancySwatchStyle"],
                Background = ColorHelper.Brush(c),
            });
            AutomationProperties.SetName((Border)quick.Items[^1], ColorHelper.ToHex(c));
        }
        quick.ItemClick += (_, e) => Commit((uint)((Border)e.ClickedItem).Tag);
        panel.Children.Add(new TextBlock { Text = AppStrings.Color_Quick, Style = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"] });
        panel.Children.Add(quick);
        panel.Children.Add(_picker);
        var accept = new Button { Content = AppStrings.Color_Use, Style = (Style)Application.Current.Resources["AccentButtonStyle"], HorizontalAlignment = HorizontalAlignment.Right };
        accept.Click += (_, _) => Commit(ColorHelper.ToArgb(_picker.Color) | 0xFF000000);
        panel.Children.Add(accept);
        _flyout.Content = panel;
        Flyout = _flyout;
        _flyout.Opening += (_, _) => _picker.Color = ColorHelper.FromArgb(Color);
        UpdateVisual();
    }

    public static readonly DependencyProperty ColorProperty =
        DependencyProperty.Register(nameof(Color), typeof(uint), typeof(ColorButton), new PropertyMetadata(0xFFFFFFFFu, (d, _) => ((ColorButton)d).UpdateVisual()));

    public uint Color { get => (uint)GetValue(ColorProperty); set => SetValue(ColorProperty, value); }

    public event EventHandler<uint>? ColorChosen;

    private void Commit(uint color)
    {
        _flyout.Hide();
        Color = color;
        ColorChosen?.Invoke(this, color);
    }

    private void UpdateVisual()
    {
        _swatch.Background = ColorHelper.Brush(Color);
        _label.Text = ColorHelper.ToHex(Color);
        AutomationProperties.SetItemStatus(this, ColorHelper.ToHex(Color));
    }
}
