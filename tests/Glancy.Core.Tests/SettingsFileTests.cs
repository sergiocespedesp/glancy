using System.Text.Json.Nodes;
using Glancy.Core.Storage;

namespace Glancy.Core.Tests;

public class SettingsFileTests
{
    [Fact]
    public void SerializeDeserialize_RoundTrips()
    {
        var snap = SampleSnapshots.Customized();
        var json = SettingsFile.Serialize(snap);
        var back = SettingsFile.Deserialize(json, out var ignored);
        Assert.Equal(0, ignored);
        SnapshotAssert.Equal(snap, back);
    }

    [Fact]
    public void SerializeDeserialize_Defaults_RoundTrips()
    {
        var json = SettingsFile.Serialize(SettingsSnapshot.CreateDefaults());
        var back = SettingsFile.Deserialize(json, out var ignored);
        Assert.Equal(0, ignored);
        SnapshotAssert.Equal(SettingsSnapshot.CreateDefaults(), back);
    }

    [Fact]
    public void Serialize_ProducesTaggedDocument()
    {
        var obj = JsonNode.Parse(SettingsFile.Serialize(SampleSnapshots.Customized()))!.AsObject();
        Assert.Equal("Glancy", obj["product"]!.GetValue<string>());
        Assert.Equal(1, obj["version"]!.GetValue<int>());
        Assert.NotNull(obj["exported"]);
        Assert.Equal(Catalog.Formats.Count, obj["formats"]!.AsObject().Count);
        Assert.Equal(2, obj["overrides"]!.AsObject().Count);
        Assert.Equal(2, obj["global"]![Catalog.BlockedFolders.Key]!.AsArray().Count);
    }

    [Fact]
    public void Serialize_SkipsEmptyOverrideDictionaries()
    {
        var snap = SettingsSnapshot.CreateDefaults();
        snap.Overrides[".psb"] = new Dictionary<string, object>();
        var obj = JsonNode.Parse(SettingsFile.Serialize(snap))!.AsObject();
        Assert.Empty(obj["overrides"]!.AsObject());
    }

    [Theory]
    [InlineData("""{"product":"Other","version":1}""")]
    [InlineData("""{"product":"glancy"}""")]
    [InlineData("""{"version":1,"global":{}}""")]
    [InlineData("""[1,2,3]""")]
    [InlineData("""null""")]
    [InlineData("""not json at all""")]
    [InlineData("")]
    [InlineData("""{"product":"Glancy" """)]
    public void Deserialize_ForeignOrMalformed_ThrowsSettingsStoreException(string json)
    {
        Assert.Throws<SettingsStoreException>(() => SettingsFile.Deserialize(json, out _));
    }

    [Fact]
    public void Deserialize_UnknownKeysAndInvalidValues_AreCountedAndIgnored()
    {
        const string json = """
        {
          "product": "Glancy",
          "version": 99,
          "futureField": {"x": 1},
          "global": {
            "enabled": 0,
            "unknownKey": 1,
            "debugLog": 7,
            "blockedFolders": ["C:\\a", "D:\\b"],
            "checkerScale": "2"
          },
          "formats": {
            "psd": { "mode": 2, "transparency": 42, "canvasColor": "red", "bogus": 1, "gamma": 2.2 },
            "PDF": { "adornment": 0 },
            "xyz": { "mode": 0 },
            "svg": 5
          },
          "overrides": {
            ".psb": { "mode": 0, "nope": 1 },
            "EPSI": { "transparency": 0 },
            ".psd": { "mode": 2 },
            ".abc": { "mode": 0 }
          }
        }
        """;
        var snap = SettingsFile.Deserialize(json, out var ignored);

        // global: unknownKey, debugLog=7, checkerScale="2"          -> 3
        // formats: transparency=42, canvasColor, bogus, gamma=2.2,
        //          xyz (unknown format), svg (not an object)        -> 6
        // overrides: nope, .abc (unknown extension)                 -> 2
        Assert.Equal(11, ignored);

        Assert.Equal(0, snap.Global[Catalog.Enabled.Key]);
        Assert.Equal(0, snap.Global[Catalog.DebugLog.Key]);
        Assert.Equal(1, snap.Global[Catalog.CheckerScale.Key]);
        Assert.Equal(new[] { @"C:\a", @"D:\b" }, (string[])snap.Global[Catalog.BlockedFolders.Key]);
        Assert.Equal(Values.Generation.Fallback, snap.Formats["psd"][Catalog.Mode.Key]);
        Assert.Equal(Values.Transparency.Transparent, snap.Formats["psd"][Catalog.Transparency.Key]);
        Assert.Equal(unchecked((int)Catalog.White), snap.Formats["psd"][Catalog.CanvasColor.Key]);
        Assert.Equal(100, snap.Formats["psd"][Catalog.Gamma.Key]);
        Assert.False(snap.Formats["psd"].ContainsKey("bogus"));
        Assert.Equal(Values.Adornment.None, snap.Formats["pdf"][Catalog.Adornment.Key]);
        Assert.Equal(Catalog.FormatSettings.Count, snap.Formats["psd"].Count);

        Assert.Equal(Values.Generation.Disabled, snap.Overrides[".psb"][Catalog.Mode.Key]);
        Assert.Single(snap.Overrides[".psb"]);
        Assert.Equal(Values.Transparency.Opaque, snap.Overrides[".epsi"][Catalog.Transparency.Key]);
        Assert.False(snap.Overrides.ContainsKey(".psd")); // equal to format value: dropped, not counted
        Assert.False(snap.Overrides.ContainsKey(".abc"));
    }

    [Fact]
    public void Deserialize_MissingSections_UseDefaults()
    {
        var snap = SettingsFile.Deserialize("""{"product":"Glancy"}""", out var ignored);
        Assert.Equal(0, ignored);
        SnapshotAssert.Equal(SettingsSnapshot.CreateDefaults(), snap);
    }

    // ---- Robustness of the import path. These document behaviour of Deserialize on hostile/odd input:
    // its contract says unknown/invalid values are ignored and invalid documents throw SettingsStoreException.

    [Fact]
    public void Deserialize_ListWithNonStringItems_IsIgnoredNotThrown()
    {
        const string json = """{"product":"Glancy","global":{"blockedFolders":[1,2],"debugLog":1}}""";
        var snap = SettingsFile.Deserialize(json, out var ignored);
        Assert.Equal(1, ignored);
        Assert.Empty((string[])snap.Global[Catalog.BlockedFolders.Key]);
        Assert.Equal(1, snap.Global[Catalog.DebugLog.Key]);
    }

    [Fact]
    public void Deserialize_NonStringProductTag_ThrowsSettingsStoreException()
    {
        Assert.Throws<SettingsStoreException>(() => SettingsFile.Deserialize("""{"product":1}""", out _));
    }

    [Theory]
    [InlineData("""{"product":"Glancy","global":{"blockedFolders":["a",null]}}""")]
    [InlineData("""{"product":"Glancy","global":{"blockedFolders":["a",""]}}""")]
    public void Deserialize_ListWithNullOrEmptyItem_IsIgnored(string json)
    {
        // An empty folder path would block nothing useful and confuse the engine: the whole list is rejected.
        var snap = SettingsFile.Deserialize(json, out var ignored);
        Assert.Equal(1, ignored);
        Assert.Empty((string[])snap.Global[Catalog.BlockedFolders.Key]);
    }
}
