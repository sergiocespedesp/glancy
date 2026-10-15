using System.Runtime.InteropServices.WindowsRuntime;
using Glancy.Core.Settings;
using Microsoft.UI.Xaml.Media.Imaging;

namespace Glancy.Settings.Helpers;

/// <summary>Generates checkerboard bitmaps (WinUI brushes cannot tile images).</summary>
public static class CheckerBitmap
{
    private static readonly Dictionary<(uint, uint, int, int, int), WriteableBitmap> Cache = [];

    public static WriteableBitmap Create(CheckerColors colors, int width, int height, int cell)
    {
        width = Math.Max(1, width);
        height = Math.Max(1, height);
        cell = Math.Max(1, cell);
        var key = (colors.Light, colors.Dark, width, height, cell);
        if (Cache.TryGetValue(key, out var cached)) return cached;

        var pixels = new byte[width * height * 4];
        for (var y = 0; y < height; y++)
        {
            for (var x = 0; x < width; x++)
            {
                var c = ((x / cell) + (y / cell)) % 2 == 0 ? colors.Light : colors.Dark;
                var i = (y * width + x) * 4;
                pixels[i] = (byte)c;              // B
                pixels[i + 1] = (byte)(c >> 8);   // G
                pixels[i + 2] = (byte)(c >> 16);  // R
                pixels[i + 3] = 255;
            }
        }

        var bmp = new WriteableBitmap(width, height);
        using (var stream = bmp.PixelBuffer.AsStream()) stream.Write(pixels, 0, pixels.Length);
        bmp.Invalidate();
        if (Cache.Count > 64) Cache.Clear();
        Cache[key] = bmp;
        return bmp;
    }

    public static WriteableBitmap FromPixels(int width, int height, byte[] premultipliedBgra)
    {
        var bmp = new WriteableBitmap(width, height);
        using (var stream = bmp.PixelBuffer.AsStream()) stream.Write(premultipliedBgra, 0, premultipliedBgra.Length);
        bmp.Invalidate();
        return bmp;
    }
}
