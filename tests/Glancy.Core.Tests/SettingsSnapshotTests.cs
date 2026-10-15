namespace Glancy.Core.Tests;

public class SettingsSnapshotTests
{
    [Fact]
    public void CreateDefaults_EveryFormatHasEveryFormatSetting()
    {
        var s = SettingsSnapshot.CreateDefaults();
        Assert.Equal(Catalog.Formats.Count, s.Formats.Count);
        foreach (var f in Catalog.Formats)
        {
            Assert.True(s.Formats.ContainsKey(f.Id), f.Id);
            var values = s.Formats[f.Id];
            Assert.Equal(Catalog.FormatSettings.Count, values.Count);
            foreach (var setting in Catalog.FormatSettings)
                Assert.True(values.ContainsKey(setting.Key), $"{f.Id} lacks {setting.Key}");
        }
    }

    [Fact]
    public void CreateDefaults_GlobalsPresentWithDefaults_AndNoOverrides()
    {
        var s = SettingsSnapshot.CreateDefaults();
        Assert.Equal(Catalog.GlobalSettings.Count, s.Global.Count);
        foreach (var g in Catalog.GlobalSettings)
            Assert.True(SettingsSnapshot.ValueEquals(g.DefaultValue, s.Global[g.Key]), g.Key);
        Assert.Empty(s.Overrides);
    }

    [Theory]
    [InlineData("ai")]
    [InlineData("eps")]
    [InlineData("pdf")]
    public void CreateDefaults_AppliesPerFormatOverride_TransparencyCanvas(string id)
    {
        var s = SettingsSnapshot.CreateDefaults();
        Assert.Equal(Values.Transparency.Canvas, s.Formats[id][Catalog.Transparency.Key]);
    }

    [Theory]
    [InlineData("psd")]
    [InlineData("svg")]
    public void CreateDefaults_FormatsWithoutOverride_UseCatalogDefault(string id)
    {
        var s = SettingsSnapshot.CreateDefaults();
        Assert.Equal(Catalog.Transparency.DefaultValue, s.Formats[id][Catalog.Transparency.Key]);
        Assert.Equal(Values.Transparency.Transparent, s.Formats[id][Catalog.Transparency.Key]);
    }

    [Fact]
    public void CreateDefaults_ListValues_AreCopiesOfCatalogArrays()
    {
        var s = SettingsSnapshot.CreateDefaults();
        var lib = (string[])s.Global[Catalog.CheckerLibrary.Key];
        Assert.NotSame(Catalog.DefaultCheckerLibrary, lib);
        lib[0] = "mutated";
        Assert.Equal("FFFFFFFF;FFCCCCCC", Catalog.DefaultCheckerLibrary[0]);
    }

    [Fact]
    public void Clone_IsDeep()
    {
        var original = SampleSnapshots.Customized();
        var clone = original.Clone();
        SnapshotAssert.Equal(original, clone);

        var cloneList = (string[])clone.Global[Catalog.BlockedFolders.Key];
        Assert.NotSame(original.Global[Catalog.BlockedFolders.Key], cloneList);
        cloneList[0] = "changed";
        Assert.Equal(@"C:\Secret", ((string[])original.Global[Catalog.BlockedFolders.Key])[0]);

        clone.Formats["psd"][Catalog.Mode.Key] = Values.Generation.Disabled;
        Assert.Equal(Values.Generation.Fallback, original.Formats["psd"][Catalog.Mode.Key]);

        clone.Overrides[".psb"].Remove(Catalog.Mode.Key);
        clone.Overrides.Remove(".epsi");
        Assert.True(original.Overrides[".psb"].ContainsKey(Catalog.Mode.Key));
        Assert.True(original.Overrides.ContainsKey(".epsi"));

        clone.Global[Catalog.DebugLog.Key] = 0;
        Assert.Equal(1, original.Global[Catalog.DebugLog.Key]);
    }

    [Fact]
    public void EffectiveValue_PrefersOverride()
    {
        var s = SampleSnapshots.Customized();
        Assert.Equal(Values.Generation.Disabled, s.EffectiveValue(Catalog.Photoshop, ".psb", Catalog.Mode.Key));
        Assert.Equal(Values.Generation.Fallback, s.EffectiveValue(Catalog.Photoshop, ".psd", Catalog.Mode.Key));
        // Override dictionary is case-insensitive on extension.
        Assert.Equal(Values.Generation.Disabled, s.EffectiveValue(Catalog.Photoshop, ".PSB", Catalog.Mode.Key));
    }

    [Fact]
    public void ValueEquals_HandlesArraysNullsAndBoxedInts()
    {
        Assert.True(SettingsSnapshot.ValueEquals(null, null));
        Assert.False(SettingsSnapshot.ValueEquals(null, 0));
        Assert.True(SettingsSnapshot.ValueEquals(1, 1));
        Assert.False(SettingsSnapshot.ValueEquals(1, "1"));
        Assert.True(SettingsSnapshot.ValueEquals(new[] { "a", "b" }, new[] { "a", "b" }));
        Assert.False(SettingsSnapshot.ValueEquals(new[] { "a", "b" }, new[] { "b", "a" }));
        Assert.False(SettingsSnapshot.ValueEquals(new[] { "a" }, new[] { "A" }));
        Assert.True(SettingsSnapshot.ValueEquals(Array.Empty<string>(), Array.Empty<string>()));
    }
}

public class CatalogTests
{
    [Fact]
    public void SettingKeys_AreUnique()
    {
        var keys = Catalog.FormatSettings.Concat(Catalog.GlobalSettings).Select(s => s.Key).ToList();
        Assert.Equal(keys.Count, keys.Distinct(StringComparer.Ordinal).Count());
    }

    [Fact]
    public void ScopesMatchLists()
    {
        Assert.All(Catalog.FormatSettings, s => Assert.Equal(SettingScope.Format, s.Scope));
        Assert.All(Catalog.GlobalSettings, s => Assert.Equal(SettingScope.Global, s.Scope));
    }

    [Fact]
    public void AllDefaults_AreValid_IncludingPerFormatOverrides()
    {
        foreach (var s in Catalog.FormatSettings.Concat(Catalog.GlobalSettings))
            Assert.True(s.IsValid(s.DefaultValue), $"default of {s.Key} invalid");
        foreach (var f in Catalog.Formats)
            foreach (var (key, value) in f.DefaultOverrides)
            {
                var setting = Catalog.FindSetting(key);
                Assert.NotNull(setting);
                Assert.True(setting!.IsValid(value), $"{f.Id}.{key} override invalid");
            }
    }

    [Fact]
    public void Extensions_AreNormalizedAndOwnedByExactlyOneFormat()
    {
        var all = Catalog.AllExtensions.ToList();
        Assert.Equal(all.Count, all.Distinct(StringComparer.OrdinalIgnoreCase).Count());
        foreach (var ext in all)
        {
            Assert.Equal(FormatDefinition.Normalize(ext), ext);
            Assert.Single(Catalog.Formats, f => f.Owns(ext));
        }
    }

    [Fact]
    public void Lookups_WorkCaseInsensitively()
    {
        Assert.Same(Catalog.Photoshop, Catalog.FindFormat("PSD"));
        Assert.Same(Catalog.Photoshop, Catalog.FormatForExtension("PSB"));
        Assert.Same(Catalog.Eps, Catalog.FormatForExtension(" .EPSI "));
        Assert.Same(Catalog.FindFormat("png"), Catalog.FormatForExtension("PNG"));
        Assert.Null(Catalog.FindFormat("xyz"));
        Assert.Null(Catalog.FormatForExtension(".xyz"));
        Assert.Null(Catalog.FormatForExtension(""));
        Assert.Same(Catalog.Alpha, Catalog.FindSetting("alpha"));
        Assert.Null(Catalog.FindSetting("nope"));
    }

    [Fact]
    public void PremultipliedAlpha_IsSupportedOnlyByFormatsDeclaringIt()
    {
        var expected = new[] { "psd", "tiff", "tga", "exr", "dds", "ktx", "pvr", "tim", "sgi" };
        Assert.Equal(expected.Order(), Catalog.Formats.Where(Catalog.Alpha.IsSupportedBy).Select(f => f.Id).Order());
    }

    [Fact]
    public void GlobalSettings_AreNotSupportedByAnyFormat()
    {
        foreach (var g in Catalog.GlobalSettings)
            Assert.DoesNotContain(Catalog.Formats, g.IsSupportedBy);
    }

    [Fact]
    public void SettingsScope_ForExtension_NormalizesAndLabels()
    {
        var scope = SettingsScope.ForExtension(Catalog.Photoshop, "PSB");
        Assert.Equal(".psb", scope.Extension);
        Assert.Equal("Adobe Photoshop › .psb", scope.Label);
        Assert.Equal("All formats", SettingsScope.AllFormats.Label);
        Assert.Equal(Catalog.Formats, SettingsScope.AllFormats.Formats);
        Assert.Equal(new[] { Catalog.Pdf }, SettingsScope.ForFormat(Catalog.Pdf).Formats);
    }
}
