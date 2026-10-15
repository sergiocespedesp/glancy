using Glancy.Core.Settings;
using Glancy.Settings.Helpers;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Windows.Foundation;

namespace Glancy.Settings.Controls;

/// <summary>Everything the preview needs to imitate an Explorer thumbnail.</summary>
public sealed record PreviewState(
    int Transparency,
    uint CanvasColor,
    CheckerColors Checker,
    int CheckerCell,
    int Scale,
    int FullFill,
    int Adornment,
    bool ShowBadge,
    bool CustomIcon,
    string Monogram,
    uint BadgeColor);

/// <summary>
/// Draws a sample image with transparent and semi-transparent areas the way Explorer would show its thumbnail
/// at a given size, applying the transparency, scaling, square-canvas, frame and overlay-icon settings.
/// It is a simulation: no file is decoded, so it never blocks the UI.
/// </summary>
public sealed partial class ThumbnailTile : UserControl
{
    private const double ArtworkWidth = 150, ArtworkHeight = 100;
    private readonly Grid _box = new();

    public ThumbnailTile()
    {
        IsTabStop = false;
        Content = _box;
    }

    /// <summary>The thumbnail size Explorer requests (e.g. 96 or 256).</summary>
    public double TileSize { get; set; } = 256;

    public void Render(PreviewState s)
    {
        _box.Children.Clear();
        _box.Width = _box.Height = TileSize;

        var shadow = s.Adornment == Values.Adornment.DropShadow;
        var available = TileSize - (shadow ? Math.Max(3, TileSize / 48) : 0);
        var fit = Math.Min(available / ArtworkWidth, available / ArtworkHeight);
        var scale = s.Scale == Values.Scale.ActualSize ? Math.Min(fit, 1.0) : fit;
        double w = Math.Round(ArtworkWidth * scale), h = Math.Round(ArtworkHeight * scale);

        var art = BuildArtwork(w, h, s);
        FrameworkElement thumb = art;
        if (s.FullFill != Values.FullFill.None)
        {
            var square = new Grid { Width = available, Height = available };
            square.Children.Add(FillLayer(s.FullFill switch
            {
                Values.FullFill.Checkerboard => Values.Transparency.Checkerboard,
                Values.FullFill.Canvas => Values.Transparency.Canvas,
                _ => Values.Transparency.Transparent,
            }, available, available, s));
            art.HorizontalAlignment = HorizontalAlignment.Center;
            art.VerticalAlignment = VerticalAlignment.Center;
            square.Children.Add(art);
            thumb = square;
        }

        var holder = new Grid { HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center };
        if (shadow)
        {
            // Explorer's frame: a soft offset shadow plus a hairline border.
            var offset = Math.Max(2, TileSize / 96);
            holder.Children.Add(new Rectangle
            {
                Fill = new SolidColorBrush(ColorHelper.FromArgb(0x38000000)),
                Margin = new Thickness(offset, offset, -offset, -offset),
                RadiusX = 1, RadiusY = 1,
            });
            holder.Children.Add(thumb);
            holder.Children.Add(new Rectangle
            {
                Stroke = new SolidColorBrush(ColorHelper.FromArgb(0x30000000)),
                StrokeThickness = 1,
            });
        }
        else
        {
            holder.Children.Add(thumb);
        }

        if (s.ShowBadge) holder.Children.Add(BuildBadge(s, thumb));
        _box.Children.Add(holder);
    }

    private static FrameworkElement BuildArtwork(double w, double h, PreviewState s)
    {
        var grid = new Grid { Width = w, Height = h };
        grid.Children.Add(FillLayer(s.Transparency, w, h, s));

        var canvas = new Canvas { Width = ArtworkWidth, Height = ArtworkHeight };
        var glow = new Ellipse { Width = 90, Height = 90 };
        var glowBrush = new RadialGradientBrush();
        glowBrush.GradientStops.Add(new GradientStop { Color = ColorHelper.FromArgb(0xCC8A3FFC), Offset = 0 });
        glowBrush.GradientStops.Add(new GradientStop { Color = ColorHelper.FromArgb(0x008A3FFC), Offset = 1 });
        glow.Fill = glowBrush;
        Canvas.SetLeft(glow, -8); Canvas.SetTop(glow, -14);
        canvas.Children.Add(glow);

        var sun = new Ellipse { Width = 34, Height = 34, Fill = ColorHelper.Brush(0xFFFFB020) };
        Canvas.SetLeft(sun, 98); Canvas.SetTop(sun, 10);
        canvas.Children.Add(sun);
        canvas.Children.Add(Polygon(0xF24C5CFF, (0, 100), (56, 34), (108, 100)));
        canvas.Children.Add(Polygon(0xB818B4A0, (62, 100), (112, 52), (150, 84), (150, 100)));

        grid.Children.Add(new Viewbox { Child = canvas, Stretch = Stretch.Fill });
        return grid;
    }

    private static FrameworkElement FillLayer(int transparency, double w, double h, PreviewState s) => transparency switch
    {
        Values.Transparency.Checkerboard => new Image
        {
            Source = CheckerBitmap.Create(s.Checker, (int)Math.Ceiling(w), (int)Math.Ceiling(h), s.CheckerCell),
            Width = w, Height = h, Stretch = Stretch.None,
        },
        Values.Transparency.Canvas => new Rectangle { Width = w, Height = h, Fill = ColorHelper.Brush(s.CanvasColor) },
        // Opaque discards alpha; transparent pixels typically carry white in design files.
        Values.Transparency.Opaque => new Rectangle { Width = w, Height = h, Fill = new SolidColorBrush(Microsoft.UI.Colors.White) },
        _ => new Rectangle { Width = w, Height = h },
    };

    private Border BuildBadge(PreviewState s, FrameworkElement thumb)
    {
        var size = Math.Clamp(Math.Round(TileSize * 0.22), 16, 48);
        var color = s.CustomIcon ? 0xFF5B5FC7 : ColorHelper.Darken(s.BadgeColor, 0.78);
        return new Border
        {
            Width = size,
            Height = size,
            CornerRadius = new CornerRadius(size * 0.22),
            Background = ColorHelper.Brush(color),
            BorderBrush = new SolidColorBrush(ColorHelper.FromArgb(0x66FFFFFF)),
            BorderThickness = new Thickness(1),
            HorizontalAlignment = HorizontalAlignment.Right,
            VerticalAlignment = VerticalAlignment.Bottom,
            Margin = new Thickness(0, 0, -size * 0.18, -size * 0.18),
            Child = s.CustomIcon
                ? new FontIcon { Glyph = "", FontSize = size * 0.5, Foreground = new SolidColorBrush(Microsoft.UI.Colors.White) }
                : new TextBlock
                {
                    Text = s.Monogram,
                    FontSize = size * FormatBadge.MonogramScale(s.Monogram, 0.42),
                    FontWeight = Microsoft.UI.Text.FontWeights.Bold,
                    Foreground = new SolidColorBrush(ColorHelper.ContrastingText(color)),
                    HorizontalAlignment = HorizontalAlignment.Center,
                    VerticalAlignment = VerticalAlignment.Center,
                },
        };
    }

    private static Polygon Polygon(uint color, params (double X, double Y)[] points)
    {
        var p = new Polygon { Fill = ColorHelper.Brush(color) };
        foreach (var (x, y) in points) p.Points.Add(new Point(x, y));
        return p;
    }
}
