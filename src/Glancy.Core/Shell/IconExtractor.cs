namespace Glancy.Core.Shell;

/// <summary>An icon as premultiplied BGRA pixels, ready for a WriteableBitmap.</summary>
public sealed record IconImage(int Index, int Width, int Height, byte[] PremultipliedBgra);

/// <summary>Reads the icons contained in an .ico, .exe or .dll file.</summary>
public static class IconExtractor
{
    public const int MaxIcons = 400;

    public static int CountIcons(string file) =>
        File.Exists(file) ? (int)Math.Min(NativeMethods.ExtractIconExW(file, -1, null, null, 0), MaxIcons) : 0;

    public static IReadOnlyList<IconImage> ExtractAll(string file, int size)
    {
        var list = new List<IconImage>();
        var count = CountIcons(file);
        for (var i = 0; i < count; i++)
        {
            var icon = Extract(file, i, size);
            if (icon is not null) list.Add(icon);
        }
        return list;
    }

    public static IconImage? Extract(string file, int index, int size)
    {
        var handles = new IntPtr[1];
        var ids = new IntPtr[1];
        if (NativeMethods.PrivateExtractIconsW(file, index, size, size, handles, ids, 1, 0) == 0 || handles[0] == IntPtr.Zero)
            return null;
        try
        {
            return ToImage(handles[0], index);
        }
        finally
        {
            NativeMethods.DestroyIcon(handles[0]);
        }
    }

    /// <summary>Parses "path,index" as stored in the customIconPath setting.</summary>
    public static (string Path, int Index) ParseLocation(string location)
    {
        var comma = location.LastIndexOf(',');
        if (comma > 0 && int.TryParse(location[(comma + 1)..], out var index)) return (location[..comma].Trim('"', ' '), index);
        return (location.Trim('"', ' '), 0);
    }

    public static string FormatLocation(string path, int index) => $"{path},{index}";

    private static IconImage? ToImage(IntPtr hIcon, int index)
    {
        if (!NativeMethods.GetIconInfo(hIcon, out var info)) return null;
        try
        {
            if (info.hbmColor == IntPtr.Zero) return null;
            NativeMethods.GetObjectW(info.hbmColor, System.Runtime.InteropServices.Marshal.SizeOf<NativeMethods.BITMAP>(), out var bmp);
            int w = bmp.bmWidth, h = bmp.bmHeight;
            var color = ReadBits(info.hbmColor, w, h);
            if (color is null) return null;

            var hasAlpha = false;
            for (var i = 3; i < color.Length; i += 4) if (color[i] != 0) { hasAlpha = true; break; }

            if (!hasAlpha)
            {
                // Legacy icon without alpha: derive transparency from the AND mask.
                var mask = info.hbmMask != IntPtr.Zero ? ReadBits(info.hbmMask, w, h) : null;
                for (var p = 0; p < w * h; p++)
                    color[p * 4 + 3] = mask is not null && mask[p * 4] != 0 ? (byte)0 : (byte)255;
            }

            for (var p = 0; p < color.Length; p += 4)
            {
                var a = color[p + 3];
                color[p] = (byte)(color[p] * a / 255);
                color[p + 1] = (byte)(color[p + 1] * a / 255);
                color[p + 2] = (byte)(color[p + 2] * a / 255);
            }
            return new IconImage(index, w, h, color);
        }
        finally
        {
            if (info.hbmColor != IntPtr.Zero) NativeMethods.DeleteObject(info.hbmColor);
            if (info.hbmMask != IntPtr.Zero) NativeMethods.DeleteObject(info.hbmMask);
        }
    }

    private static byte[]? ReadBits(IntPtr hbm, int w, int h)
    {
        var header = new NativeMethods.BITMAPINFOHEADER
        {
            biSize = (uint)System.Runtime.InteropServices.Marshal.SizeOf<NativeMethods.BITMAPINFOHEADER>(),
            biWidth = w,
            biHeight = -h, // top-down rows
            biPlanes = 1,
            biBitCount = 32,
        };
        var bits = new byte[w * h * 4];
        var dc = NativeMethods.CreateCompatibleDC(IntPtr.Zero);
        try
        {
            return NativeMethods.GetDIBits(dc, hbm, 0, (uint)h, bits, ref header, 0) == h ? bits : null;
        }
        finally
        {
            NativeMethods.DeleteDC(dc);
        }
    }
}
