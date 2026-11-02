using System.Diagnostics;
using Microsoft.Win32;

namespace Glancy.Core.Shell;

/// <summary>Why Explorer will draw no overlay icon although the settings ask for one.</summary>
public enum OverlayNote
{
    None,
    /// <summary>No app opens the type, so there is no app icon to show (a custom icon still works).</summary>
    NoApp,
    /// <summary>The type's ProgID sets its own <c>TypeOverlay</c>, which Explorer reads before Glancy's.</summary>
    FileTypeOverrides,
}

/// <summary>The icon Explorer will draw in the corner of a thumbnail (<c>"path,index"</c> or <c>@{…}</c>), if any.</summary>
public sealed record OverlayPreview(string? IconLocation, OverlayNote Note);

/// <summary>
/// The overlay icon as Explorer will draw it, worked out the same way the engine writes it
/// (src/Glancy.Thumbnails/src/registration.rs, <c>overlay_values</c>; docs/architecture.md, Registration), and the
/// engine's <c>sync</c> command, which writes it right away.
/// </summary>
public static class OverlayIcon
{
    /// <summary>
    /// The app's icon reference, or else the first icon of its executable (as the engine does). Windows may give the
    /// reference as a bare path, which gets the index 0.
    /// </summary>
    public static string? AppIconFrom(string? iconReference, string? executable)
    {
        var location = !string.IsNullOrWhiteSpace(iconReference) ? iconReference.Trim()
            : !string.IsNullOrWhiteSpace(executable) ? executable.Trim()
            : null;
        if (location is null) return null;
        var comma = location.LastIndexOf(',');
        var indexed = comma >= 0 && int.TryParse(location[(comma + 1)..].Trim(), out _);
        return location.StartsWith('@') || indexed ? location : $"{location},0";
    }

    public static OverlayPreview Resolve(bool showAppIcon, string? customIcon, bool unassociated, bool fileTypeSetsOwn, string? appIcon)
    {
        if (fileTypeSetsOwn) return new OverlayPreview(null, OverlayNote.FileTypeOverrides);
        if (!showAppIcon) return new OverlayPreview(null, OverlayNote.None);
        if (!string.IsNullOrWhiteSpace(customIcon)) return new OverlayPreview(customIcon.Trim(), OverlayNote.None);
        if (unassociated || appIcon is null) return new OverlayPreview(null, OverlayNote.NoApp);
        return new OverlayPreview(appIcon, OverlayNote.None);
    }

    /// <summary>Windows reports <c>Unknown</c> (or nothing) as the ProgID of a type no app opens.</summary>
    public static bool IsUnassociated(string extension, string? progId) =>
        string.IsNullOrWhiteSpace(progId)
        || progId.Equals("Unknown", StringComparison.OrdinalIgnoreCase)
        || progId.Equals(extension, StringComparison.OrdinalIgnoreCase);

    /// <summary>Reads the association of <paramref name="extension"/> (a few registry reads).</summary>
    public static OverlayPreview ForExtension(string extension, bool showAppIcon, string? customIcon)
    {
        var progId = Query(extension, NativeMethods.ASSOCSTR_PROGID, null);
        var unassociated = IsUnassociated(extension, progId);
        var ownOverlay = false;
        if (!unassociated)
        {
            using var key = Registry.ClassesRoot.OpenSubKey(progId!);
            ownOverlay = key?.GetValue("TypeOverlay") is not null;
        }
        var appIcon = unassociated ? null : AppIconFrom(
            Query(extension, NativeMethods.ASSOCSTR_APPICONREFERENCE, null),
            Query(extension, NativeMethods.ASSOCSTR_EXECUTABLE, "open"));
        return Resolve(showAppIcon, customIcon, unassociated, ownOverlay, appIcon);
    }

    internal static string SyncArguments(string dll) => $"/s /n /i:sync \"{dll}\"";

    /// <summary>
    /// Has the engine write <c>Treatment</c>/<c>TypeOverlay</c> from the saved settings now
    /// (<c>regsvr32 /n /i:sync</c>), instead of on the next thumbnail it draws. Returns false if it failed.
    /// </summary>
    public static async Task<bool> SyncEngineAsync(string dll, CancellationToken cancel)
    {
        var tool = Path.Combine(Environment.SystemDirectory, "regsvr32.exe");
        using var p = Process.Start(new ProcessStartInfo(tool, SyncArguments(Environment.ExpandEnvironmentVariables(dll)))
        {
            UseShellExecute = false,
            CreateNoWindow = true,
        });
        if (p is null) return false;
        await p.WaitForExitAsync(cancel);
        return p.ExitCode == 0;
    }

    private static string? Query(string extension, int what, string? extra)
    {
        var buffer = new char[1024];
        var length = (uint)buffer.Length;
        var hr = NativeMethods.AssocQueryStringW(0, what, extension, extra, buffer, ref length);
        return hr == 0 && length > 1 ? new string(buffer, 0, (int)length - 1) : null;
    }
}
