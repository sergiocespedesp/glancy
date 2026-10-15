namespace Glancy.Core.Tests;

public class UndoTests
{
    private static SettingsSession Create() => new(new InMemorySettingsStore());

    [Fact]
    public void Undo_RevertsEditsOneAtATime()
    {
        var session = Create();
        var psd = SettingsScope.ForFormat(Catalog.Photoshop);
        session.Write(psd, Catalog.Transparency, Values.Transparency.Checkerboard);
        session.Write(psd, Catalog.ScaleMode, Values.Scale.ScaleToFit);

        Assert.True(session.Undo());
        Assert.Equal(Values.Scale.ActualSize, session.Read(psd, Catalog.ScaleMode).Value);
        Assert.Equal(Values.Transparency.Checkerboard, session.Read(psd, Catalog.Transparency).Value);

        Assert.True(session.Undo());
        Assert.False(session.IsDirty);
        Assert.False(session.CanUndo);
        Assert.False(session.Undo());
    }

    [Fact]
    public void NoOpWrite_DoesNotCreateUndoStep_OrRaiseChanged()
    {
        var session = Create();
        var raised = 0;
        session.Changed += (_, _) => raised++;
        session.Write(SettingsScope.ForFormat(Catalog.Photoshop), Catalog.Transparency, Values.Transparency.Transparent);
        Assert.False(session.CanUndo);
        Assert.Equal(0, raised);
    }

    [Fact]
    public void ApplyAndDiscard_ClearUndoHistory()
    {
        var session = Create();
        session.WriteGlobal(Catalog.DebugLog, 1);
        session.Apply();
        Assert.False(session.CanUndo);

        session.WriteGlobal(Catalog.DebugLog, 0);
        session.Discard();
        Assert.False(session.CanUndo);
    }

    [Fact]
    public void Undo_RevertsPasteAndImportAsSingleSteps()
    {
        var session = Create();
        var clip = new SettingsClipboard("x", new Dictionary<string, object>
        {
            [Catalog.Transparency.Key] = Values.Transparency.Checkerboard,
            [Catalog.ScaleMode.Key] = Values.Scale.ScaleToFit,
        });
        session.Paste(SettingsScope.ForFormat(Catalog.Svg), clip);
        session.Undo();
        Assert.False(session.IsDirty);

        var imported = SettingsSnapshot.CreateDefaults();
        imported.Global[Catalog.DebugLog.Key] = 1;
        session.LoadWorking(imported);
        session.Undo();
        Assert.False(session.IsDirty);
    }
}
