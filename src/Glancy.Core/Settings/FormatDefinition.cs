using Glancy.Core.Localization;

namespace Glancy.Core.Settings;

/// <summary>A file format Glancy can render, with the extensions it owns and its per-format defaults.</summary>
public sealed class FormatDefinition
{
    public required string Id { get; init; }

    /// <summary>English name from formats.json; brand names ("Adobe Photoshop") are never translated.</summary>
    public required string CatalogName { get; init; }

    /// <summary>English description from formats.json.</summary>
    public required string CatalogDescription { get; init; }

    /// <summary>Name in the UI language: CoreStrings Format_&lt;id&gt;_Name when the name is translatable.</summary>
    public string DisplayName => CoreText.Find($"Format_{Id}_Name") ?? CatalogName;

    public string Description => CoreText.Find($"Format_{Id}_Description") ?? CatalogDescription;

    /// <summary>Two-letter monogram used for the format badge (e.g. "Ps").</summary>
    public required string Monogram { get; init; }

    /// <summary>Id of the <see cref="FormatCategory"/> the format is listed under.</summary>
    public string Category { get; init; } = "other";

    /// <summary>Lower-case extensions with the dot, in catalog order (the first one names sample files).</summary>
    public required IReadOnlyList<string> Extensions { get; init; }
    public required FormatCapability Capabilities { get; init; }

    /// <summary>Badge color, ARGB.</summary>
    public required uint BadgeColor { get; init; }

    /// <summary>Per-format default values that differ from the catalog defaults.</summary>
    public IReadOnlyDictionary<string, object> DefaultOverrides { get; init; } = new Dictionary<string, object>();

    public object DefaultFor(SettingDefinition setting) =>
        DefaultOverrides.TryGetValue(setting.Key, out var v) ? v : setting.DefaultValue;

    public bool Owns(string extension) =>
        Extensions.Contains(Normalize(extension), StringComparer.Ordinal);

    public static string Normalize(string extension)
    {
        var e = extension.Trim().ToLowerInvariant();
        return e.StartsWith('.') ? e : "." + e;
    }
}
