using Glancy.Core.Shell;

namespace Glancy.Core.Tests;

public class OverlayIconTests
{
    private const string Photoshop = @"C:\Adobe\Photoshop.exe,0";

    [Fact]
    public void AppIcon_PrefersTheIconReference()
    {
        const string store = "@{Microsoft.Windows.Photos_8wekyb3d8bbwe?ms-resource://Microsoft.Windows.Photos/Logo.png}";
        Assert.Equal(store, OverlayIcon.AppIconFrom(store, @"C:\x.exe"));
        Assert.Equal(Photoshop, OverlayIcon.AppIconFrom(null, @"C:\Adobe\Photoshop.exe"));
        Assert.Equal(Photoshop, OverlayIcon.AppIconFrom(" ", @"C:\Adobe\Photoshop.exe"));
        Assert.Null(OverlayIcon.AppIconFrom(null, null));
        // Windows may give the reference as a bare path.
        Assert.Equal(Photoshop, OverlayIcon.AppIconFrom(@"C:\Adobe\Photoshop.exe", null));
        Assert.Equal(@"C:\a,b\x.exe,0", OverlayIcon.AppIconFrom(@"C:\a,b\x.exe", null));
        Assert.Equal(@"C:\x.dll,-101", OverlayIcon.AppIconFrom(@"C:\x.dll,-101", null));
    }

    [Fact]
    public void Resolve_ShowsTheAppIcon()
    {
        Assert.Equal(new OverlayPreview(Photoshop, OverlayNote.None),
            OverlayIcon.Resolve(showAppIcon: true, customIcon: null, unassociated: false, fileTypeSetsOwn: false, appIcon: Photoshop));
    }

    [Fact]
    public void Resolve_Hidden_ShowsNothing()
    {
        Assert.Equal(new OverlayPreview(null, OverlayNote.None),
            OverlayIcon.Resolve(showAppIcon: false, customIcon: @"C:\i.ico,0", unassociated: false, fileTypeSetsOwn: false, appIcon: Photoshop));
    }

    [Fact]
    public void Resolve_CustomIcon_WinsAndWorksWithoutAnApp()
    {
        Assert.Equal(new OverlayPreview(@"C:\i.ico,0", OverlayNote.None),
            OverlayIcon.Resolve(showAppIcon: true, customIcon: @"C:\i.ico,0", unassociated: true, fileTypeSetsOwn: false, appIcon: null));
    }

    [Fact]
    public void Resolve_NoApp_ExplainsWhy()
    {
        Assert.Equal(new OverlayPreview(null, OverlayNote.NoApp),
            OverlayIcon.Resolve(showAppIcon: true, customIcon: "  ", unassociated: true, fileTypeSetsOwn: false, appIcon: null));
        Assert.Equal(new OverlayPreview(null, OverlayNote.NoApp),
            OverlayIcon.Resolve(showAppIcon: true, customIcon: null, unassociated: false, fileTypeSetsOwn: false, appIcon: null));
    }

    [Fact]
    public void Resolve_FileTypeWithItsOwnOverlay_IgnoresTheSetting()
    {
        Assert.Equal(new OverlayPreview(null, OverlayNote.FileTypeOverrides),
            OverlayIcon.Resolve(showAppIcon: true, customIcon: @"C:\i.ico,0", unassociated: false, fileTypeSetsOwn: true, appIcon: Photoshop));
    }

    [Theory]
    [InlineData(null, true)]
    [InlineData("", true)]
    [InlineData("Unknown", true)]
    [InlineData(".tga", true)]
    [InlineData("Adobe.Photoshop.27", false)]
    public void IsUnassociated(string? progId, bool expected) =>
        Assert.Equal(expected, OverlayIcon.IsUnassociated(".tga", progId));

    [Fact]
    public void ResolveIndirect_UnknownPackage_IsNull() =>
        Assert.Null(IconExtractor.ResolveIndirect("@{Glancy.NoSuchPackage_1.0.0.0_x64__0000000000000?ms-resource://x/y.png}"));

    [Fact]
    public void SyncArguments_QuoteTheDll() =>
        Assert.Equal(@"/s /n /i:sync ""C:\Program Files\Glancy\Glancy.Thumbnails.dll""",
            OverlayIcon.SyncArguments(@"C:\Program Files\Glancy\Glancy.Thumbnails.dll"));
}
