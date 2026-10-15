using Glancy.Core.Resources;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.Marshalling;
using Microsoft.Win32;

namespace Glancy.Core.Shell;

public sealed record CleanupResult(bool Completed, ulong BytesFreed, string Message);

/// <summary>
/// Clears the Windows thumbnail and icon caches using Windows' own mechanisms: the "Thumbnails" Disk Cleanup
/// handler and <c>ie4uinit -show</c>. Glancy never deletes cache files by hand.
/// </summary>
public static partial class CacheCleaner
{
    private const string ThumbnailHandlerKey = @"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\VolumeCaches\Thumbnail Cache";

    public static Task<CleanupResult> ClearThumbnailCacheAsync() => StaRunner.Run(ClearThumbnailCache);

    public static async Task<CleanupResult> ClearIconCacheAsync()
    {
        var tool = Path.Combine(Environment.SystemDirectory, "ie4uinit.exe");
        if (!File.Exists(tool))
            return new CleanupResult(false, 0, CoreStrings.Cache_IconToolMissing);

        using var p = Process.Start(new ProcessStartInfo(tool, "-show") { UseShellExecute = false, CreateNoWindow = true });
        if (p is not null)
        {
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(30));
            try { await p.WaitForExitAsync(timeout.Token); }
            catch (OperationCanceledException) { return new CleanupResult(false, 0, CoreStrings.Cache_IconTimeout); }
        }
        NativeMethods.SHChangeNotify(NativeMethods.SHCNE_ASSOCCHANGED, NativeMethods.SHCNF_IDLIST, IntPtr.Zero, IntPtr.Zero);
        return new CleanupResult(true, 0, CoreStrings.Cache_IconDone);
    }

    /// <summary>Opens Windows' Disk Cleanup for the system drive as a manual alternative.</summary>
    public static void OpenDiskCleanup()
    {
        var drive = Path.GetPathRoot(Environment.SystemDirectory)?.TrimEnd('\\') ?? "C:";
        Process.Start(new ProcessStartInfo("cleanmgr.exe", $"/d {drive}") { UseShellExecute = true });
    }

    private static CleanupResult ClearThumbnailCache()
    {
        const int E_ABORT = unchecked((int)0x80004004);
        IEmptyVolumeCache? cleaner = null;
        var initialized = false;
        var callbackPtr = IntPtr.Zero;
        try
        {
            // GetTypeFromCLSID never returns null; creation itself fails (REGDB_E_CLASSNOTREG) when the
            // handler is missing, e.g. on Server Core, so it belongs inside the try.
            cleaner = (IEmptyVolumeCache)Activator.CreateInstance(Type.GetTypeFromCLSID(NativeMethods.CLSID_ThumbnailCacheCleaner)!)!;

            using var key = Registry.LocalMachine.OpenSubKey(ThumbnailHandlerKey);
            var hkey = key?.Handle.DangerousGetHandle() ?? IntPtr.Zero;
            var volume = Path.GetPathRoot(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData)) ?? @"C:\";

            var hr = cleaner.Initialize(hkey, volume, out var name, out var description, out _);
            Marshal.FreeCoTaskMem(name);
            Marshal.FreeCoTaskMem(description);
            // emptyvc.h: Initialize returns E_ABORT when there is nothing to clean.
            if (hr == E_ABORT) return new CleanupResult(true, 0, CoreStrings.Cache_AlreadyEmpty);
            Marshal.ThrowExceptionForHR(hr);
            initialized = true;

            callbackPtr = ComWrappersSupport.GetCallbackPointer(new CleanupCallback());
            cleaner.GetSpaceUsed(out var used, callbackPtr);
            Marshal.ThrowExceptionForHR(cleaner.Purge(ulong.MaxValue, callbackPtr));

            NativeMethods.SHChangeNotify(NativeMethods.SHCNE_ASSOCCHANGED, NativeMethods.SHCNF_IDLIST, IntPtr.Zero, IntPtr.Zero);
            return new CleanupResult(true, used, CoreStrings.Cache_Cleared);
        }
        catch (Exception ex) when (ex is COMException or UnauthorizedAccessException or InvalidCastException or TypeLoadException)
        {
            return new CleanupResult(false, 0, string.Format(CoreStrings.Cache_Denied, ex.Message.Trim()));
        }
        finally
        {
            if (initialized) cleaner!.Deactivate(out _);
            if (callbackPtr != IntPtr.Zero) Marshal.Release(callbackPtr);
            if (cleaner is not null) Marshal.FinalReleaseComObject(cleaner);
        }
    }

    /// <summary>Progress callback the cleanup handler may call; it only acknowledges progress.</summary>
    [GeneratedComClass]
    internal sealed partial class CleanupCallback : IEmptyVolumeCacheCallBack
    {
        public int ScanProgress(ulong spaceUsed, uint flags, IntPtr status) => 0;
        public int PurgeProgress(ulong spaceFreed, ulong spaceToFree, uint flags, IntPtr status) => 0;
    }
}

[GeneratedComInterface, Guid("6E793361-73C6-11D0-8469-00AA00442901")]
internal partial interface IEmptyVolumeCacheCallBack
{
    [PreserveSig] int ScanProgress(ulong spaceUsed, uint flags, IntPtr status);
    [PreserveSig] int PurgeProgress(ulong spaceFreed, ulong spaceToFree, uint flags, IntPtr status);
}

internal static class ComWrappersSupport
{
    private static readonly StrategyBasedComWrappers Wrappers = new();

    /// <summary>Returns an AddRef'd IEmptyVolumeCacheCallBack pointer for the managed callback.</summary>
    public static IntPtr GetCallbackPointer(object callback)
    {
        var unknown = Wrappers.GetOrCreateComInterfaceForObject(callback, CreateComInterfaceFlags.None);
        try
        {
            var iid = typeof(IEmptyVolumeCacheCallBack).GUID;
            Marshal.ThrowExceptionForHR(Marshal.QueryInterface(unknown, in iid, out var ptr));
            return ptr;
        }
        finally
        {
            Marshal.Release(unknown);
        }
    }
}
