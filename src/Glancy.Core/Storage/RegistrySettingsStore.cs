using Glancy.Core.Resources;
using Glancy.Core.Settings;
using Microsoft.Win32;

namespace Glancy.Core.Storage;

/// <summary>
/// Persists settings under HKCU using the layout documented in docs/registry-contract.md:
/// <c>Global</c>, <c>Formats\{id}</c>, <c>Formats\{id}\Extensions\{.ext}</c> and a <c>Revision</c> counter
/// the engine can poll to know when to reload.
/// </summary>
public sealed class RegistrySettingsStore(string keyPath = AppInfo.SettingsKeyPath) : ISettingsStore
{
    public const string GlobalKey = "Global";
    public const string FormatsKey = "Formats";
    public const string ExtensionsKey = "Extensions";
    public const string RevisionValue = "Revision";
    public const string SchemaValue = "Schema";
    public const int SchemaVersion = 1;

    public string KeyPath { get; } = keyPath;

    public SettingsSnapshot Load()
    {
        var snap = SettingsSnapshot.CreateDefaults();
        try
        {
            using var root = Registry.CurrentUser.OpenSubKey(KeyPath);
            if (root is null) return snap;

            using (var g = root.OpenSubKey(GlobalKey))
            {
                if (g is not null)
                    foreach (var s in Catalog.GlobalSettings)
                        if (TryRead(g, s, out var v)) snap.Global[s.Key] = v;
            }

            foreach (var f in Catalog.Formats)
            {
                using var fk = root.OpenSubKey($@"{FormatsKey}\{f.Id}");
                if (fk is null) continue;
                foreach (var s in Catalog.FormatSettings)
                    if (TryRead(fk, s, out var v)) snap.Formats[f.Id][s.Key] = v;

                using var ek = fk.OpenSubKey(ExtensionsKey);
                if (ek is null) continue;
                foreach (var ext in f.Extensions)
                {
                    using var xk = ek.OpenSubKey(ext);
                    if (xk is null) continue;
                    var o = new Dictionary<string, object>(StringComparer.Ordinal);
                    foreach (var s in Catalog.FormatSettings)
                        if (TryRead(xk, s, out var v) && !SettingsSnapshot.ValueEquals(v, snap.Formats[f.Id][s.Key]))
                            o[s.Key] = v;
                    if (o.Count > 0) snap.Overrides[ext] = o;
                }
            }
            return snap;
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or System.Security.SecurityException or IOException)
        {
            throw new SettingsStoreException(CoreStrings.Store_ReadFailed, ex);
        }
    }

    public void Save(SettingsSnapshot snapshot)
    {
        try
        {
            using var root = Registry.CurrentUser.CreateSubKey(KeyPath, writable: true);
            root.SetValue(SchemaValue, SchemaVersion, RegistryValueKind.DWord);

            using (var g = root.CreateSubKey(GlobalKey, writable: true))
                foreach (var s in Catalog.GlobalSettings) Write(g, s.Key, snapshot.Global[s.Key]);

            foreach (var f in Catalog.Formats)
            {
                using var fk = root.CreateSubKey($@"{FormatsKey}\{f.Id}", writable: true);
                foreach (var s in Catalog.FormatSettings) Write(fk, s.Key, snapshot.Formats[f.Id][s.Key]);

                // Extension keys hold only overrides. Update them in place (never delete-and-recreate), so an
                // engine reading concurrently never observes a moment with all overrides missing. Most formats
                // have none: only open (or create) the Extensions key when there is something to write or clean.
                var hasOverrides = f.Extensions.Any(e => snapshot.Overrides.TryGetValue(e, out var o) && o.Count > 0);
                using var ek = hasOverrides ? fk.CreateSubKey(ExtensionsKey, writable: true) : fk.OpenSubKey(ExtensionsKey, writable: true);
                if (ek is null) continue;
                foreach (var existing in ek.GetSubKeyNames())
                {
                    if (!snapshot.Overrides.TryGetValue(existing, out var keep) || keep.Count == 0 || !f.Owns(existing))
                        ek.DeleteSubKeyTree(existing, throwOnMissingSubKey: false);
                }
                foreach (var ext in f.Extensions)
                {
                    if (!snapshot.Overrides.TryGetValue(ext, out var o) || o.Count == 0) continue;
                    using var xk = ek.CreateSubKey(ext, writable: true);
                    foreach (var stale in xk.GetValueNames().Where(n => !o.ContainsKey(n))) xk.DeleteValue(stale, throwOnMissingValue: false);
                    foreach (var (k, v) in o) Write(xk, k, v);
                }
            }

            // Bumped last: the engine reloads when it sees a new revision, so everything above must be complete.
            var revision = root.GetValue(RevisionValue) is int r ? r : 0;
            root.SetValue(RevisionValue, unchecked(revision + 1), RegistryValueKind.DWord);
        }
        catch (Exception ex) when (ex is not SettingsStoreException)
        {
            throw new SettingsStoreException(CoreStrings.Store_SaveFailed, ex);
        }
    }

    public int ReadRevision()
    {
        using var root = Registry.CurrentUser.OpenSubKey(KeyPath);
        return root?.GetValue(RevisionValue) is int r ? r : 0;
    }

    private static bool TryRead(RegistryKey key, SettingDefinition setting, out object value)
    {
        value = null!;
        var raw = key.GetValue(setting.Key);
        object? candidate = raw switch
        {
            int i => i,
            string s when setting.Kind is SettingKind.Text or SettingKind.Checker => s,
            string[] l when setting.Kind == SettingKind.List => l,
            _ => null,
        };
        if (candidate is null || !setting.IsValid(candidate)) return false;
        value = candidate;
        return true;
    }

    private static void Write(RegistryKey key, string name, object value)
    {
        switch (value)
        {
            case int i: key.SetValue(name, i, RegistryValueKind.DWord); break;
            case string s: key.SetValue(name, s, RegistryValueKind.String); break;
            case string[] l: key.SetValue(name, l, RegistryValueKind.MultiString); break;
            default: throw new InvalidOperationException($"Unsupported value type for {name}: {value.GetType()}");
        }
    }
}
