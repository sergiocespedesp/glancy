using Glancy.Core.Shell;

namespace Glancy.Core.Tests;

public class IconExtractorTests
{
    private static readonly string Shell32 = Path.Combine(Environment.SystemDirectory, "shell32.dll");

    [Theory]
    [InlineData(@"C:\Windows\System32\shell32.dll,5", @"C:\Windows\System32\shell32.dll", 5)]
    [InlineData(@"C:\Windows\System32\shell32.dll", @"C:\Windows\System32\shell32.dll", 0)]
    [InlineData(@"C:\icons\a.ico,-101", @"C:\icons\a.ico", -101)]
    [InlineData(@"""C:\My Icons\a.dll"",3", @"C:\My Icons\a.dll", 3)]
    [InlineData(@"""C:\My Icons\a.ico""", @"C:\My Icons\a.ico", 0)]
    [InlineData(@"  C:\x.dll , 7", @"C:\x.dll", 7)]
    [InlineData(@"C:\a,b\x.dll,2", @"C:\a,b\x.dll", 2)]
    [InlineData(@"C:\a,b\x.dll", @"C:\a,b\x.dll", 0)]
    [InlineData(@"x.dll,abc", @"x.dll,abc", 0)]
    [InlineData(@",5", @",5", 0)]
    [InlineData("", "", 0)]
    public void ParseLocation(string input, string path, int index)
    {
        var (p, i) = IconExtractor.ParseLocation(input);
        Assert.Equal(path, p);
        Assert.Equal(index, i);
    }

    [Theory]
    [InlineData(@"C:\Windows\System32\shell32.dll", 0)]
    [InlineData(@"C:\a,b\x.dll", 12)]
    [InlineData(@"C:\icons\a.ico", -3)]
    public void FormatLocation_RoundTripsThroughParse(string path, int index)
    {
        var text = IconExtractor.FormatLocation(path, index);
        Assert.Equal($"{path},{index}", text);
        Assert.Equal((path, index), IconExtractor.ParseLocation(text));
    }

    [Fact]
    public void CountIcons_MissingFile_IsZero()
    {
        Assert.Equal(0, IconExtractor.CountIcons(@"C:\does\not\exist\nope.dll"));
        Assert.Empty(IconExtractor.ExtractAll(@"C:\does\not\exist\nope.dll", 32));
    }

    [Fact]
    public void ExtractAll_Shell32_Size32()
    {
        Assert.True(File.Exists(Shell32));
        var icons = IconExtractor.ExtractAll(Shell32, 32);
        Assert.NotEmpty(icons);
        Assert.True(icons.Count <= IconExtractor.MaxIcons);
        foreach (var icon in icons)
        {
            Assert.Equal(32, icon.Width);
            Assert.Equal(32, icon.Height);
            Assert.Equal(icon.Width * icon.Height * 4, icon.PremultipliedBgra.Length);
        }
        Assert.Equal(icons.Select(i => i.Index).Distinct().Count(), icons.Count);
    }

    [Fact]
    public void Extract_Shell32_FirstIcon_IsPremultiplied()
    {
        var icon = IconExtractor.Extract(Shell32, 0, 32);
        Assert.NotNull(icon);
        var px = icon!.PremultipliedBgra;
        for (var p = 0; p < px.Length; p += 4)
        {
            var a = px[p + 3];
            Assert.True(px[p] <= a && px[p + 1] <= a && px[p + 2] <= a, $"pixel {p / 4} not premultiplied");
        }
        Assert.Contains(Enumerable.Range(0, px.Length / 4), p => px[p * 4 + 3] != 0); // not fully transparent
    }
}

public class EngineStatusServiceTests
{
    [Fact]
    public void GetStatus_ReportsOneHandlerPerExtension()
    {
        // Works whether or not the engine is installed on this machine.
        var status = EngineStatusService.GetStatus();
        Assert.Equal(status.DllPath is not null && File.Exists(Environment.ExpandEnvironmentVariables(status.DllPath)),
            status.IsInstalled);
        Assert.Equal(Catalog.AllExtensions, status.Handlers.Select(h => h.Extension));
        Assert.All(status.Handlers, h => Assert.False(string.IsNullOrWhiteSpace(h.Describe())));
        Assert.All(status.Handlers, h => Assert.Equal(h.Owner == HandlerOwner.None, h.Clsid is null));
        Assert.Equal(status.Handlers.Count(h => h.Owner == HandlerOwner.Glancy), status.ActiveExtensionCount);
    }

    [Fact]
    public void GetHandler_UnknownExtension_HasNoHandler()
    {
        var info = EngineStatusService.GetHandler(".glancy-unassociated");
        Assert.Equal(HandlerOwner.None, info.Owner);
        Assert.Null(info.Clsid);
    }

    [Fact]
    public void CandidateKeys_ProgIdsBeforeExtension()
    {
        Assert.Equal(
            ["Photoshop.Image.27", "SageThumbsImage.psd", ".psd", @"SystemFileAssociations\.psd", @"SystemFileAssociations\image"],
            EngineStatusService.CandidateKeys(".psd", "Photoshop.Image.27", "SageThumbsImage.psd", "Image"));
    }

    [Fact]
    public void CandidateKeys_SkipsMissingAndDuplicateEntries()
    {
        Assert.Equal([".svgz", @"SystemFileAssociations\.svgz"], EngineStatusService.CandidateKeys(".svgz", null, " ", null));
        Assert.Equal(["SVGFILE", ".svg", @"SystemFileAssociations\.svg"],
            EngineStatusService.CandidateKeys(".svg", "SVGFILE", "svgfile", ""));
    }

    [Fact]
    public void HandlerInfo_Describe()
    {
        Assert.Equal("Glancy", new ThumbnailHandlerInfo(".psd", HandlerOwner.Glancy, AppInfo.ThumbnailProviderClsid, "Glancy").Describe());
        Assert.Equal("None (file type icon)", new ThumbnailHandlerInfo(".psd", HandlerOwner.None, null, null).Describe());
        Assert.Equal("Foo", new ThumbnailHandlerInfo(".psd", HandlerOwner.Other, Guid.Empty, "Foo").Describe());
        var g = Guid.NewGuid();
        Assert.Equal($"Other provider ({g:B})", new ThumbnailHandlerInfo(".psd", HandlerOwner.Other, g, " ").Describe());
    }

    [Fact]
    public void EngineStatus_IsActiveFor()
    {
        var s = new EngineStatus(true, "x.dll",
        [
            new ThumbnailHandlerInfo(".psd", HandlerOwner.Glancy, AppInfo.ThumbnailProviderClsid, "Glancy"),
            new ThumbnailHandlerInfo(".pdf", HandlerOwner.Other, Guid.Empty, "Other"),
        ]);
        Assert.True(s.IsActiveFor(".psd"));
        Assert.False(s.IsActiveFor(".pdf"));
        Assert.False(s.IsActiveFor(".svg"));
        Assert.Equal(1, s.ActiveExtensionCount);
    }
}
