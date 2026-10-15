namespace Glancy.Core.Settings;

/// <summary>
/// A complete, self-contained copy of every setting. Format dictionaries are always complete;
/// extension override dictionaries only contain the keys that differ from their format.
/// </summary>
public sealed class SettingsSnapshot
{
    public Dictionary<string, object> Global { get; } = new(StringComparer.Ordinal);

    /// <summary>Format id → complete value set.</summary>
    public Dictionary<string, Dictionary<string, object>> Formats { get; } = new(StringComparer.OrdinalIgnoreCase);

    /// <summary>Extension (".psb") → partial value set overriding its format.</summary>
    public Dictionary<string, Dictionary<string, object>> Overrides { get; } = new(StringComparer.OrdinalIgnoreCase);

    public static SettingsSnapshot CreateDefaults()
    {
        var s = new SettingsSnapshot();
        foreach (var g in Catalog.GlobalSettings) s.Global[g.Key] = CloneValue(g.DefaultValue);
        foreach (var f in Catalog.Formats)
        {
            var values = new Dictionary<string, object>(StringComparer.Ordinal);
            foreach (var setting in Catalog.FormatSettings) values[setting.Key] = CloneValue(f.DefaultFor(setting));
            s.Formats[f.Id] = values;
        }
        return s;
    }

    public SettingsSnapshot Clone()
    {
        var c = new SettingsSnapshot();
        foreach (var (k, v) in Global) c.Global[k] = CloneValue(v);
        foreach (var (id, values) in Formats) c.Formats[id] = CloneDictionary(values);
        foreach (var (ext, values) in Overrides) c.Overrides[ext] = CloneDictionary(values);
        return c;
    }

    /// <summary>The effective value for an extension: its override if present, otherwise its format's value.</summary>
    public object EffectiveValue(FormatDefinition format, string extension, string key) =>
        Overrides.TryGetValue(extension, out var o) && o.TryGetValue(key, out var v) ? v : Formats[format.Id][key];

    /// <summary>Deep comparison of every value, format and override.</summary>
    public bool ContentEquals(SettingsSnapshot other) =>
        DictionaryEquals(Global, other.Global)
        && Formats.Count == other.Formats.Count
        && Formats.All(f => other.Formats.TryGetValue(f.Key, out var o) && DictionaryEquals(f.Value, o))
        && Overrides.Count == other.Overrides.Count
        && Overrides.All(x => other.Overrides.TryGetValue(x.Key, out var o) && DictionaryEquals(x.Value, o));

    private static bool DictionaryEquals(Dictionary<string, object> a, Dictionary<string, object> b) =>
        a.Count == b.Count && a.All(kv => b.TryGetValue(kv.Key, out var v) && ValueEquals(kv.Value, v));

    public static bool ValueEquals(object? a, object? b) => (a, b) switch
    {
        (null, null) => true,
        (string[] x, string[] y) => x.SequenceEqual(y, StringComparer.Ordinal),
        _ => Equals(a, b),
    };

    public static object CloneValue(object value) => value is string[] arr ? arr.ToArray() : value;

    private static Dictionary<string, object> CloneDictionary(Dictionary<string, object> source)
    {
        var d = new Dictionary<string, object>(StringComparer.Ordinal);
        foreach (var (k, v) in source) d[k] = CloneValue(v);
        return d;
    }
}
