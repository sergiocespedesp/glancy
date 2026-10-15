using Glancy.Core.Resources;
using System.Text.Json;
using System.Text.Json.Nodes;
using Glancy.Core.Settings;

namespace Glancy.Core.Storage;

/// <summary>Import/export of the complete configuration as a small, human-readable JSON document.</summary>
public static class SettingsFile
{
    public const string Extension = ".glancysettings";
    public static string FileTypeName => CoreStrings.File_TypeName;
    private const string ProductTag = "Glancy";
    private const int FormatVersion = 1;

    public static string Serialize(SettingsSnapshot snapshot)
    {
        var root = new JsonObject
        {
            ["product"] = ProductTag,
            ["version"] = FormatVersion,
            ["exported"] = DateTimeOffset.Now.ToString("O"),
            ["global"] = ToJson(snapshot.Global),
        };
        var formats = new JsonObject();
        foreach (var (id, values) in snapshot.Formats) formats[id] = ToJson(values);
        root["formats"] = formats;
        var overrides = new JsonObject();
        foreach (var (ext, values) in snapshot.Overrides.Where(o => o.Value.Count > 0)) overrides[ext] = ToJson(values);
        root["overrides"] = overrides;
        return root.ToJsonString(new JsonSerializerOptions { WriteIndented = true });
    }

    /// <summary>
    /// Parses a settings document. Unknown keys are ignored and invalid values fall back to defaults, so files
    /// from newer or older versions import as much as they safely can. Returns the number of values ignored.
    /// </summary>
    public static SettingsSnapshot Deserialize(string json, out int ignoredValues)
    {
        ignoredValues = 0;
        JsonNode? root;
        try { root = JsonNode.Parse(json); }
        catch (JsonException ex) { throw new SettingsStoreException(CoreStrings.File_Invalid, ex); }

        if (root is not JsonObject obj
            || obj["product"] is not JsonValue product
            || !product.TryGetValue<string>(out var tag)
            || tag != ProductTag)
            throw new SettingsStoreException(CoreStrings.File_NotGlancy);

        var snap = SettingsSnapshot.CreateDefaults();
        if (obj["global"] is JsonObject g)
            ignoredValues += ReadInto(g, Catalog.GlobalSettings, snap.Global);

        if (obj["formats"] is JsonObject formats)
        {
            foreach (var (id, node) in formats)
            {
                var format = Catalog.FindFormat(id);
                if (format is null || node is not JsonObject fo) { ignoredValues++; continue; }
                ignoredValues += ReadInto(fo, Catalog.FormatSettings, snap.Formats[format.Id]);
            }
        }

        if (obj["overrides"] is JsonObject overrides)
        {
            foreach (var (ext, node) in overrides)
            {
                var format = Catalog.FormatForExtension(ext);
                if (format is null || node is not JsonObject oo) { ignoredValues++; continue; }
                var values = new Dictionary<string, object>(StringComparer.Ordinal);
                ignoredValues += ReadInto(oo, Catalog.FormatSettings, values);
                foreach (var key in values.Keys.ToList())
                    if (SettingsSnapshot.ValueEquals(values[key], snap.Formats[format.Id][key])) values.Remove(key);
                if (values.Count > 0) snap.Overrides[FormatDefinition.Normalize(ext)] = values;
            }
        }
        return snap;
    }

    private static JsonObject ToJson(Dictionary<string, object> values)
    {
        var o = new JsonObject();
        foreach (var (k, v) in values)
        {
            o[k] = v switch
            {
                int i => JsonValue.Create(i),
                string s => JsonValue.Create(s),
                string[] l => new JsonArray(l.Select(x => (JsonNode?)JsonValue.Create(x)).ToArray()),
                _ => null,
            };
        }
        return o;
    }

    private static int ReadInto(JsonObject source, IEnumerable<SettingDefinition> settings, Dictionary<string, object> target)
    {
        var ignored = 0;
        var known = settings.ToDictionary(s => s.Key);
        foreach (var (key, node) in source)
        {
            if (!known.TryGetValue(key, out var setting)) { ignored++; continue; }
            object? value = node switch
            {
                JsonValue v when v.TryGetValue<int>(out var i) => i,
                JsonValue v when v.TryGetValue<string>(out var s) => s,
                JsonArray a => ReadStringList(a),
                _ => null,
            };
            if (value is not null && setting.IsValid(value)) target[key] = value;
            else ignored++;
        }
        return ignored;
    }

    /// <summary>A list is accepted only if every item is a non-empty string; otherwise the whole value is ignored.</summary>
    private static string[]? ReadStringList(JsonArray array)
    {
        var items = new string[array.Count];
        for (var i = 0; i < array.Count; i++)
        {
            if (array[i] is not JsonValue v || !v.TryGetValue<string>(out var s) || string.IsNullOrWhiteSpace(s)) return null;
            items[i] = s;
        }
        return items;
    }
}
