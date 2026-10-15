namespace Glancy.Core.Tests;

public class SettingsSessionTests
{
    private static readonly SettingsScope Psd = SettingsScope.ForFormat(Catalog.Photoshop);
    private static readonly SettingsScope Psb = SettingsScope.ForExtension(Catalog.Photoshop, ".psb");
    private static readonly SettingsScope PsdExt = SettingsScope.ForExtension(Catalog.Photoshop, ".psd");

    private static (SettingsSession Session, InMemorySettingsStore Store) Create(SettingsSnapshot? stored = null)
    {
        var store = new InMemorySettingsStore();
        if (stored is not null) store.Stored = stored;
        return (new SettingsSession(store), store);
    }

    // ------------------------------------------------------------------ basic write / pending / apply

    [Fact]
    public void Constructor_LoadsStore_AndStartsClean()
    {
        var (session, store) = Create();
        Assert.Equal(1, store.LoadCount);
        Assert.False(session.IsDirty);
        Assert.Empty(session.GetChanges());
        Assert.Empty(session.GetAffectedExtensions());
        Assert.NotSame(session.Applied, session.Working);
    }

    [Fact]
    public void Write_FormatScope_ChangesWorkingOnly_AndIsPending()
    {
        var (session, store) = Create();
        var raised = 0;
        session.Changed += (_, _) => raised++;

        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);

        Assert.Equal(1, raised);
        var r = session.Read(Psd, Catalog.Mode);
        Assert.Equal(Values.Generation.Disabled, r.Value);
        Assert.False(r.IsMixed);
        Assert.False(r.IsOverridden);
        Assert.True(session.IsPending(Psd, Catalog.Mode));
        Assert.False(session.IsPending(Psd, Catalog.Adornment));
        Assert.True(session.IsDirty);
        Assert.Equal(Values.Generation.Enabled, session.Applied.Formats["psd"][Catalog.Mode.Key]);
        Assert.Equal(0, store.SaveCount);
        // Other formats untouched.
        Assert.Equal(Values.Generation.Enabled, session.Working.Formats["ai"][Catalog.Mode.Key]);
    }

    [Fact]
    public void GetChanges_ListsFormatChange_WithFromAndTo()
    {
        var (session, _) = Create();
        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);

        var change = Assert.Single(session.GetChanges());
        Assert.Equal("Adobe Photoshop", change.ScopeLabel);
        Assert.Same(Catalog.Mode, change.Setting);
        Assert.Equal(Values.Generation.Enabled, change.From);
        Assert.Equal(Values.Generation.Disabled, change.To);
        Assert.Equal("Thumbnails: Glancy → Off", change.Summary);
    }

    [Fact]
    public void GetChanges_ListsGlobalAndExtensionChanges()
    {
        var (session, _) = Create();
        session.WriteGlobal(Catalog.DebugLog, 1);
        session.Write(Psb, Catalog.Adornment, Values.Adornment.None);

        var changes = session.GetChanges();
        Assert.Equal(2, changes.Count);

        var g = Assert.Single(changes, c => c.Setting == Catalog.DebugLog);
        Assert.Equal("General", g.ScopeLabel);
        Assert.Equal(0, g.From);
        Assert.Equal(1, g.To);

        var x = Assert.Single(changes, c => c.Setting == Catalog.Adornment);
        Assert.Equal("Adobe Photoshop › .psb", x.ScopeLabel);
        Assert.Null(x.From);
        Assert.Equal(Values.Adornment.None, x.To);
        Assert.Contains("Inherited from the format", x.Summary);
    }

    [Fact]
    public void WritingBackToAppliedValue_IsNoLongerPending()
    {
        var (session, _) = Create();
        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);
        session.Write(Psd, Catalog.Mode, Values.Generation.Enabled);
        Assert.False(session.IsDirty);
        Assert.False(session.IsPending(Psd, Catalog.Mode));
    }

    [Fact]
    public void Discard_RestoresApplied()
    {
        var (session, _) = Create();
        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);
        session.Write(Psb, Catalog.Adornment, Values.Adornment.None);
        session.WriteGlobal(Catalog.Enabled, 0);

        session.Discard();

        Assert.False(session.IsDirty);
        Assert.Equal(Values.Generation.Enabled, session.Read(Psd, Catalog.Mode).Value);
        Assert.False(session.HasOverrides(".psb"));
        Assert.Equal(1, session.ReadGlobal(Catalog.Enabled).Value);
        SnapshotAssert.Equal(session.Applied, session.Working);
        Assert.NotSame(session.Applied, session.Working);
    }

    [Fact]
    public void Apply_SavesToStore_AndClearsDirty()
    {
        var (session, store) = Create();
        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);
        session.WriteGlobal(Catalog.BlockedFolders, new[] { @"C:\x" });

        session.Apply();

        Assert.Equal(1, store.SaveCount);
        Assert.Equal(Values.Generation.Disabled, store.Stored.Formats["psd"][Catalog.Mode.Key]);
        Assert.Equal(new[] { @"C:\x" }, (string[])store.Stored.Global[Catalog.BlockedFolders.Key]);
        Assert.False(session.IsDirty);
        Assert.Empty(session.GetChanges());
        Assert.False(session.IsPending(Psd, Catalog.Mode));
        Assert.Equal(Values.Generation.Disabled, session.Applied.Formats["psd"][Catalog.Mode.Key]);
        Assert.NotSame(session.Applied, session.Working);

        // Further edits after Apply do not leak into Applied.
        session.Write(Psd, Catalog.Mode, Values.Generation.Fallback);
        Assert.Equal(Values.Generation.Disabled, session.Applied.Formats["psd"][Catalog.Mode.Key]);
    }

    [Fact]
    public void Apply_WhenStoreThrows_KeepsPendingState()
    {
        var store = new ThrowingStore();
        var session = new SettingsSession(store);
        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);
        Assert.Throws<SettingsStoreException>(session.Apply);
        Assert.True(session.IsDirty);
        Assert.Equal(Values.Generation.Enabled, session.Applied.Formats["psd"][Catalog.Mode.Key]);
    }

    private sealed class ThrowingStore : ISettingsStore
    {
        public SettingsSnapshot Load() => SettingsSnapshot.CreateDefaults();
        public void Save(SettingsSnapshot snapshot) => throw new SettingsStoreException("boom");
    }

    [Fact]
    public void Reload_RereadsStore_AndDropsPendingEdits()
    {
        var (session, store) = Create();
        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);
        var external = SettingsSnapshot.CreateDefaults();
        external.Formats["svg"][Catalog.Adornment.Key] = Values.Adornment.None;
        store.Stored = external;

        session.Reload();

        Assert.False(session.IsDirty);
        Assert.Equal(Values.Generation.Enabled, session.Read(Psd, Catalog.Mode).Value);
        Assert.Equal(Values.Adornment.None, session.Read(SettingsScope.ForFormat(Catalog.Svg), Catalog.Adornment).Value);
    }

    [Fact]
    public void LoadWorking_ReplacesWorkingWithCopy_AndIsPending()
    {
        var (session, _) = Create();
        var imported = SampleSnapshots.Customized();
        session.LoadWorking(imported);
        Assert.True(session.IsDirty);
        SnapshotAssert.Equal(imported, session.Working);
        Assert.NotSame(imported, session.Working);
        imported.Formats["psd"][Catalog.Mode.Key] = Values.Generation.Enabled;
        Assert.Equal(Values.Generation.Fallback, session.Working.Formats["psd"][Catalog.Mode.Key]);
    }

    // ------------------------------------------------------------------ all formats scope

    [Fact]
    public void Write_AllFormats_OnlyAffectsSupportingFormats()
    {
        var (session, _) = Create();
        session.Write(SettingsScope.AllFormats, Catalog.Alpha, Values.Alpha.Premultiplied);

        Assert.Equal(Values.Alpha.Premultiplied, session.Working.Formats["psd"][Catalog.Alpha.Key]);
        foreach (var id in new[] { "ai", "eps", "pdf", "svg" })
            Assert.Equal(Values.Alpha.Straight, session.Working.Formats[id][Catalog.Alpha.Key]);

        var r = session.Read(SettingsScope.AllFormats, Catalog.Alpha);
        Assert.False(r.IsMixed);
        Assert.Equal(Values.Alpha.Premultiplied, r.Value);

        // One change per format that declares the "alpha" capability, and none for the others.
        var supporting = Catalog.Formats.Where(Catalog.Alpha.IsSupportedBy).Select(f => f.DisplayName).ToList();
        Assert.Contains("Adobe Photoshop", supporting);
        Assert.Equal(supporting, session.GetChanges().Select(c => c.ScopeLabel));
    }

    [Fact]
    public void Write_AllFormats_Sampling_SkipsAiPdfSvg()
    {
        var (session, _) = Create();
        session.Write(SettingsScope.AllFormats, Catalog.Sampling, Values.Sampling.Nearest);
        Assert.Equal(Values.Sampling.Nearest, session.Working.Formats["psd"][Catalog.Sampling.Key]);
        Assert.Equal(Values.Sampling.Nearest, session.Working.Formats["eps"][Catalog.Sampling.Key]);
        foreach (var id in new[] { "ai", "pdf", "svg" })
            Assert.Equal(Values.Sampling.Cubic, session.Working.Formats[id][Catalog.Sampling.Key]);
        Assert.Equal(Catalog.Formats.Count(Catalog.Sampling.IsSupportedBy), session.GetChanges().Count);
    }

    [Fact]
    public void Read_AllFormats_IsMixed_WhenFormatsDiffer()
    {
        var (session, _) = Create();
        // Defaults: psd/svg Transparent, ai/eps/pdf Canvas.
        var r = session.Read(SettingsScope.AllFormats, Catalog.Transparency);
        Assert.True(r.IsMixed);
        Assert.Null(r.Value);

        session.Write(SettingsScope.AllFormats, Catalog.Transparency, Values.Transparency.Opaque);
        r = session.Read(SettingsScope.AllFormats, Catalog.Transparency);
        Assert.False(r.IsMixed);
        Assert.Equal(Values.Transparency.Opaque, r.Value);
        Assert.All(Catalog.Formats, f => Assert.Equal(Values.Transparency.Opaque, session.Working.Formats[f.Id][Catalog.Transparency.Key]));

        session.Write(SettingsScope.ForFormat(Catalog.Svg), Catalog.Transparency, Values.Transparency.Checkerboard);
        Assert.True(session.Read(SettingsScope.AllFormats, Catalog.Transparency).IsMixed);
    }

    [Fact]
    public void Read_AllFormats_ReportsOverridden_WhenAnyExtensionOverrides()
    {
        var (session, _) = Create();
        Assert.False(session.Read(SettingsScope.AllFormats, Catalog.Mode).IsOverridden);
        session.Write(SettingsScope.ForExtension(Catalog.Eps, ".epsi"), Catalog.Mode, Values.Generation.Disabled);
        Assert.True(session.Read(SettingsScope.AllFormats, Catalog.Mode).IsOverridden);
        Assert.True(session.IsPending(SettingsScope.AllFormats, Catalog.Mode));
    }

    [Fact]
    public void Write_AllFormats_ClearsExtensionOverridesForThatKey()
    {
        var (session, _) = Create();
        session.Write(SettingsScope.ForExtension(Catalog.Eps, ".epsi"), Catalog.Mode, Values.Generation.Disabled);
        session.Write(SettingsScope.ForExtension(Catalog.Eps, ".epsi"), Catalog.Adornment, Values.Adornment.None);
        session.Write(SettingsScope.AllFormats, Catalog.Mode, Values.Generation.Fallback);
        var r = session.Read(SettingsScope.ForExtension(Catalog.Eps, ".epsi"), Catalog.Mode);
        Assert.False(r.IsOverridden);
        Assert.Equal(Values.Generation.Fallback, r.Value);
        Assert.True(session.Read(SettingsScope.ForExtension(Catalog.Eps, ".epsi"), Catalog.Adornment).IsOverridden);
    }

    [Fact]
    public void UnsupportedSetting_InScope_ReadsNull_AndIsNotSupported()
    {
        var (session, _) = Create();
        var svg = SettingsScope.ForFormat(Catalog.Svg);
        Assert.False(SettingsSession.IsSupported(svg, Catalog.Alpha));
        Assert.True(SettingsSession.IsSupported(SettingsScope.AllFormats, Catalog.Alpha));
        Assert.Empty(SettingsSession.SupportingFormats(svg, Catalog.Gamma));

        // Write at format scope for an unsupported setting is a no-op.
        session.Write(svg, Catalog.Alpha, Values.Alpha.Premultiplied);
        Assert.Equal(Values.Alpha.Straight, session.Working.Formats["svg"][Catalog.Alpha.Key]);
        Assert.False(session.IsDirty);
    }

    [Fact]
    public void UnsupportedSetting_AtExtensionScope_IsNotStored()
    {
        var (session, _) = Create();
        session.Write(SettingsScope.ForExtension(Catalog.Svg, ".svg"), Catalog.Alpha, Values.Alpha.Premultiplied);
        Assert.False(session.HasOverrides(".svg"));
        Assert.False(session.IsDirty);
        Assert.Empty(session.GetAffectedExtensions());
    }

    // ------------------------------------------------------------------ extension scope

    [Fact]
    public void Write_Extension_DifferentValue_CreatesOverride()
    {
        var (session, _) = Create();
        session.Write(Psb, Catalog.Mode, Values.Generation.Disabled);

        var r = session.Read(Psb, Catalog.Mode);
        Assert.Equal(Values.Generation.Disabled, r.Value);
        Assert.True(r.IsOverridden);
        Assert.False(r.IsMixed);
        Assert.True(session.HasOverrides(".psb"));
        Assert.False(session.HasOverrides(".psd"));

        // Sibling extension and format are unaffected.
        var sibling = session.Read(PsdExt, Catalog.Mode);
        Assert.Equal(Values.Generation.Enabled, sibling.Value);
        Assert.False(sibling.IsOverridden);
        var format = session.Read(Psd, Catalog.Mode);
        Assert.Equal(Values.Generation.Enabled, format.Value);
        Assert.True(format.IsOverridden);

        Assert.True(session.IsPending(Psb, Catalog.Mode));
        Assert.True(session.IsPending(Psd, Catalog.Mode)); // IsOverridden changed
        Assert.False(session.IsPending(PsdExt, Catalog.Mode));
    }

    [Fact]
    public void Write_Extension_ValueEqualToFormat_RemovesOverride()
    {
        var (session, _) = Create();
        session.Write(Psb, Catalog.Mode, Values.Generation.Disabled);
        session.Write(Psb, Catalog.Mode, Values.Generation.Enabled);

        Assert.False(session.Read(Psb, Catalog.Mode).IsOverridden);
        Assert.False(session.HasOverrides(".psb"));
        Assert.False(session.Working.Overrides.ContainsKey(".psb"));
        Assert.False(session.IsDirty);
    }

    [Fact]
    public void Write_Extension_EqualToFormat_WithNoExistingOverride_DoesNothing()
    {
        var (session, _) = Create();
        session.Write(Psb, Catalog.Mode, Values.Generation.Enabled);
        Assert.False(session.HasOverrides(".psb"));
        Assert.False(session.IsDirty);
    }

    [Fact]
    public void Write_Extension_IsCaseInsensitive()
    {
        var (session, _) = Create();
        session.Write(SettingsScope.ForExtension(Catalog.Photoshop, "PSB"), Catalog.Mode, Values.Generation.Disabled);
        Assert.True(session.HasOverrides(".psb"));
        Assert.True(session.HasOverrides(".PSB"));
        Assert.Equal(Values.Generation.Disabled, session.Read(Psb, Catalog.Mode).Value);
    }

    [Fact]
    public void Write_Format_ClearsThatKeysExtensionOverrides_Only()
    {
        var (session, _) = Create();
        session.Write(Psb, Catalog.Mode, Values.Generation.Disabled);
        session.Write(Psb, Catalog.Adornment, Values.Adornment.None);
        session.Write(PsdExt, Catalog.Mode, Values.Generation.Fallback);

        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);

        Assert.False(session.Read(Psb, Catalog.Mode).IsOverridden);
        Assert.False(session.Read(PsdExt, Catalog.Mode).IsOverridden);
        Assert.Equal(Values.Generation.Disabled, session.Read(PsdExt, Catalog.Mode).Value);
        Assert.True(session.Read(Psb, Catalog.Adornment).IsOverridden);
        Assert.Equal(Values.Adornment.None, session.Read(Psb, Catalog.Adornment).Value);
        Assert.False(session.HasOverrides(".psd"));
        Assert.True(session.HasOverrides(".psb"));
    }

    [Fact]
    public void Write_Extension_ListAndStringValues_AreCopied()
    {
        var (session, _) = Create();
        var path = "x.ico,2";
        session.Write(Psb, Catalog.CustomIconPath, path);
        Assert.Equal(path, session.Read(Psb, Catalog.CustomIconPath).Value);

        var folders = new[] { @"C:\a" };
        session.WriteGlobal(Catalog.BlockedFolders, folders);
        folders[0] = "mutated";
        Assert.Equal(@"C:\a", ((string[])session.ReadGlobal(Catalog.BlockedFolders).Value!)[0]);
    }

    // ------------------------------------------------------------------ reset

    [Fact]
    public void Reset_Format_RestoresPerFormatDefault_AndClearsOverrides()
    {
        var (session, _) = Create();
        var ai = SettingsScope.ForFormat(Catalog.Illustrator);
        session.Write(ai, Catalog.Transparency, Values.Transparency.Opaque);
        Assert.True(session.IsModifiedFromDefault(ai, Catalog.Transparency));

        session.Reset(ai, Catalog.Transparency);
        Assert.Equal(Values.Transparency.Canvas, session.Read(ai, Catalog.Transparency).Value); // not catalog default
        Assert.False(session.IsModifiedFromDefault(ai, Catalog.Transparency));

        session.Write(Psb, Catalog.Mode, Values.Generation.Disabled);
        session.Reset(Psd, Catalog.Mode);
        Assert.False(session.HasOverrides(".psb"));
    }

    [Fact]
    public void Reset_Extension_RemovesOverrideOnly()
    {
        var (session, _) = Create();
        session.Write(Psd, Catalog.Mode, Values.Generation.Fallback);
        session.Write(Psb, Catalog.Mode, Values.Generation.Disabled);
        session.Write(Psb, Catalog.Adornment, Values.Adornment.None);

        session.Reset(Psb, Catalog.Mode);

        Assert.Equal(Values.Generation.Fallback, session.Read(Psb, Catalog.Mode).Value);
        Assert.False(session.Read(Psb, Catalog.Mode).IsOverridden);
        Assert.Equal(Values.Generation.Fallback, session.Read(Psd, Catalog.Mode).Value);
        Assert.True(session.Read(Psb, Catalog.Adornment).IsOverridden);
    }

    [Fact]
    public void ResetScope_Format_RestoresEverySettingOfThatFormat()
    {
        var (session, _) = Create();
        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);
        session.Write(Psd, Catalog.Alpha, Values.Alpha.Premultiplied);
        session.Write(Psb, Catalog.Adornment, Values.Adornment.None);
        session.Write(SettingsScope.ForFormat(Catalog.Eps), Catalog.Gamma, 220);

        session.ResetScope(Psd);

        var defaults = SettingsSnapshot.CreateDefaults().Formats["psd"];
        foreach (var s in Catalog.FormatSettings)
            Assert.True(SettingsSnapshot.ValueEquals(defaults[s.Key], session.Working.Formats["psd"][s.Key]), s.Key);
        Assert.False(session.HasOverrides(".psb"));
        Assert.Equal(220, session.Working.Formats["eps"][Catalog.Gamma.Key]); // other formats untouched
    }

    [Fact]
    public void ResetScope_AllFormats_RestoresPerFormatDefaultsButNotGlobals()
    {
        var (session, _) = Create(SampleSnapshots.Customized());
        session.ResetScope(SettingsScope.AllFormats);

        var defaults = SettingsSnapshot.CreateDefaults();
        foreach (var f in Catalog.Formats)
            foreach (var s in Catalog.FormatSettings.Where(s => s.IsSupportedBy(f)))
                Assert.True(SettingsSnapshot.ValueEquals(defaults.Formats[f.Id][s.Key], session.Working.Formats[f.Id][s.Key]), $"{f.Id}.{s.Key}");
        Assert.Empty(session.Working.Overrides);
        Assert.Equal(1, session.ReadGlobal(Catalog.DebugLog).Value);
        Assert.Equal(Values.Transparency.Canvas, session.Working.Formats["pdf"][Catalog.Transparency.Key]);
    }

    [Fact]
    public void ResetAll_RestoresFactoryDefaults_StillPendingUntilApply()
    {
        var (session, store) = Create(SampleSnapshots.Customized());
        session.ResetAll();

        SnapshotAssert.Equal(SettingsSnapshot.CreateDefaults(), session.Working);
        Assert.True(session.IsDirty);
        Assert.Equal(0, store.SaveCount);

        session.Apply();
        SnapshotAssert.Equal(SettingsSnapshot.CreateDefaults(), store.Stored);
        Assert.False(session.IsDirty);
    }

    [Fact]
    public void ResetGlobal_RestoresDefault()
    {
        var (session, _) = Create();
        session.WriteGlobal(Catalog.CheckerScale, 2);
        Assert.True(session.IsGlobalModifiedFromDefault(Catalog.CheckerScale));
        Assert.True(session.IsGlobalPending(Catalog.CheckerScale));
        session.ResetGlobal(Catalog.CheckerScale);
        Assert.Equal(1, session.ReadGlobal(Catalog.CheckerScale).Value);
        Assert.False(session.IsGlobalModifiedFromDefault(Catalog.CheckerScale));
        Assert.False(session.IsDirty);
    }

    // ------------------------------------------------------------------ copy / paste

    [Fact]
    public void CopyPaste_BetweenFormats_SkipsUnsupportedSettings()
    {
        var (session, _) = Create();
        session.Write(Psd, Catalog.Mode, Values.Generation.Fallback);
        session.Write(Psd, Catalog.Alpha, Values.Alpha.Premultiplied);
        session.Write(Psd, Catalog.Sampling, Values.Sampling.Nearest);
        session.Write(Psd, Catalog.Gamma, 240);
        session.Write(Psd, Catalog.CanvasColor, unchecked((int)0xFF00FF00));

        var clip = session.Copy(Psd);
        Assert.Equal("Adobe Photoshop", clip.SourceLabel);
        Assert.Equal(Catalog.FormatSettings.Count, clip.Values.Count); // psd supports everything

        var aiScope = SettingsScope.ForFormat(Catalog.Illustrator);
        var pasted = session.Paste(aiScope, clip);
        Assert.Equal(Catalog.FormatSettings.Count(s => s.IsSupportedBy(Catalog.Illustrator)), pasted);

        var ai = session.Working.Formats["ai"];
        Assert.Equal(Values.Generation.Fallback, ai[Catalog.Mode.Key]);
        Assert.Equal(240, ai[Catalog.Gamma.Key]);
        Assert.Equal(unchecked((int)0xFF00FF00), ai[Catalog.CanvasColor.Key]);
        Assert.Equal(Values.Transparency.Transparent, ai[Catalog.Transparency.Key]); // psd's value pasted
        Assert.Equal(Values.Alpha.Straight, ai[Catalog.Alpha.Key]);       // unsupported: skipped
        Assert.Equal(Values.Sampling.Cubic, ai[Catalog.Sampling.Key]);    // unsupported: skipped

        var svgPasted = session.Paste(SettingsScope.ForFormat(Catalog.Svg), clip);
        Assert.Equal(Catalog.FormatSettings.Count(s => s.IsSupportedBy(Catalog.Svg)), svgPasted);
        Assert.Equal(100, session.Working.Formats["svg"][Catalog.Gamma.Key]); // gamma unsupported by svg
    }

    [Fact]
    public void Copy_FromSvg_OnlyContainsSupportedSettings()
    {
        var (session, _) = Create();
        var clip = session.Copy(SettingsScope.ForFormat(Catalog.Svg));
        Assert.DoesNotContain(Catalog.Alpha.Key, clip.Values.Keys);
        Assert.DoesNotContain(Catalog.Gamma.Key, clip.Values.Keys);
        Assert.Contains(Catalog.Mode.Key, clip.Values.Keys);
    }

    [Fact]
    public void Copy_AllFormats_ExcludesMixedSettings()
    {
        var (session, _) = Create();
        var clip = session.Copy(SettingsScope.AllFormats);
        Assert.DoesNotContain(Catalog.Transparency.Key, clip.Values.Keys); // mixed at defaults
        Assert.DoesNotContain(Catalog.Mode.Key, clip.Values.Keys);         // jpeg, png... default to the system provider
        Assert.Contains(Catalog.SmallIcons.Key, clip.Values.Keys);
        Assert.Contains(Catalog.Alpha.Key, clip.Values.Keys); // same default in every supporting format -> not mixed
    }

    [Fact]
    public void CopyPaste_ExtensionToExtension_CreatesOnlyDifferingOverrides()
    {
        var (session, _) = Create();
        session.Write(Psb, Catalog.Mode, Values.Generation.Disabled);
        var clip = session.Copy(Psb);
        Assert.Equal(Values.Generation.Disabled, clip.Values[Catalog.Mode.Key]);

        session.Paste(PsdExt, clip);
        var o = session.Working.Overrides[".psd"];
        Assert.Single(o);
        Assert.Equal(Values.Generation.Disabled, o[Catalog.Mode.Key]);
    }

    [Fact]
    public void Paste_IgnoresUnknownKeys_InvalidValues_AndGlobalSettings()
    {
        var (session, _) = Create();
        var clip = new SettingsClipboard("x", new Dictionary<string, object>
        {
            ["nope"] = 1,
            [Catalog.Mode.Key] = 42,                // invalid choice
            [Catalog.Enabled.Key] = 0,              // global setting
            [Catalog.Adornment.Key] = Values.Adornment.None,
        });
        var pasted = session.Paste(Psd, clip);
        Assert.Equal(1, pasted);
        Assert.Equal(Values.Adornment.None, session.Working.Formats["psd"][Catalog.Adornment.Key]);
        Assert.Equal(Values.Generation.Enabled, session.Working.Formats["psd"][Catalog.Mode.Key]);
        Assert.Equal(1, session.Working.Global[Catalog.Enabled.Key]);
    }

    // ------------------------------------------------------------------ validation

    public static TheoryData<string, object> InvalidFormatValues => new()
    {
        { "mode", 99 },
        { "mode", "1" },
        { "mode", 1L },
        { "showAssocIcon", 2 },
        { "showAssocIcon", true },
        { "canvasColor", "red" },
        { "canvasColor", 0xFFFFFFFFu },
        { "checkerColors", "nope" },
        { "checkerColors", 5 },
        { "customIconPath", 3 },
        { "maxFileSizeMB", 75 },
        { "gamma", 200 },
    };

    [Theory]
    [MemberData(nameof(InvalidFormatValues))]
    public void Write_InvalidValue_ThrowsArgumentException_AndLeavesStateUnchanged(string key, object value)
    {
        var (session, _) = Create();
        var setting = Catalog.FindSetting(key)!;
        var raised = 0;
        session.Changed += (_, _) => raised++;

        var ex = Assert.Throws<ArgumentException>(() => session.Write(Psd, setting, value));
        Assert.Equal("value", ex.ParamName);
        Assert.Throws<ArgumentException>(() => session.Write(Psb, setting, value));
        Assert.Throws<ArgumentException>(() => session.Write(SettingsScope.AllFormats, setting, value));

        Assert.Equal(0, raised);
        Assert.False(session.IsDirty);
        Assert.Empty(session.Working.Overrides);
    }

    [Fact]
    public void WriteGlobal_InvalidValue_Throws()
    {
        var (session, _) = Create();
        Assert.Throws<ArgumentException>(() => session.WriteGlobal(Catalog.Enabled, 2));
        Assert.Throws<ArgumentException>(() => session.WriteGlobal(Catalog.BlockedFolders, "C:\\x"));
        Assert.Throws<ArgumentException>(() => session.WriteGlobal(Catalog.CheckerAnchor, 9));
        Assert.Throws<ArgumentException>(() => session.WriteGlobal(Catalog.ContextMenu, null!));
        Assert.False(session.IsDirty);
    }

    // ------------------------------------------------------------------ affected extensions

    [Fact]
    public void GetAffectedExtensions_FormatChange_ReturnsThatFormatsExtensions()
    {
        var (session, _) = Create();
        session.Write(SettingsScope.ForFormat(Catalog.Eps), Catalog.Gamma, 220);
        Assert.Equal(new[] { ".eps", ".epsf", ".epsi", ".ps" }, session.GetAffectedExtensions());
    }

    [Fact]
    public void GetAffectedExtensions_ExtensionChange_ReturnsOnlyThatExtension()
    {
        var (session, _) = Create();
        session.Write(Psb, Catalog.Mode, Values.Generation.Disabled);
        Assert.Equal(new[] { ".psb" }, session.GetAffectedExtensions());
    }

    [Fact]
    public void GetAffectedExtensions_FormatChangeMaskedByExistingOverride_ExcludesThatExtension()
    {
        var stored = SettingsSnapshot.CreateDefaults();
        stored.Overrides[".psb"] = new Dictionary<string, object> { [Catalog.Mode.Key] = Values.Generation.Disabled };
        var (session, _) = Create(stored);

        // psd -> Disabled; the .psb override (Disabled) is cleared, but .psb's effective value doesn't change.
        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);

        Assert.Equal(new[] { ".psd", ".pdd", ".psdt" }, session.GetAffectedExtensions());
        Assert.True(session.IsDirty);
    }

    [Fact]
    public void GetAffectedExtensions_GlobalChange_ReturnsAllExtensions()
    {
        var (session, _) = Create();
        session.WriteGlobal(Catalog.ForceDesktopTransparency, 1);
        Assert.Equal(Catalog.AllExtensions, session.GetAffectedExtensions());
    }

    [Fact]
    public void GetAffectedExtensions_Empty_AfterRevertingEdit()
    {
        var (session, _) = Create();
        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);
        session.Write(Psd, Catalog.Mode, Values.Generation.Enabled);
        Assert.Empty(session.GetAffectedExtensions());
    }

    // ------------------------------------------------------------------ modified from default

    [Fact]
    public void IsModifiedFromDefault_UsesPerFormatDefaults()
    {
        var (session, _) = Create();
        var ai = SettingsScope.ForFormat(Catalog.Illustrator);
        Assert.False(session.IsModifiedFromDefault(ai, Catalog.Transparency));
        Assert.False(session.IsModifiedFromDefault(SettingsScope.AllFormats, Catalog.Transparency));

        // Transparent equals the catalog default but not ai's default.
        session.Write(ai, Catalog.Transparency, Values.Transparency.Transparent);
        Assert.True(session.IsModifiedFromDefault(ai, Catalog.Transparency));
        Assert.True(session.IsModifiedFromDefault(SettingsScope.AllFormats, Catalog.Transparency));
        Assert.False(session.IsModifiedFromDefault(Psd, Catalog.Transparency));
    }

    [Fact]
    public void IsModifiedFromDefault_ExtensionScope_MeansHasOverride()
    {
        var (session, _) = Create();
        Assert.False(session.IsModifiedFromDefault(Psb, Catalog.Mode));

        session.Write(Psd, Catalog.Mode, Values.Generation.Fallback);
        // Format modified, extension inherits -> extension itself is not "modified".
        Assert.False(session.IsModifiedFromDefault(Psb, Catalog.Mode));
        Assert.True(session.IsModifiedFromDefault(Psd, Catalog.Mode));

        session.Reset(Psd, Catalog.Mode);
        session.Write(Psb, Catalog.Mode, Values.Generation.Disabled);
        Assert.True(session.IsModifiedFromDefault(Psb, Catalog.Mode));
        Assert.False(session.IsModifiedFromDefault(PsdExt, Catalog.Mode));
        // Format value is default but an extension overrides -> format reports modified.
        Assert.True(session.IsModifiedFromDefault(Psd, Catalog.Mode));
        Assert.True(session.IsModifiedFromDefault(SettingsScope.AllFormats, Catalog.Mode));
    }

    [Fact]
    public void IsModifiedFromDefault_IsAgainstDefaults_NotApplied()
    {
        var (session, _) = Create();
        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);
        session.Apply();
        Assert.True(session.IsModifiedFromDefault(Psd, Catalog.Mode));
        Assert.False(session.IsPending(Psd, Catalog.Mode));
    }

    [Fact]
    public void IsModifiedFromDefault_UnsupportedSetting_IsFalse()
    {
        var (session, _) = Create();
        Assert.False(session.IsModifiedFromDefault(SettingsScope.ForFormat(Catalog.Svg), Catalog.Alpha));
    }

    // ------------------------------------------------------------------ apply one global now (Jump List)

    [Fact]
    public void ApplyGlobalNow_SavesOnlyThatSetting_AndKeepsOtherEditsPending()
    {
        var (session, store) = Create();
        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);
        var raised = 0;
        session.Changed += (_, _) => raised++;

        session.ApplyGlobalNow(Catalog.Enabled, 0);

        Assert.Equal(1, raised);
        Assert.Equal(1, store.SaveCount);
        Assert.Equal(0, store.Stored.Global[Catalog.Enabled.Key]);
        Assert.Equal(Values.Generation.Enabled, store.Stored.Formats["psd"][Catalog.Mode.Key]);
        Assert.Equal(0, session.Applied.Global[Catalog.Enabled.Key]);
        Assert.False(session.IsGlobalPending(Catalog.Enabled));
        var change = Assert.Single(session.GetChanges());
        Assert.Same(Catalog.Mode, change.Setting);
    }

    [Fact]
    public void ApplyGlobalNow_UndoDoesNotRevertTheAppliedValue()
    {
        var (session, _) = Create();
        session.Write(Psd, Catalog.Mode, Values.Generation.Disabled);
        session.ApplyGlobalNow(Catalog.Enabled, 0);

        Assert.True(session.Undo());

        Assert.Equal(0, session.Working.Global[Catalog.Enabled.Key]);
        Assert.False(session.IsDirty);
    }

    [Fact]
    public void ApplyGlobalNow_InvalidValue_Throws_AndSavesNothing()
    {
        var (session, store) = Create();
        Assert.Throws<ArgumentException>(() => session.ApplyGlobalNow(Catalog.Enabled, "sí"));
        Assert.Equal(0, store.SaveCount);
    }
}
