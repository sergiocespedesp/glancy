using System.Globalization;

namespace Glancy.Core.Localization;

/// <summary>The UI languages Glancy ships, and how the active one is chosen (preference, then Windows, then English).</summary>
/// <param name="WinUICultures">
/// The cultures whose WinUI <c>.mui</c> files the app ships for this language (<c>GlancyWinUILanguages</c> in
/// Glancy.Settings.csproj), the default first.
/// </param>
public sealed record AppLanguage(string Code, string NativeName, IReadOnlyList<string> WinUICultures)
{
    public static IReadOnlyList<AppLanguage> Supported { get; } =
    [
        new("en", "English", ["en-US", "en-GB"]), new("es", "Español", ["es-ES", "es-MX"]),
        new("de", "Deutsch", ["de-DE"]), new("fr", "Français", ["fr-FR", "fr-CA"]),
        new("pt-BR", "Português (Brasil)", ["pt-BR"]), new("ja", "日本語", ["ja-JP"]),
        new("zh-Hans", "简体中文", ["zh-CN"]),
    ];

    /// <summary>The supported code for a language tag, or null (es-MX → es, pt-PT → pt-BR, zh-CN → zh-Hans).</summary>
    public static string? Match(string? tag)
    {
        if (string.IsNullOrWhiteSpace(tag) || tag.Equals("auto", StringComparison.OrdinalIgnoreCase)) return null;
        var exact = Supported.FirstOrDefault(l => l.Code.Equals(tag, StringComparison.OrdinalIgnoreCase));
        if (exact is not null) return exact.Code;
        var language = tag.Split('-')[0].ToLowerInvariant();
        return language switch
        {
            "pt" => "pt-BR",
            "zh" => IsTraditional(tag) ? null : "zh-Hans",
            _ => Supported.FirstOrDefault(l => l.Code == language)?.Code,
        };
    }

    public static string Resolve(string? preference, IEnumerable<string> windowsLanguages) =>
        Match(preference) ?? windowsLanguages.Select(Match).FirstOrDefault(c => c is not null) ?? "en";

    /// <summary>
    /// The culture WinUI should load its <c>.mui</c> strings in (text box menu, control type names) for a supported
    /// code: the first of Windows' languages (display language first, then the preferred ones) that is a shipped
    /// variant (es-MX for Spanish on a Latin American Windows), else the default variant (es-ES).
    /// </summary>
    public static string WinUICulture(string code, IEnumerable<string> windowsLanguages)
    {
        var cultures = Supported.Single(l => l.Code == code).WinUICultures;
        return windowsLanguages
            .Select(tag => cultures.FirstOrDefault(c => c.Equals(tag, StringComparison.OrdinalIgnoreCase)))
            .FirstOrDefault(c => c is not null) ?? cultures[0];
    }

    /// <summary>Sets the UI culture for .NET resources on every thread. Number and date formats are left alone.</summary>
    public static void Apply(string code)
    {
        var culture = CultureInfo.GetCultureInfo(code);
        CultureInfo.DefaultThreadCurrentUICulture = culture;
        CultureInfo.CurrentUICulture = culture;
    }

    private static bool IsTraditional(string tag) =>
        tag.Contains("Hant", StringComparison.OrdinalIgnoreCase)
        || tag.EndsWith("-TW", StringComparison.OrdinalIgnoreCase)
        || tag.EndsWith("-HK", StringComparison.OrdinalIgnoreCase)
        || tag.EndsWith("-MO", StringComparison.OrdinalIgnoreCase);
}
