using Glancy.Core.Resources;

namespace Glancy.Core.Localization;

/// <summary>Looks up Core strings by computed key (catalog settings, options, groups, categories, formats).</summary>
public static class CoreText
{
    public static string? Find(string key) => CoreStrings.ResourceManager.GetString(key, CoreStrings.Culture);

    public static string Get(string key) => Find(key) ?? throw new KeyNotFoundException($"Missing Core string '{key}'.");
}
