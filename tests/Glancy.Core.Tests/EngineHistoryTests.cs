using Glancy.Core.Shell;

namespace Glancy.Core.Tests;

public class EngineHistoryTests
{
    [Fact]
    public void ReadRecent_ReturnsNewestFirst_AndSkipsMalformedLines()
    {
        var file = Path.Combine(Path.GetTempPath(), $"glancy-history-{Guid.NewGuid():N}.log");
        try
        {
            File.WriteAllLines(file,
            [
                "2026-10-14T01:00:00Z\tok\t42\tC:\\a.psd",
                "basura",
                "2026-10-14T01:00:05Z\terror\t9\tC:\\b.pdf\tPDFium: archivo dañado",
                "2026-10-14T01:00:09Z\tok\tNaN\tC:\\c.svg",
            ]);

            var entries = EngineHistory.ReadRecent(10, file);

            Assert.Equal(2, entries.Count);
            Assert.Equal(@"C:\b.pdf", entries[0].Path);
            Assert.False(entries[0].Succeeded);
            Assert.Equal("PDFium: archivo dañado", entries[0].Detail);
            Assert.True(entries[1].Succeeded);
            Assert.Equal(42, entries[1].Milliseconds);
        }
        finally
        {
            File.Delete(file);
        }
    }

    [Fact]
    public void ReadRecent_MissingFile_IsEmpty() =>
        Assert.Empty(EngineHistory.ReadRecent(5, Path.Combine(Path.GetTempPath(), $"missing-{Guid.NewGuid():N}.log")));
}
