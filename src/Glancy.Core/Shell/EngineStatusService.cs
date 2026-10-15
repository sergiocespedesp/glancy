using Glancy.Core.Resources;
using Glancy.Core.Settings;
using Microsoft.Win32;

namespace Glancy.Core.Shell;

public enum HandlerOwner { None, Glancy, Other }

/// <summary>Which thumbnail provider Explorer will use for an extension.</summary>
public sealed record ThumbnailHandlerInfo(string Extension, HandlerOwner Owner, Guid? Clsid, string? Name)
{
    public string Describe() => Owner switch
    {
        HandlerOwner.Glancy => "Glancy",
        HandlerOwner.Other => string.IsNullOrWhiteSpace(Name) ? string.Format(CoreStrings.Handler_Other, $"{Clsid:B}") : Name!,
        _ => CoreStrings.Handler_None,
    };
}

public sealed record EngineStatus(bool IsInstalled, string? DllPath, IReadOnlyList<ThumbnailHandlerInfo> Handlers)
{
    private Dictionary<string, ThumbnailHandlerInfo>? _byExtension;

    /// <summary>The handler for an extension, or a "none" entry if the extension was not checked.</summary>
    public ThumbnailHandlerInfo HandlerFor(string extension)
    {
        _byExtension ??= Handlers.GroupBy(h => h.Extension, StringComparer.OrdinalIgnoreCase)
            .ToDictionary(g => g.Key, g => g.First(), StringComparer.OrdinalIgnoreCase);
        return _byExtension.TryGetValue(extension, out var h) ? h : new ThumbnailHandlerInfo(extension, HandlerOwner.None, null, null);
    }

    public bool IsActiveFor(string extension) => HandlerFor(extension).Owner == HandlerOwner.Glancy;

    public int ActiveExtensionCount => Handlers.Count(h => h.Owner == HandlerOwner.Glancy);
}

/// <summary>
/// Reads (never writes) the shell registration to tell the user whether the Glancy engine is installed and which
/// provider Explorer currently resolves for each extension. Lookup order mirrors Explorer, which gives the ProgID
/// priority over the extension key (verified: docs/architecture.md, Registration): the UserChoice ProgID, the default
/// ProgID, the extension, then SystemFileAssociations for the extension and for its perceived type. The engine
/// registers itself in the same order (src/Glancy.Thumbnails/src/registration.rs, <c>handler_classes</c>).
/// </summary>
public static class EngineStatusService
{
    /// <summary>
    /// Reads the registration of every catalog extension (a few hundred registry reads, ~0.1–0.3 s): call it off the
    /// UI thread (<see cref="GetStatusAsync"/>).
    /// </summary>
    public static EngineStatus GetStatus()
    {
        string? dll = null;
        using (var inproc = Registry.ClassesRoot.OpenSubKey($@"CLSID\{AppInfo.ThumbnailProviderClsid:B}\InprocServer32"))
        {
            dll = inproc?.GetValue(null) as string;
        }
        var installed = dll is not null && File.Exists(Environment.ExpandEnvironmentVariables(dll));
        var names = new Dictionary<Guid, string?>();
        var handlers = Catalog.AllExtensions.Select(e => GetHandler(e, names)).ToList();
        return new EngineStatus(installed, dll, handlers);
    }

    public static Task<EngineStatus> GetStatusAsync() => Task.Run(GetStatus);

    public static ThumbnailHandlerInfo GetHandler(string extension) => GetHandler(extension, new Dictionary<Guid, string?>());

    /// <param name="names">CLSID → display name cache shared across one status read (most types share a few providers).</param>
    private static ThumbnailHandlerInfo GetHandler(string extension, Dictionary<Guid, string?> names)
    {
        try
        {
            // Ask Windows itself first: it also honours associations the registry walk below cannot see, such as
            // Windows 11's UserChoiceLatest and app-registered ProgIDs (verified: docs/architecture.md, Registration).
            if (ResolvedByWindows(extension) is { } resolved)
            {
                if (resolved == AppInfo.ThumbnailProviderClsid)
                    return new ThumbnailHandlerInfo(extension, HandlerOwner.Glancy, resolved, AppInfo.ProductName);
                if (!names.TryGetValue(resolved, out var resolvedName)) names[resolved] = resolvedName = ClsidName(resolved);
                return new ThumbnailHandlerInfo(extension, HandlerOwner.Other, resolved, resolvedName);
            }
            foreach (var path in CandidateKeys(extension))
            {
                using var k = Registry.ClassesRoot.OpenSubKey($@"{path}\ShellEx\{AppInfo.ThumbnailHandlerShellExKey}");
                if (k?.GetValue(null) is not string text || !Guid.TryParse(text, out var clsid)) continue;
                if (clsid == AppInfo.ThumbnailProviderClsid)
                    return new ThumbnailHandlerInfo(extension, HandlerOwner.Glancy, clsid, AppInfo.ProductName);
                if (!names.TryGetValue(clsid, out var name)) names[clsid] = name = ClsidName(clsid);
                return new ThumbnailHandlerInfo(extension, HandlerOwner.Other, clsid, name);
            }
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or System.Security.SecurityException or IOException)
        {
            // Unreadable association: report as no handler rather than failing the whole status page.
        }
        return new ThumbnailHandlerInfo(extension, HandlerOwner.None, null, null);
    }

    /// <summary>The thumbnail handler CLSID Explorer resolves for the extension, or null when there is none.</summary>
    private static Guid? ResolvedByWindows(string extension)
    {
        var buffer = new char[128];
        var length = (uint)buffer.Length;
        var hr = NativeMethods.AssocQueryStringW(0, NativeMethods.ASSOCSTR_SHELLEXTENSION, extension,
            AppInfo.ThumbnailHandlerShellExKey, buffer, ref length);
        if (hr != 0 || length < 2) return null;
        return Guid.TryParse(new string(buffer, 0, (int)length - 1), out var clsid) ? clsid : null;
    }

    private static IEnumerable<string> CandidateKeys(string extension)
    {
        string? userChoice, defaultProgId, perceivedType;
        using (var uc = Registry.CurrentUser.OpenSubKey(
                   $@"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{extension}\UserChoice"))
        {
            userChoice = uc?.GetValue("ProgId") as string;
        }
        using (var ext = Registry.ClassesRoot.OpenSubKey(extension))
        {
            defaultProgId = ext?.GetValue(null) as string;
            perceivedType = ext?.GetValue("PerceivedType") as string;
        }
        return CandidateKeys(extension, userChoice, defaultProgId, perceivedType);
    }

    /// <summary>Keys under HKCR to look for the thumbnail handler, highest priority first, without duplicates.</summary>
    internal static IReadOnlyList<string> CandidateKeys(
        string extension, string? userChoiceProgId, string? defaultProgId, string? perceivedType)
    {
        var keys = new List<string>();
        void Add(string? key)
        {
            if (!string.IsNullOrWhiteSpace(key) && !keys.Contains(key.Trim(), StringComparer.OrdinalIgnoreCase))
                keys.Add(key.Trim());
        }
        Add(userChoiceProgId);
        Add(defaultProgId);
        Add(extension);
        Add($@"SystemFileAssociations\{extension}");
        if (!string.IsNullOrWhiteSpace(perceivedType))
            Add($@"SystemFileAssociations\{perceivedType.Trim().ToLowerInvariant()}");
        return keys;
    }

    private static string? ClsidName(Guid clsid)
    {
        using var k = Registry.ClassesRoot.OpenSubKey($@"CLSID\{clsid:B}");
        if (k?.GetValue(null) is string name && !string.IsNullOrWhiteSpace(name)) return name;
        using var inproc = k?.OpenSubKey("InprocServer32");
        return inproc?.GetValue(null) is string dll ? Path.GetFileName(dll) : null;
    }
}
