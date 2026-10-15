using System.Runtime.InteropServices;

namespace Glancy.Core.Shell;

internal static partial class NativeMethods
{
    public const int SHCNE_UPDATEITEM = 0x00002000;
    public const int SHCNE_ASSOCCHANGED = 0x08000000;
    public const uint SHCNF_IDLIST = 0x0000;
    public const uint SHCNF_PATHW = 0x0005;
    public const uint SHCNF_FLUSHNOWAIT = 0x3000;

    public const int ASSOCSTR_SHELLEXTENSION = 16;

    [LibraryImport("shlwapi.dll", StringMarshalling = StringMarshalling.Utf16)]
    public static partial int AssocQueryStringW(int flags, int str, string assoc, string? extra, [Out] char[] output, ref uint length);

    [LibraryImport("shell32.dll")]
    public static partial void SHChangeNotify(int wEventId, uint uFlags, IntPtr dwItem1, IntPtr dwItem2);

    [LibraryImport("shell32.dll", StringMarshalling = StringMarshalling.Utf16)]
    public static partial int SHCreateItemFromParsingName(string path, IntPtr pbc, in Guid riid, out IntPtr ppv);

    [LibraryImport("shell32.dll", StringMarshalling = StringMarshalling.Utf16)]
    public static partial uint ExtractIconExW(string file, int index, IntPtr[]? large, IntPtr[]? small, uint count);

    [LibraryImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool DestroyIcon(IntPtr hIcon);

    [LibraryImport("user32.dll", StringMarshalling = StringMarshalling.Utf16)]
    public static partial uint PrivateExtractIconsW(string file, int index, int cx, int cy, IntPtr[]? icons, IntPtr[]? ids, uint count, uint flags);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool GetIconInfo(IntPtr hIcon, out ICONINFO info);

    [LibraryImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool DeleteObject(IntPtr obj);

    [LibraryImport("gdi32.dll")]
    public static partial int GetObjectW(IntPtr h, int size, out BITMAP bmp);

    [LibraryImport("gdi32.dll")]
    public static partial IntPtr CreateCompatibleDC(IntPtr hdc);

    [LibraryImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool DeleteDC(IntPtr hdc);

    [LibraryImport("gdi32.dll")]
    public static partial int GetDIBits(IntPtr hdc, IntPtr hbm, uint start, uint lines, byte[]? bits, ref BITMAPINFOHEADER bmi, uint usage);

    [StructLayout(LayoutKind.Sequential)]
    public struct ICONINFO
    {
        public int fIcon;
        public int xHotspot;
        public int yHotspot;
        public IntPtr hbmMask;
        public IntPtr hbmColor;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct BITMAP
    {
        public int bmType;
        public int bmWidth;
        public int bmHeight;
        public int bmWidthBytes;
        public ushort bmPlanes;
        public ushort bmBitsPixel;
        public IntPtr bmBits;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct BITMAPINFOHEADER
    {
        public uint biSize;
        public int biWidth;
        public int biHeight;
        public ushort biPlanes;
        public ushort biBitCount;
        public uint biCompression;
        public uint biSizeImage;
        public int biXPelsPerMeter;
        public int biYPelsPerMeter;
        public uint biClrUsed;
        public uint biClrImportant;
    }

    public static readonly Guid IID_IShellItem = new("43826d1e-e718-42ee-bc55-a1e261c37bfe");
    public static readonly Guid CLSID_LocalThumbnailCache = new("50EF4544-AC9F-4A8E-B21B-8A26180DB13F");
    public static readonly Guid CLSID_ThumbnailCacheCleaner = new("889900c3-59f3-4c2f-ae21-a409ea01e605");
}

/// <summary>IThumbnailCache (thumbcache.h). Only GetThumbnail is called; pointers are kept raw to avoid extra interop types.</summary>
[ComImport, Guid("F676C15D-596A-4ce2-8234-33996F445DB1"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IThumbnailCache
{
    [PreserveSig]
    int GetThumbnail(IntPtr shellItem, uint requestedSize, uint flags, out IntPtr sharedBitmap, IntPtr outFlags, IntPtr thumbnailId);

    /// <summary>Unused; declared to keep the vtable layout. WTS_THUMBNAILID is a 16-byte struct passed by value.</summary>
    [PreserveSig]
    int GetThumbnailByID(Guid thumbnailId, uint requestedSize, out IntPtr sharedBitmap, IntPtr outFlags);
}

/// <summary>IEmptyVolumeCache (emptyvc.h), implemented by Windows' own "Thumbnails" Disk Cleanup handler.</summary>
[ComImport, Guid("8FCE5227-04DA-11d1-A004-00805F8ABE06"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IEmptyVolumeCache
{
    [PreserveSig]
    int Initialize(IntPtr hkRegKey, [MarshalAs(UnmanagedType.LPWStr)] string volume, out IntPtr displayName, out IntPtr description, out uint flags);

    [PreserveSig]
    int GetSpaceUsed(out ulong spaceUsed, IntPtr callback);

    [PreserveSig]
    int Purge(ulong spaceToFree, IntPtr callback);

    [PreserveSig]
    int ShowProperties(IntPtr hwnd);

    [PreserveSig]
    int Deactivate(out uint flags);
}
