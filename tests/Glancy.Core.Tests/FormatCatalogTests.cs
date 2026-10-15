using System.Diagnostics;
using Glancy.Core.Storage;

namespace Glancy.Core.Tests;

/// <summary>The data-driven catalog loaded from the embedded formats.json (shared with the engine).</summary>
public class FormatCatalogTests
{
    [Fact]
    public void EmbeddedJson_Loads()
    {
        var json = FormatCatalog.ReadResource();
        Assert.Contains("\"formats\"", json);
        var parsed = FormatCatalog.Parse(json);
        Assert.Equal(Catalog.Formats.Select(f => f.Id), parsed.Formats.Select(f => f.Id));
    }

    [Fact]
    public void Has87Formats_And265UniqueLowercaseExtensions()
    {
        Assert.Equal(87, Catalog.Formats.Count);
        Assert.Equal(87, Catalog.Formats.Select(f => f.Id).Distinct(StringComparer.OrdinalIgnoreCase).Count());

        var all = Catalog.AllExtensions;
        Assert.Equal(265, all.Count);
        Assert.Equal(265, all.Distinct(StringComparer.Ordinal).Count());
        foreach (var ext in all)
        {
            Assert.StartsWith(".", ext);
            Assert.Equal(ext.ToLowerInvariant(), ext);
            Assert.Equal(ext.Trim(), ext);
        }
    }

    [Fact]
    public void OriginalFormats_KeepTheirIdsAndOrder()
    {
        Assert.Equal(new[] { "psd", "ai", "eps", "pdf", "svg" }, Catalog.Formats.Take(5).Select(f => f.Id));
        Assert.Equal(new[] { "psd", "ai", "eps", "pdf", "svg" }, Catalog.QuickFormats.Select(f => f.Id));
        Assert.Same(Catalog.Photoshop, Catalog.FindFormat("psd"));
        Assert.Equal(new[] { ".psd", ".psb", ".pdd", ".psdt" }, Catalog.Photoshop.Extensions);
        Assert.Equal(new[] { ".eps", ".epsf", ".epsi", ".ps" }, Catalog.Eps.Extensions);
        Assert.Equal(new[] { ".svg", ".svgz" }, Catalog.Svg.Extensions);
    }

    [Fact]
    public void EveryCategoryReferenced_Exists_AndEveryCategoryHasFormats()
    {
        var ids = Catalog.Categories.Select(c => c.Id).ToHashSet();
        Assert.Equal(11, Catalog.Categories.Count);
        foreach (var f in Catalog.Formats) Assert.Contains(f.Category, ids);
        foreach (var c in Catalog.Categories)
        {
            Assert.False(string.IsNullOrWhiteSpace(c.Name));
            Assert.NotEmpty(Catalog.FormatsIn(c));
        }
        Assert.Equal(Catalog.Formats.Count, Catalog.Categories.Sum(c => Catalog.FormatsIn(c).Count()));
        Assert.Equal("Photography", Catalog.FindCategory("photo")!.Name);
        Assert.Equal("design", Catalog.Photoshop.Category);
    }

    [Fact]
    public void EveryFormat_HasPresentationData()
    {
        foreach (var f in Catalog.Formats)
        {
            Assert.False(string.IsNullOrWhiteSpace(f.DisplayName), f.Id);
            Assert.False(string.IsNullOrWhiteSpace(f.Description), f.Id);
            Assert.InRange(f.Monogram.Length, 1, 4);
            Assert.Equal(0xFFu, f.BadgeColor >> 24); // opaque badge
            Assert.NotEmpty(f.Extensions);
        }
    }

    [Fact]
    public void PerFormatDefaults_ComeFromJson()
    {
        Assert.Equal(Values.Generation.Fallback, Catalog.FindFormat("jpeg")!.DefaultFor(Catalog.Mode));
        Assert.Equal(Values.Generation.Fallback, Catalog.FindFormat("png")!.DefaultFor(Catalog.Mode));
        Assert.Equal(Values.Generation.Enabled, Catalog.Photoshop.DefaultFor(Catalog.Mode));
        Assert.Equal(Values.Generation.Enabled, Catalog.FindFormat("heif")!.DefaultFor(Catalog.Mode));

        Assert.Equal(Values.Transparency.Canvas, Catalog.Pdf.DefaultFor(Catalog.Transparency));
        Assert.Equal(Values.Transparency.Canvas, Catalog.Illustrator.DefaultFor(Catalog.Transparency));
        Assert.Equal(Values.Transparency.Transparent, Catalog.Svg.DefaultFor(Catalog.Transparency));

        var office = Catalog.FindFormat("officexml")!;
        Assert.Equal(Values.Generation.Fallback, office.DefaultFor(Catalog.Mode));
        Assert.Equal(Values.Transparency.Canvas, office.DefaultFor(Catalog.Transparency));

        // Settings without an override use the catalog default.
        Assert.Equal(Catalog.Sampling.DefaultValue, Catalog.FindFormat("jpeg")!.DefaultFor(Catalog.Sampling));
        foreach (var f in Catalog.Formats)
        {
            Assert.All(f.DefaultOverrides.Keys, k => Assert.Contains(k, new[] { Catalog.Mode.Key, Catalog.Transparency.Key }));
            foreach (var s in Catalog.FormatSettings) Assert.True(s.IsValid(f.DefaultFor(s)), $"{f.Id}.{s.Key}");
        }
    }

    [Fact]
    public void Defaults_FlowIntoSnapshotsAndAllFormatsReading()
    {
        var snap = SettingsSnapshot.CreateDefaults();
        Assert.Equal(87, snap.Formats.Count);
        Assert.Equal(Values.Generation.Fallback, snap.Formats["jpeg"][Catalog.Mode.Key]);
        Assert.Equal(Values.Transparency.Canvas, snap.Formats["pdf"][Catalog.Transparency.Key]);

        var session = new SettingsSession(new InMemorySettingsStore());
        var mode = session.Read(SettingsScope.AllFormats, Catalog.Mode);
        Assert.True(mode.IsMixed);
        Assert.False(session.IsModifiedFromDefault(SettingsScope.AllFormats, Catalog.Mode));
        Assert.False(session.IsModifiedFromDefault(SettingsScope.ForFormat(Catalog.FindFormat("jpeg")!), Catalog.Mode));
    }

    [Fact]
    public void ResetScope_RestoresPerFormatDefaults()
    {
        var session = new SettingsSession(new InMemorySettingsStore());
        session.Write(SettingsScope.AllFormats, Catalog.Mode, Values.Generation.Enabled);
        Assert.Equal(Values.Generation.Enabled, session.Working.Formats["jpeg"][Catalog.Mode.Key]);

        session.ResetScope(SettingsScope.AllFormats);
        Assert.Equal(Values.Generation.Fallback, session.Working.Formats["jpeg"][Catalog.Mode.Key]);
        Assert.Equal(Values.Generation.Enabled, session.Working.Formats["psd"][Catalog.Mode.Key]);
        Assert.False(session.IsDirty);

        session.Write(SettingsScope.ForFormat(Catalog.FindFormat("png")!), Catalog.Mode, Values.Generation.Disabled);
        session.ResetAll();
        Assert.Equal(Values.Generation.Fallback, session.Working.Formats["png"][Catalog.Mode.Key]);
        Assert.False(session.IsDirty);
    }

    [Fact]
    public void Import_MissingFormats_GetTheirOwnDefaults()
    {
        // A file exported before the catalog grew only has the five original formats.
        const string json = """{ "product": "Glancy", "version": 1, "formats": { "psd": { "mode": 0 } } }""";
        var snap = SettingsFile.Deserialize(json, out var ignored);
        Assert.Equal(0, ignored);
        Assert.Equal(Values.Generation.Disabled, snap.Formats["psd"][Catalog.Mode.Key]);
        Assert.Equal(Values.Generation.Fallback, snap.Formats["jpeg"][Catalog.Mode.Key]);
        Assert.Equal(Values.Transparency.Canvas, snap.Formats["pdf"][Catalog.Transparency.Key]);
    }

    [Theory]
    [InlineData("embedded", FormatCapability.EmbeddedThumbnail)]
    [InlineData("alpha", FormatCapability.PremultipliedAlpha)]
    [InlineData("sampling", FormatCapability.Sampling)]
    [InlineData("gamma", FormatCapability.Gamma)]
    [InlineData("pixelLimit", FormatCapability.PixelLimit)]
    public void Capabilities_MapFromJsonNames(string name, FormatCapability expected) =>
        Assert.Equal(expected, FormatCatalog.ParseCapability(name, "test"));

    [Fact]
    public void Capabilities_OfKnownFormats()
    {
        Assert.Equal(FormatCapability.EmbeddedThumbnail | FormatCapability.PremultipliedAlpha | FormatCapability.Sampling
                     | FormatCapability.Gamma | FormatCapability.PixelLimit, Catalog.Photoshop.Capabilities);
        Assert.Equal(FormatCapability.EmbeddedThumbnail | FormatCapability.Gamma, Catalog.Illustrator.Capabilities);
        Assert.Equal(FormatCapability.Gamma, Catalog.Pdf.Capabilities);
        Assert.Equal(FormatCapability.None, Catalog.Svg.Capabilities);
        Assert.Equal(FormatCapability.None, Catalog.FindFormat("blender")!.Capabilities);
        Assert.Equal(FormatCapability.Sampling | FormatCapability.PixelLimit, Catalog.FindFormat("gif")!.Capabilities);
        Assert.Throws<InvalidDataException>(() => FormatCatalog.ParseCapability("bogus", "test"));
    }

    [Theory]
    [InlineData(".psd", "psd")]
    [InlineData(".PSDT", "psd")]
    [InlineData("ps", "eps")]
    [InlineData(".jpg", "jpeg")]
    [InlineData(".jpeg", "jpeg")]
    [InlineData(".heic", "heif")]
    [InlineData(".avif", "heif")]
    [InlineData(".cr2", "raw")]
    [InlineData(".nef", "raw")]
    [InlineData(".dng", "raw")]
    [InlineData(".png", "png")]
    [InlineData(".tif", "tiff")]
    [InlineData(".blend", "blender")]
    [InlineData(".blend1", "blender")]
    [InlineData(".docx", "officexml")]
    [InlineData(".epub", "epub")]
    [InlineData(".exr", "exr")]
    [InlineData(".dds", "dds")]
    [InlineData(".wmf", "metafile")]
    [InlineData(".zip", "archive")]
    public void FormatForExtension_FindsOwner(string extension, string formatId)
    {
        var format = Catalog.FormatForExtension(extension);
        Assert.NotNull(format);
        Assert.Equal(formatId, format.Id);
        Assert.True(format.Owns(extension));
    }

    [Fact]
    public void LargeFormats_AreInTheCatalog()
    {
        Assert.Equal(33, Catalog.FindFormat("blender")!.Extensions.Count);
        Assert.Equal(35, Catalog.FindFormat("raw")!.Extensions.Count);
    }

    [Fact]
    public void Parse_RejectsInvalidCatalogs()
    {
        const string ok = """{ "id": "a", "name": "A", "monogram": "A", "color": "FF000000", "category": "c", "description": "d", "extensions": [".a"] }""";
        string Doc(string format) => $$"""{ "categories": [ { "id": "c", "name": "C" } ], "formats": [ {{format}} ] }""";

        Assert.Single(FormatCatalog.Parse(Doc(ok)).Formats);
        Assert.Throws<InvalidDataException>(() => FormatCatalog.Parse(Doc(ok.Replace("\"c\"", "\"zzz\""))));            // unknown category
        Assert.Throws<InvalidDataException>(() => FormatCatalog.Parse(Doc(ok.Replace("\".a\"", "\".A\""))));             // upper case
        Assert.Throws<InvalidDataException>(() => FormatCatalog.Parse(Doc(ok.Replace("\".a\"", "\"a\""))));              // no dot
        Assert.Throws<InvalidDataException>(() => FormatCatalog.Parse(Doc(ok + "," + ok)));                               // duplicate id
        Assert.Throws<InvalidDataException>(() => FormatCatalog.Parse(Doc(ok + "," + ok.Replace("\"a\"", "\"b\"")))); // duplicate extension
        Assert.Throws<InvalidDataException>(() => FormatCatalog.Parse(Doc(ok.Replace("FF000000", "red"))));              // bad color
        Assert.Throws<InvalidDataException>(() => FormatCatalog.Parse(Doc(ok.Replace("[\".a\"]", "[\".a\"], \"defaults\": { \"mode\": 7 }"))));
        Assert.Throws<InvalidDataException>(() => FormatCatalog.Parse(Doc(ok.Replace("[\".a\"]", "[\".a\"], \"defaults\": { \"bogus\": 1 }"))));
        Assert.Throws<InvalidDataException>(() => FormatCatalog.Parse(Doc(ok.Replace("[\".a\"]", "[\".a\"], \"capabilities\": [\"x\"]"))));
    }
}

/// <summary>With 87 formats the registry round trip must stay fast: one key per format/extension, not per value.</summary>
[Collection("Registry")]
public sealed class RegistryPerformanceTests(ITestOutputHelper output) : IDisposable
{
    private readonly TestRegistryKey _key = new();

    public void Dispose() => _key.Dispose();

    [Fact]
    public void SaveAndLoad_AllFormats_AreFast()
    {
        var store = new RegistrySettingsStore(_key.Path);
        var snap = SampleSnapshots.Customized();
        store.Save(snap); // first save creates every key
        store.Load();

        const int runs = 5;
        var save = Stopwatch.StartNew();
        for (var i = 0; i < runs; i++) store.Save(snap);
        save.Stop();
        var load = Stopwatch.StartNew();
        SettingsSnapshot? loaded = null;
        for (var i = 0; i < runs; i++) loaded = store.Load();
        load.Stop();

        var saveMs = save.Elapsed.TotalMilliseconds / runs;
        var loadMs = load.Elapsed.TotalMilliseconds / runs;
        output.WriteLine($"Save {saveMs:F1} ms · Load {loadMs:F1} ms ({Catalog.Formats.Count} formats, {Catalog.AllExtensions.Count} extensions)");
        SnapshotAssert.Equal(snap, loaded!);
        Assert.True(saveMs < 200, $"Save took {saveMs:F1} ms");
        Assert.True(loadMs < 200, $"Load took {loadMs:F1} ms");
    }
}

/// <summary>The engine status reads the shell registration of every catalog extension (read-only).</summary>
public sealed class EngineStatusPerformanceTests(ITestOutputHelper output)
{
    [Fact]
    public async Task GetStatus_CoversEveryExtension_OffTheUiThread()
    {
        var sw = Stopwatch.StartNew();
        var status = await Glancy.Core.Shell.EngineStatusService.GetStatusAsync();
        sw.Stop();
        output.WriteLine($"Engine status: {sw.Elapsed.TotalMilliseconds:F0} ms for {status.Handlers.Count} extensions");
        Assert.Equal(Catalog.AllExtensions, status.Handlers.Select(h => h.Extension));
        Assert.Equal(".psd", status.HandlerFor(".PSD").Extension);
        Assert.Equal(Glancy.Core.Shell.HandlerOwner.None, status.HandlerFor(".xyz").Owner);
        Assert.True(sw.Elapsed < TimeSpan.FromSeconds(5));
    }
}
