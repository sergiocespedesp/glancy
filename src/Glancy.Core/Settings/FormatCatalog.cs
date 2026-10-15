using System.Globalization;
using System.Text.Json;

namespace Glancy.Core.Settings;

/// <summary>A group of formats shown together in the navigation and on the home page ("Photography").</summary>
public sealed record FormatCategory(string Id, string CatalogName)
{
    /// <summary>Name in the UI language (CoreStrings Category_&lt;id&gt;), falling back to the English catalog name.</summary>
    public string Name => Glancy.Core.Localization.CoreText.Find($"Category_{Id}") ?? CatalogName;
}

/// <summary>
/// Loads the embedded <c>formats.json</c> once and indexes it. The file is shared with the thumbnail engine, so
/// it is validated strictly: a malformed catalog is a build defect and fails loudly (and in the tests).
/// </summary>
internal sealed class FormatCatalog
{
    public const string ResourceName = "Glancy.Core.Settings.formats.json";

    private static readonly Lazy<FormatCatalog> Lazy = new(() => Parse(ReadResource()));

    public static FormatCatalog Instance => Lazy.Value;

    private FormatCatalog(IReadOnlyList<FormatCategory> categories, IReadOnlyList<FormatDefinition> formats)
    {
        Categories = categories;
        Formats = formats;
        ById = formats.ToDictionary(f => f.Id, StringComparer.OrdinalIgnoreCase);
        var byExtension = new Dictionary<string, FormatDefinition>(StringComparer.OrdinalIgnoreCase);
        var all = new List<string>();
        foreach (var f in formats)
        {
            foreach (var ext in f.Extensions)
            {
                if (!byExtension.TryAdd(ext, f))
                    throw new InvalidDataException($"formats.json: extension {ext} appears in {byExtension[ext].Id} and in {f.Id}.");
                all.Add(ext);
            }
        }
        ByExtension = byExtension;
        AllExtensions = all;
    }

    public IReadOnlyList<FormatCategory> Categories { get; }
    public IReadOnlyList<FormatDefinition> Formats { get; }
    public IReadOnlyDictionary<string, FormatDefinition> ById { get; }
    public IReadOnlyDictionary<string, FormatDefinition> ByExtension { get; }
    public IReadOnlyList<string> AllExtensions { get; }

    public FormatDefinition Require(string id) =>
        ById.TryGetValue(id, out var f) ? f : throw new InvalidDataException($"formats.json: format '{id}' is missing.");

    internal static string ReadResource()
    {
        using var stream = typeof(FormatCatalog).Assembly.GetManifestResourceStream(ResourceName)
                           ?? throw new InvalidDataException($"Embedded resource {ResourceName} is missing.");
        using var reader = new StreamReader(stream);
        return reader.ReadToEnd();
    }

    /// <summary>Parses and validates a catalog document. Throws <see cref="InvalidDataException"/> on any defect.</summary>
    internal static FormatCatalog Parse(string json)
    {
        using var doc = JsonDocument.Parse(json, new JsonDocumentOptions { CommentHandling = JsonCommentHandling.Skip });
        var root = doc.RootElement;

        var categories = new List<FormatCategory>();
        foreach (var c in root.GetProperty("categories").EnumerateArray())
        {
            var category = new FormatCategory(RequiredString(c, "id"), RequiredString(c, "name"));
            if (categories.Any(x => x.Id == category.Id))
                throw new InvalidDataException($"formats.json: duplicate category '{category.Id}'.");
            categories.Add(category);
        }

        var formats = new List<FormatDefinition>();
        foreach (var f in root.GetProperty("formats").EnumerateArray())
        {
            var id = RequiredString(f, "id");
            if (formats.Any(x => string.Equals(x.Id, id, StringComparison.OrdinalIgnoreCase)))
                throw new InvalidDataException($"formats.json: duplicate format '{id}'.");

            var category = RequiredString(f, "category");
            if (categories.All(c => c.Id != category))
                throw new InvalidDataException($"formats.json: {id} uses unknown category '{category}'.");

            var color = RequiredString(f, "color");
            if (color.Length != 8 || !uint.TryParse(color, NumberStyles.HexNumber, CultureInfo.InvariantCulture, out var argb))
                throw new InvalidDataException($"formats.json: {id} has an invalid color '{color}' (expected AARRGGBB).");

            var extensions = new List<string>();
            foreach (var e in f.GetProperty("extensions").EnumerateArray())
            {
                var ext = e.GetString() ?? string.Empty;
                if (ext.Length < 2 || ext[0] != '.' || ext != ext.Trim().ToLowerInvariant() || extensions.Contains(ext))
                    throw new InvalidDataException($"formats.json: {id} has an invalid or repeated extension '{ext}'.");
                extensions.Add(ext);
            }
            if (extensions.Count == 0) throw new InvalidDataException($"formats.json: {id} has no extensions.");

            var capabilities = FormatCapability.None;
            if (f.TryGetProperty("capabilities", out var caps))
            {
                foreach (var cap in caps.EnumerateArray())
                    capabilities |= ParseCapability(cap.GetString(), id);
            }

            var defaults = new Dictionary<string, object>(StringComparer.Ordinal);
            if (f.TryGetProperty("defaults", out var defs))
            {
                foreach (var d in defs.EnumerateObject())
                {
                    var setting = Catalog.FormatSettings.FirstOrDefault(s => s.Key == d.Name)
                                  ?? throw new InvalidDataException($"formats.json: {id} defines an unknown default '{d.Name}'.");
                    object? value = d.Value.ValueKind switch
                    {
                        JsonValueKind.Number when d.Value.TryGetInt32(out var i) => i,
                        JsonValueKind.String => d.Value.GetString(),
                        _ => null,
                    };
                    if (value is null || !setting.IsValid(value))
                        throw new InvalidDataException($"formats.json: {id} has an invalid value for '{d.Name}'.");
                    defaults[setting.Key] = value;
                }
            }

            formats.Add(new FormatDefinition
            {
                Id = id,
                CatalogName = RequiredString(f, "name"),
                Monogram = RequiredString(f, "monogram"),
                CatalogDescription = RequiredString(f, "description"),
                Category = category,
                Extensions = extensions,
                Capabilities = capabilities,
                BadgeColor = argb,
                DefaultOverrides = defaults,
            });
        }

        // Empty categories would show as empty navigation groups.
        categories.RemoveAll(c => formats.All(f => f.Category != c.Id));
        return new FormatCatalog(categories, formats);
    }

    internal static FormatCapability ParseCapability(string? name, string formatId) => name switch
    {
        "embedded" => FormatCapability.EmbeddedThumbnail,
        "alpha" => FormatCapability.PremultipliedAlpha,
        "sampling" => FormatCapability.Sampling,
        "gamma" => FormatCapability.Gamma,
        "pixelLimit" => FormatCapability.PixelLimit,
        _ => throw new InvalidDataException($"formats.json: {formatId} has an unknown capability '{name}'."),
    };

    private static string RequiredString(JsonElement element, string property)
    {
        if (element.TryGetProperty(property, out var p) && p.ValueKind == JsonValueKind.String && p.GetString() is { Length: > 0 } s)
            return s;
        throw new InvalidDataException($"formats.json: '{property}' is missing in {element.GetRawText()[..Math.Min(60, element.GetRawText().Length)]}…");
    }
}
