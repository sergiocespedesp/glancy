using Glancy.Core.Storage;
using Microsoft.Win32;

namespace Glancy.Core.Tests;

// Shares the HKCU\Software\Glancy.Tests parent key with other registry tests: run them sequentially.
[Collection("Registry")]
public sealed class RegistrySettingsStoreTests : IDisposable
{
    private readonly TestRegistryKey _key = new();
    private readonly RegistrySettingsStore _store;

    public RegistrySettingsStoreTests()
    {
        _store = new RegistrySettingsStore(_key.Path);
        Assert.NotEqual(AppInfo.SettingsKeyPath, _store.KeyPath, StringComparer.OrdinalIgnoreCase);
    }

    public void Dispose() => _key.Dispose();

    [Fact]
    public void Load_MissingKey_ReturnsDefaults()
    {
        SnapshotAssert.Equal(SettingsSnapshot.CreateDefaults(), _store.Load());
        Assert.Equal(0, _store.ReadRevision());
        Assert.Null(_key.Open()); // Load must not create the key
    }

    [Fact]
    public void SaveThenLoad_RoundTrips()
    {
        var snap = SampleSnapshots.Customized();
        _store.Save(snap);
        SnapshotAssert.Equal(snap, _store.Load());
    }

    [Fact]
    public void Save_WritesDocumentedLayout()
    {
        _store.Save(SampleSnapshots.Customized());

        using var root = _key.Open()!;
        Assert.Equal(RegistrySettingsStore.SchemaVersion, root.GetValue(RegistrySettingsStore.SchemaValue));
        Assert.Equal(RegistryValueKind.DWord, root.GetValueKind(RegistrySettingsStore.RevisionValue));

        using var g = _key.Open("Global")!;
        Assert.Equal(RegistryValueKind.DWord, g.GetValueKind(Catalog.Enabled.Key));
        Assert.Equal(RegistryValueKind.MultiString, g.GetValueKind(Catalog.BlockedFolders.Key));

        using var psd = _key.Open(@"Formats\psd")!;
        Assert.Equal(RegistryValueKind.DWord, psd.GetValueKind(Catalog.CanvasColor.Key));
        Assert.Equal(RegistryValueKind.String, psd.GetValueKind(Catalog.Checker.Key));
        Assert.Equal(RegistryValueKind.String, psd.GetValueKind(Catalog.CustomIconPath.Key));
        Assert.Equal(Catalog.FormatSettings.Count, psd.ValueCount);

        using var psb = _key.Open(@"Formats\psd\Extensions\.psb")!;
        Assert.Equal(new[] { Catalog.CustomIconPath.Key, Catalog.Mode.Key }, psb.GetValueNames().Order());
        Assert.Null(_key.Open(@"Formats\psd\Extensions\.psd"));
    }

    [Fact]
    public void RemovedOverrides_DisappearOnResave()
    {
        var snap = SampleSnapshots.Customized();
        _store.Save(snap);
        Assert.NotNull(_key.Open(@"Formats\eps\Extensions\.epsi"));

        snap.Overrides.Remove(".epsi");
        snap.Overrides[".psb"].Remove(Catalog.Mode.Key);
        _store.Save(snap);

        Assert.Null(_key.Open(@"Formats\eps\Extensions\.epsi"));
        using (var psb = _key.Open(@"Formats\psd\Extensions\.psb")!)
            Assert.Equal(new[] { Catalog.CustomIconPath.Key }, psb.GetValueNames());

        var loaded = _store.Load();
        Assert.False(loaded.Overrides.ContainsKey(".epsi"));
        Assert.Single(loaded.Overrides[".psb"]);

        snap.Overrides.Clear();
        _store.Save(snap);
        using (var ek = _key.Open(@"Formats\psd\Extensions")!)
            Assert.Empty(ek.GetSubKeyNames());
        Assert.Empty(_store.Load().Overrides);
    }

    [Fact]
    public void Overrides_EqualToFormatValue_AreNotReportedOnLoad()
    {
        var snap = SettingsSnapshot.CreateDefaults();
        snap.Overrides[".psb"] = new Dictionary<string, object>
        {
            [Catalog.Mode.Key] = Values.Generation.Enabled,     // same as psd -> dropped on load
            [Catalog.Adornment.Key] = Values.Adornment.None,    // differs -> kept
        };
        _store.Save(snap);

        var loaded = _store.Load();
        var o = Assert.Single(loaded.Overrides).Value;
        Assert.Equal(new[] { Catalog.Adornment.Key }, o.Keys);
    }

    [Fact]
    public void Session_OverridesPersistOnlyForDifferingValues()
    {
        var session = new SettingsSession(_store);
        var psb = SettingsScope.ForExtension(Catalog.Photoshop, ".psb");
        session.Write(psb, Catalog.Mode, Values.Generation.Disabled);
        session.Write(psb, Catalog.Adornment, Values.Adornment.DropShadow); // equals format: no override
        session.Apply();

        using (var k = _key.Open(@"Formats\psd\Extensions\.psb")!)
            Assert.Equal(new[] { Catalog.Mode.Key }, k.GetValueNames());

        session.Write(psb, Catalog.Mode, Values.Generation.Enabled); // back to format value
        session.Apply();
        Assert.Null(_key.Open(@"Formats\psd\Extensions\.psb"));

        var reloaded = new SettingsSession(_store);
        Assert.False(reloaded.HasOverrides(".psb"));
    }

    [Fact]
    public void Revision_IncrementsOnEachSave()
    {
        Assert.Equal(0, _store.ReadRevision());
        _store.Save(SettingsSnapshot.CreateDefaults());
        Assert.Equal(1, _store.ReadRevision());
        _store.Save(SettingsSnapshot.CreateDefaults());
        Assert.Equal(2, _store.ReadRevision());
        new RegistrySettingsStore(_key.Path).Save(SampleSnapshots.Customized());
        Assert.Equal(3, _store.ReadRevision());
    }

    [Fact]
    public void Revision_WrapsInsteadOfThrowing()
    {
        using (var root = Registry.CurrentUser.CreateSubKey(_key.Path, writable: true))
            root.SetValue(RegistrySettingsStore.RevisionValue, int.MaxValue, RegistryValueKind.DWord);
        _store.Save(SettingsSnapshot.CreateDefaults());
        Assert.Equal(int.MinValue, _store.ReadRevision());
    }

    [Fact]
    public void InvalidRegistryValues_FallBackToDefaults()
    {
        var snap = SampleSnapshots.Customized();
        _store.Save(snap);

        using (var g = _key.Create("Global"))
        {
            g.SetValue(Catalog.Enabled.Key, 5, RegistryValueKind.DWord);                 // toggle out of range
            g.SetValue(Catalog.DebugLog.Key, "1", RegistryValueKind.String);             // wrong type
            g.SetValue(Catalog.BlockedFolders.Key, @"C:\x", RegistryValueKind.String);   // list stored as REG_SZ
            g.SetValue(Catalog.CheckerScale.Key, 2L, RegistryValueKind.QWord);           // QWORD
            g.SetValue(Catalog.ContextMenu.Key, 7, RegistryValueKind.DWord);             // choice out of range
        }
        using (var psd = _key.Create(@"Formats\psd"))
        {
            psd.SetValue(Catalog.Transparency.Key, 99, RegistryValueKind.DWord);         // out of range choice
            psd.SetValue(Catalog.Checker.Key, 12345, RegistryValueKind.DWord);           // wrong type
            psd.SetValue(Catalog.CanvasColor.Key, "white", RegistryValueKind.String);    // wrong type
            psd.SetValue(Catalog.CustomIconPath.Key, new[] { "a" }, RegistryValueKind.MultiString);
        }
        using (var ai = _key.Create(@"Formats\ai"))
        {
            ai.SetValue(Catalog.Transparency.Key, "1", RegistryValueKind.String);        // wrong type
        }
        using (var eps = _key.Create(@"Formats\eps"))
        {
            eps.SetValue(Catalog.Checker.Key, "zz;yy", RegistryValueKind.String);        // unparsable checker
        }
        using (var psb = _key.Create(@"Formats\psd\Extensions\.psb"))
        {
            psb.SetValue(Catalog.Mode.Key, 77, RegistryValueKind.DWord);                  // invalid override
        }
        using (var bogus = _key.Create(@"Formats\psd\Extensions\.png"))
            bogus.SetValue(Catalog.Mode.Key, 0, RegistryValueKind.DWord);               // extension not owned by psd
        using (var unknown = _key.Create(@"Formats\xyz"))
            unknown.SetValue(Catalog.Mode.Key, 0, RegistryValueKind.DWord);             // unknown format

        var loaded = _store.Load();
        var defaults = SettingsSnapshot.CreateDefaults();

        Assert.Equal(1, loaded.Global[Catalog.Enabled.Key]);
        Assert.Equal(0, loaded.Global[Catalog.DebugLog.Key]);
        Assert.Empty((string[])loaded.Global[Catalog.BlockedFolders.Key]);
        Assert.Equal(1, loaded.Global[Catalog.CheckerScale.Key]);
        Assert.Equal(Values.ContextMenu.Submenu, loaded.Global[Catalog.ContextMenu.Key]);
        Assert.Equal(new[] { "FF000000;FFFFFFFF" }, (string[])loaded.Global[Catalog.CheckerLibrary.Key]); // valid value kept

        Assert.Equal(Values.Transparency.Transparent, loaded.Formats["psd"][Catalog.Transparency.Key]);
        Assert.Equal(defaults.Formats["psd"][Catalog.Checker.Key], loaded.Formats["psd"][Catalog.Checker.Key]);
        Assert.Equal(defaults.Formats["psd"][Catalog.CanvasColor.Key], loaded.Formats["psd"][Catalog.CanvasColor.Key]);
        Assert.Equal(string.Empty, loaded.Formats["psd"][Catalog.CustomIconPath.Key]);
        Assert.Equal(Values.Generation.Fallback, loaded.Formats["psd"][Catalog.Mode.Key]); // valid value kept
        Assert.Equal(Values.Transparency.Canvas, loaded.Formats["ai"][Catalog.Transparency.Key]); // per-format default
        Assert.Equal(defaults.Formats["eps"][Catalog.Checker.Key], loaded.Formats["eps"][Catalog.Checker.Key]);
        Assert.Equal(220, loaded.Formats["eps"][Catalog.Gamma.Key]);

        Assert.False(loaded.Overrides[".psb"].ContainsKey(Catalog.Mode.Key));
        Assert.Equal("other.ico,1", loaded.Overrides[".psb"][Catalog.CustomIconPath.Key]);
        Assert.False(loaded.Overrides.ContainsKey(".png"));
        Assert.False(loaded.Formats.ContainsKey("xyz"));
    }

    [Fact]
    public void Save_RemovesExtensionKeysTheFormatDoesNotOwn_AndKeepsOwnedOnes()
    {
        var snap = SampleSnapshots.Customized();
        _store.Save(snap);
        using (_key.Create(@"Formats\psd\Extensions\.png")) { }

        _store.Save(snap);

        Assert.Null(_key.Open(@"Formats\psd\Extensions\.png"));
        Assert.NotNull(_key.Open(@"Formats\psd\Extensions\.psb"));
    }

    [Fact]
    public void ColorDword_WithHighBitSet_RoundTrips()
    {
        var snap = SettingsSnapshot.CreateDefaults();
        snap.Formats["svg"][Catalog.CanvasColor.Key] = unchecked((int)0x80FF00FF);
        _store.Save(snap);
        Assert.Equal(unchecked((int)0x80FF00FF), _store.Load().Formats["svg"][Catalog.CanvasColor.Key]);
    }
}
