using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace Glancy.Settings.Helpers;

public static class ColorHelper
{
    public static Color FromArgb(uint argb) =>
        Color.FromArgb((byte)(argb >> 24), (byte)(argb >> 16), (byte)(argb >> 8), (byte)argb);

    public static uint ToArgb(Color c) => (uint)(c.A << 24 | c.R << 16 | c.G << 8 | c.B);

    public static SolidColorBrush Brush(uint argb) => new(FromArgb(argb));

    public static string ToHex(uint argb) => "#" + (argb & 0xFFFFFF).ToString("X6");

    /// <summary>Chooses black or white text for legibility on the given background (WCAG relative luminance).</summary>
    public static Color ContrastingText(uint argb)
    {
        static double Channel(byte v)
        {
            var c = v / 255.0;
            return c <= 0.03928 ? c / 12.92 : Math.Pow((c + 0.055) / 1.055, 2.4);
        }
        var col = FromArgb(argb);
        var l = 0.2126 * Channel(col.R) + 0.7152 * Channel(col.G) + 0.0722 * Channel(col.B);
        return l > 0.4 ? Color.FromArgb(255, 0x1A, 0x1A, 0x1A) : Color.FromArgb(255, 255, 255, 255);
    }

    /// <summary>A muted variant of a brand color, for badges whose text must meet 4.5:1 contrast.</summary>
    public static uint Darken(uint argb, double factor)
    {
        var c = FromArgb(argb);
        return ToArgb(Color.FromArgb(c.A, (byte)(c.R * factor), (byte)(c.G * factor), (byte)(c.B * factor)));
    }
}
