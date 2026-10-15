using System.Runtime.InteropServices;

namespace Glancy.Core.Shell;

public sealed record RefreshResult(int FolderCount, int FileCount, bool LimitReached);

/// <summary>
/// Makes open Explorer windows (and the desktop) show thumbnails generated with the new settings: optionally
/// forces re-extraction through the system thumbnail cache, then notifies the shell that each file changed.
/// </summary>
public static class ExplorerRefresher
{
    public const int MaxFiles = 2000;
    private const uint WTS_FORCEEXTRACTION = 0x4;
    private static readonly uint[] ExtractionSizes = [96, 256];

    public static Task<RefreshResult> RefreshAsync(
        IReadOnlyCollection<string> extensions, bool recursive, bool forceExtraction, CancellationToken cancellationToken = default) =>
        StaRunner.Run(() => Refresh(extensions, recursive, forceExtraction, cancellationToken));

    /// <summary>Paths of the folders shown in open File Explorer windows, plus the desktop.</summary>
    public static IReadOnlyList<string> GetOpenFolders()
    {
        var folders = new HashSet<string>(StringComparer.OrdinalIgnoreCase)
        {
            Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),
        };
        var shellType = Type.GetTypeFromProgID("Shell.Application");
        if (shellType is null) return folders.ToList();

        dynamic? shell = null;
        try
        {
            shell = Activator.CreateInstance(shellType);
            foreach (var window in shell!.Windows())
            {
                try
                {
                    string? path = window?.Document?.Folder?.Self?.Path;
                    if (!string.IsNullOrEmpty(path) && Directory.Exists(path)) folders.Add(path);
                }
                catch (Exception ex) when (ex is COMException or Microsoft.CSharp.RuntimeBinder.RuntimeBinderException)
                {
                    // Not a file-system folder window (e.g. Control Panel, Home): skip it.
                }
            }
        }
        catch (COMException)
        {
            // Explorer not running or shell windows unavailable: the desktop alone is still refreshed.
        }
        finally
        {
            if (shell is not null) Marshal.FinalReleaseComObject(shell);
        }
        return folders.ToList();
    }

    private static RefreshResult Refresh(IReadOnlyCollection<string> extensions, bool recursive, bool forceExtraction, CancellationToken ct)
    {
        var wanted = new HashSet<string>(extensions, StringComparer.OrdinalIgnoreCase);
        var folders = GetOpenFolders();
        var options = new EnumerationOptions { IgnoreInaccessible = true, RecurseSubdirectories = recursive };

        var files = new List<string>();
        foreach (var folder in folders)
        {
            foreach (var file in Directory.EnumerateFiles(folder, "*", options))
            {
                if (!wanted.Contains(Path.GetExtension(file))) continue;
                files.Add(file);
                if (files.Count >= MaxFiles) break;
            }
            if (files.Count >= MaxFiles) break;
        }

        IThumbnailCache? cache = null;
        if (forceExtraction && files.Count > 0)
        {
            var type = Type.GetTypeFromCLSID(NativeMethods.CLSID_LocalThumbnailCache);
            cache = type is null ? null : Activator.CreateInstance(type) as IThumbnailCache;
        }

        try
        {
            foreach (var file in files)
            {
                ct.ThrowIfCancellationRequested();
                if (cache is not null) ForceExtraction(cache, file);
                var p = Marshal.StringToHGlobalUni(file);
                try { NativeMethods.SHChangeNotify(NativeMethods.SHCNE_UPDATEITEM, NativeMethods.SHCNF_PATHW | NativeMethods.SHCNF_FLUSHNOWAIT, p, IntPtr.Zero); }
                finally { Marshal.FreeHGlobal(p); }
            }
        }
        finally
        {
            if (cache is not null) Marshal.FinalReleaseComObject(cache);
        }

        NativeMethods.SHChangeNotify(NativeMethods.SHCNE_ASSOCCHANGED, NativeMethods.SHCNF_IDLIST, IntPtr.Zero, IntPtr.Zero);
        return new RefreshResult(folders.Count, files.Count, files.Count >= MaxFiles);
    }

    private static void ForceExtraction(IThumbnailCache cache, string file)
    {
        if (NativeMethods.SHCreateItemFromParsingName(file, IntPtr.Zero, NativeMethods.IID_IShellItem, out var item) != 0) return;
        try
        {
            foreach (var size in ExtractionSizes)
            {
                if (cache.GetThumbnail(item, size, WTS_FORCEEXTRACTION, out var bitmap, IntPtr.Zero, IntPtr.Zero) >= 0 && bitmap != IntPtr.Zero)
                    Marshal.Release(bitmap);
            }
        }
        finally
        {
            Marshal.Release(item);
        }
    }
}
