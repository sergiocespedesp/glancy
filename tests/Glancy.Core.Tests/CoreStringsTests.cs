using System.Text.Json;
using System.Text.RegularExpressions;
using System.Xml.Linq;
using Glancy.Core.Localization;

namespace Glancy.Core.Tests;

/// <summary>Localization of the core: every catalog text has a key, and every translation matches the English base.</summary>
public partial class CoreStringsTests
{
    public static TheoryData<string> ResourceSets => new() { "src/Glancy.Core/Resources/CoreStrings", "src/Glancy.Settings/Resources/AppStrings" };

    [Fact]
    public void EverySettingOptionGroupCategoryAndFormat_HasItsText()
    {
        foreach (var s in Catalog.FormatSettings.Concat(Catalog.GlobalSettings))
        {
            Assert.False(string.IsNullOrWhiteSpace(s.Title), s.Key);
            Assert.False(string.IsNullOrWhiteSpace(s.Description), s.Key);
            Assert.False(string.IsNullOrWhiteSpace(s.Group), s.Key);
            foreach (var o in s.Options)
                Assert.False(string.IsNullOrWhiteSpace(o.Label), $"{s.Key} option {o.Value}");
        }
        foreach (var c in Catalog.Categories) Assert.NotNull(CoreText.Find($"Category_{c.Id}"));
        foreach (var f in Catalog.Formats) Assert.NotNull(CoreText.Find($"Format_{f.Id}_Description"));
    }

    [Fact]
    public void EnglishResources_MatchTheEnglishCatalog()
    {
        using var _ = new UICulture("en");
        foreach (var f in Catalog.Formats)
        {
            Assert.Equal(f.CatalogDescription, f.Description);
            Assert.Equal(f.CatalogName, f.DisplayName);
        }
        foreach (var c in Catalog.Categories) Assert.Equal(c.CatalogName, c.Name);
    }

    [Fact]
    public void Spanish_IsUsedWhenTheUiCultureIsSpanish()
    {
        using var _ = new UICulture("es");
        Assert.Equal("Activado", Catalog.Enabled.FormatValue(1));
        Assert.Equal("Fotografía", Catalog.FindCategory("photo")!.Name);
        Assert.Equal("Documento PDF", Catalog.Pdf.DisplayName);
        Assert.Equal("Adobe Photoshop", Catalog.Photoshop.DisplayName);
        Assert.Equal("Miniaturas: Glancy → Desactivado",
            new SettingChange("x", Catalog.Mode, Values.Generation.Enabled, Values.Generation.Disabled).Summary);
    }

    [Fact]
    public void UnsupportedUiCulture_FallsBackToEnglish()
    {
        using var _ = new UICulture("ko-KR");
        Assert.Equal("On", Catalog.Enabled.FormatValue(1));
    }

    [Theory]
    [MemberData(nameof(ResourceSets))]
    public void EveryTranslation_HasExactlyTheBaseKeys_NoEmptyValues_AndTheSamePlaceholders(string set)
    {
        var basePath = Repo.Path($"{set}.resx");
        if (!File.Exists(basePath)) return;   // AppStrings is created when the UI moves to resources.
        var english = ReadResx(basePath);
        Assert.NotEmpty(english);
        var translations = Directory.GetFiles(Path.GetDirectoryName(basePath)!, Path.GetFileName(set) + ".*.resx");
        foreach (var file in translations)
        {
            var language = Path.GetFileNameWithoutExtension(file).Split('.', 2)[1];
            Assert.Contains(language, AppLanguage.Supported.Select(l => l.Code));
            var translated = ReadResx(file);
            Assert.Equal(english.Keys.Order(), translated.Keys.Order());
            foreach (var (key, value) in translated)
            {
                // Separators may be a space or empty (Japanese and Chinese join sentences without a space).
                if (!key.EndsWith("Separator", StringComparison.Ordinal))
                    Assert.False(string.IsNullOrWhiteSpace(value), $"{language}: {key} is empty");
                Assert.Equal(Placeholders(english[key]), Placeholders(value));
            }
        }
    }

    private static Dictionary<string, string> ReadResx(string path) =>
        XDocument.Load(path).Root!.Elements("data")
            .ToDictionary(d => (string)d.Attribute("name")!, d => (string?)d.Element("value") ?? string.Empty);

    private static string Placeholders(string text) =>
        string.Join(",", PlaceholderRegex().Matches(text).Select(m => m.Value).Order());

    [GeneratedRegex(@"\{\d+(:[^}]*)?\}")]
    private static partial Regex PlaceholderRegex();
}
