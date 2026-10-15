using System.Globalization;
using System.Runtime.CompilerServices;
using Microsoft.Win32;

namespace Glancy.Core.Tests;

/// <summary>Tests run in English whatever the machine's language; <see cref="UICulture"/> switches for one test.</summary>
internal static class TestCulture
{
    [ModuleInitializer]
    internal static void UseEnglish()
    {
        CultureInfo.DefaultThreadCurrentUICulture = CultureInfo.GetCultureInfo("en");
        CultureInfo.CurrentUICulture = CultureInfo.GetCultureInfo("en");
    }
}

/// <summary>Sets the UI culture for the current test (async flow) and restores it on dispose.</summary>
internal sealed class UICulture : IDisposable
{
    private readonly CultureInfo _previous = CultureInfo.CurrentUICulture;

    public UICulture(string name) => CultureInfo.CurrentUICulture = CultureInfo.GetCultureInfo(name);

    public void Dispose() => CultureInfo.CurrentUICulture = _previous;
}

/// <summary>Paths in the repository, found by walking up from the test binaries to the folder with Glancy.slnx.</summary>
internal static class Repo
{
    public static string Root { get; } = FindRoot();

    public static string Path(params string[] parts) => System.IO.Path.Combine([Root, .. parts]);

    private static string FindRoot()
    {
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            if (File.Exists(System.IO.Path.Combine(dir.FullName, "Glancy.slnx"))) return dir.FullName;
        }
        // Build output lives outside the repo (Directory.Build.props): fall back to the source path of this file.
        return SourceRoot();
    }

    private static string SourceRoot([CallerFilePath] string file = "") =>
        System.IO.Path.GetFullPath(System.IO.Path.Combine(System.IO.Path.GetDirectoryName(file)!, "..", ".."));
}

/// <summary>In-memory <see cref="ISettingsStore"/> that records saves.</summary>
internal sealed class InMemorySettingsStore : ISettingsStore
{
    public SettingsSnapshot Stored { get; set; } = SettingsSnapshot.CreateDefaults();
    public int SaveCount { get; private set; }
    public int LoadCount { get; private set; }

    public SettingsSnapshot Load()
    {
        LoadCount++;
        return Stored.Clone();
    }

    public void Save(SettingsSnapshot snapshot)
    {
        SaveCount++;
        Stored = snapshot.Clone();
    }
}

/// <summary>
/// A unique HKCU test key (Software\Glancy.Tests\{guid}). Deleted on dispose. Never touches Software\Glancy.
/// </summary>
internal sealed class TestRegistryKey : IDisposable
{
    public const string Parent = @"Software\Glancy.Tests";

    public TestRegistryKey()
    {
        Path = $@"{Parent}\{Guid.NewGuid():N}";
        // Guard rail: the real product key must never be used by tests.
        if (Path.StartsWith(@"Software\Glancy\", StringComparison.OrdinalIgnoreCase) ||
            string.Equals(Path, AppInfo.SettingsKeyPath, StringComparison.OrdinalIgnoreCase) ||
            string.Equals(Path, AppInfo.AppPreferencesKeyPath, StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("Refusing to use the real Glancy key in tests.");
    }

    public string Path { get; }

    public RegistryKey? Open(string? sub = null, bool writable = false) =>
        Registry.CurrentUser.OpenSubKey(sub is null ? Path : $@"{Path}\{sub}", writable);

    public RegistryKey Create(string sub) => Registry.CurrentUser.CreateSubKey($@"{Path}\{sub}", writable: true);

    public void Dispose()
    {
        try { Registry.CurrentUser.DeleteSubKeyTree(Path, throwOnMissingSubKey: false); }
        finally
        {
            try
            {
                using var parent = Registry.CurrentUser.OpenSubKey(Parent);
                if (parent is not null && parent.SubKeyCount == 0 && parent.ValueCount == 0)
                    Registry.CurrentUser.DeleteSubKey(Parent, throwOnMissingSubKey: false);
            }
            catch (InvalidOperationException) { /* another test still uses the parent */ }
            catch (IOException) { }
        }
    }
}

internal static class SnapshotAssert
{
    public static void Equal(SettingsSnapshot expected, SettingsSnapshot actual)
    {
        AssertDict("global", expected.Global, actual.Global);
        Assert.Equal(expected.Formats.Keys.OrderBy(k => k, StringComparer.OrdinalIgnoreCase),
                     actual.Formats.Keys.OrderBy(k => k, StringComparer.OrdinalIgnoreCase), StringComparer.OrdinalIgnoreCase);
        foreach (var (id, values) in expected.Formats) AssertDict($"format {id}", values, actual.Formats[id]);

        var expOverrides = expected.Overrides.Where(o => o.Value.Count > 0).ToDictionary(o => o.Key, o => o.Value, StringComparer.OrdinalIgnoreCase);
        var actOverrides = actual.Overrides.Where(o => o.Value.Count > 0).ToDictionary(o => o.Key, o => o.Value, StringComparer.OrdinalIgnoreCase);
        Assert.Equal(expOverrides.Keys.OrderBy(k => k, StringComparer.OrdinalIgnoreCase),
                     actOverrides.Keys.OrderBy(k => k, StringComparer.OrdinalIgnoreCase), StringComparer.OrdinalIgnoreCase);
        foreach (var (ext, values) in expOverrides) AssertDict($"override {ext}", values, actOverrides[ext]);
    }

    private static void AssertDict(string label, Dictionary<string, object> expected, Dictionary<string, object> actual)
    {
        Assert.True(expected.Keys.ToHashSet().SetEquals(actual.Keys),
            $"{label}: keys differ. Expected [{string.Join(",", expected.Keys)}], actual [{string.Join(",", actual.Keys)}]");
        foreach (var (k, v) in expected)
            Assert.True(SettingsSnapshot.ValueEquals(v, actual[k]), $"{label}.{k}: expected {Show(v)}, actual {Show(actual[k])}");
    }

    private static string Show(object? v) => v is string[] a ? "[" + string.Join(",", a) + "]" : v?.ToString() ?? "null";
}

/// <summary>A snapshot with values changed in every kind of setting, plus overrides.</summary>
internal static class SampleSnapshots
{
    public static SettingsSnapshot Customized()
    {
        var s = SettingsSnapshot.CreateDefaults();
        s.Global[Catalog.DebugLog.Key] = 1;
        s.Global[Catalog.ContextMenu.Key] = Values.ContextMenu.Expanded;
        s.Global[Catalog.BlockedFolders.Key] = new[] { @"C:\Secret", @"D:\Private folder" };
        s.Global[Catalog.CheckerLibrary.Key] = new[] { "FF000000;FFFFFFFF" };

        var psd = s.Formats["psd"];
        psd[Catalog.Mode.Key] = Values.Generation.Fallback;
        psd[Catalog.CanvasColor.Key] = unchecked((int)0xFF112233);
        psd[Catalog.Checker.Key] = "FF101010;FF202020";
        psd[Catalog.CustomIconPath.Key] = @"C:\Icons\my icon.ico,3";
        psd[Catalog.Alpha.Key] = Values.Alpha.Premultiplied;

        s.Formats["eps"][Catalog.Gamma.Key] = 220;

        s.Overrides[".psb"] = new Dictionary<string, object>(StringComparer.Ordinal)
        {
            [Catalog.Mode.Key] = Values.Generation.Disabled,
            [Catalog.CustomIconPath.Key] = "other.ico,1",
        };
        s.Overrides[".epsi"] = new Dictionary<string, object>(StringComparer.Ordinal)
        {
            [Catalog.Transparency.Key] = Values.Transparency.Opaque,
        };
        return s;
    }
}
