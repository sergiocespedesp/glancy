using Glancy.Core.Localization;

namespace Glancy.Core.Tests;

public class AppLanguageTests
{
    [Theory]
    [InlineData(null, new[] { "es-ES" }, "es")]
    [InlineData("", new[] { "es-MX", "en-US" }, "es")]
    [InlineData("auto", new[] { "pt-PT" }, "pt-BR")]
    [InlineData(null, new[] { "zh-CN" }, "zh-Hans")]
    [InlineData(null, new[] { "zh-Hans-SG" }, "zh-Hans")]
    [InlineData(null, new[] { "zh-TW", "fr-CA" }, "fr")]
    [InlineData(null, new[] { "ko-KR", "de-AT" }, "de")]
    [InlineData(null, new[] { "ko-KR" }, "en")]
    [InlineData(null, new string[0], "en")]
    [InlineData("fr", new[] { "es-ES" }, "fr")]
    [InlineData("xx", new[] { "ja-JP" }, "ja")]
    [InlineData("PT-br", new[] { "en-US" }, "pt-BR")]
    public void Resolve_PicksPreferenceThenWindowsThenEnglish(string? preference, string[] windows, string expected) =>
        Assert.Equal(expected, AppLanguage.Resolve(preference, windows));

    [Fact]
    public void Supported_HasSevenLanguagesWithNativeNames()
    {
        Assert.Equal(["en", "es", "de", "fr", "pt-BR", "ja", "zh-Hans"], AppLanguage.Supported.Select(l => l.Code));
        Assert.Equal("日本語", AppLanguage.Supported.Single(l => l.Code == "ja").NativeName);
        Assert.Equal("Español", AppLanguage.Supported.Single(l => l.Code == "es").NativeName);
    }

    [Theory]
    [InlineData("en", new[] { "es-MX" }, "en-US")]
    [InlineData("en", new[] { "es-MX", "en-GB" }, "en-GB")]
    [InlineData("es", new[] { "es-MX", "en-US" }, "es-MX")]
    [InlineData("es", new[] { "es-MX", "es-419", "en-US" }, "es-MX")]
    [InlineData("es", new[] { "es-AR" }, "es-ES")]
    [InlineData("es", new string[0], "es-ES")]
    [InlineData("de", new[] { "de-AT" }, "de-DE")]
    [InlineData("fr", new[] { "FR-ca" }, "fr-CA")]
    [InlineData("fr", new[] { "fr-CH", "fr-FR" }, "fr-FR")]
    [InlineData("pt-BR", new[] { "pt-PT" }, "pt-BR")]
    [InlineData("ja", new[] { "es-MX" }, "ja-JP")]
    [InlineData("zh-Hans", new[] { "zh-Hans-CN" }, "zh-CN")]
    public void WinUICulture_PrefersTheWindowsVariantThenTheDefault(string code, string[] windows, string expected) =>
        Assert.Equal(expected, AppLanguage.WinUICulture(code, windows));

    [Fact]
    public void WinUICultures_AreTheOnesTheAppShips()
    {
        var csproj = File.ReadAllText(Repo.Path("src", "Glancy.Settings", "Glancy.Settings.csproj"));
        var shipped = System.Text.RegularExpressions.Regex.Match(csproj, "<GlancyWinUILanguages>(.*)</GlancyWinUILanguages>")
            .Groups[1].Value.Split(';').Select(c => c.ToLowerInvariant()).Order();
        var mapped = AppLanguage.Supported.SelectMany(l => l.WinUICultures).Select(c => c.ToLowerInvariant()).Order();
        Assert.Equal(shipped, mapped);
    }

    [Fact]
    public void Preferences_LanguageRoundTrips_AndDefaultsToAuto()
    {
        using var key = new TestRegistryKey();
        var prefs = new Glancy.Core.Storage.AppPreferences(key.Path);
        Assert.Equal(string.Empty, prefs.Language);
        prefs.Language = "de";
        Assert.Equal("de", new Glancy.Core.Storage.AppPreferences(key.Path).Language);
        prefs.Language = string.Empty;
        Assert.Equal(string.Empty, prefs.Language);
    }
}
