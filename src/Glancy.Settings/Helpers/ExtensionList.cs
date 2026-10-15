using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Glancy.Settings.Helpers;

/// <summary>
/// A wrapping grid of file extensions in monospace (".blend .blend1 … .blend32"). A plain wrapping TextBlock may
/// break inside an extension (".blend" / "6"); fixed-width cells keep every extension whole and aligned in columns.
/// </summary>
public static class ExtensionList
{
    /// <summary>Approximate advance of one Cascadia Mono / Consolas character at 12 px.</summary>
    private const double CharWidth = 7.3;

    /// <param name="max">Show at most this many, then a "+N" cell (the all-formats page lists hundreds).</param>
    public static VariableSizedWrapGrid Create(IReadOnlyCollection<string> extensions, int max = int.MaxValue)
    {
        var longest = extensions.Count == 0 ? 4 : extensions.Take(max).Max(e => e.Length);
        var grid = new VariableSizedWrapGrid
        {
            Orientation = Orientation.Horizontal,
            ItemWidth = Math.Ceiling(longest * CharWidth + 14),
            ItemHeight = 18,
            HorizontalAlignment = HorizontalAlignment.Left,
        };
        var mono = new FontFamily("Cascadia Mono, Consolas");
        var style = (Style)Application.Current.Resources["GlancyCaptionStyle"];
        foreach (var ext in extensions.Take(max))
            grid.Children.Add(new TextBlock { Text = ext, Style = style, FontFamily = mono, TextWrapping = TextWrapping.NoWrap });
        if (extensions.Count > max)
            grid.Children.Add(new TextBlock { Text = $"+{extensions.Count - max}", Style = style, TextWrapping = TextWrapping.NoWrap });
        return grid;
    }
}
