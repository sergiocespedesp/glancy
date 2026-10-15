namespace Glancy.Core.Tests;

public class CheckerColorsTests
{
    [Fact]
    public void TryParse_Valid()
    {
        Assert.True(CheckerColors.TryParse("FFFFFFFF;FFCCCCCC", out var c));
        Assert.Equal(0xFFFFFFFFu, c.Light);
        Assert.Equal(0xFFCCCCCCu, c.Dark);

        Assert.True(CheckerColors.TryParse("ff3c3c3c;ff282828", out c));
        Assert.Equal(new CheckerColors(0xFF3C3C3C, 0xFF282828), c);

        Assert.True(CheckerColors.TryParse("FF;0", out c));
        Assert.Equal(new CheckerColors(0xFF, 0), c);
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("FFFFFFFF")]
    [InlineData("FFFFFFFF;")]
    [InlineData(";FFFFFFFF")]
    [InlineData("A;B;C")]
    [InlineData("GGGGGGGG;FFFFFFFF")]
    [InlineData("0xFF;0xFF")]
    [InlineData("1FFFFFFFF;00000000")]
    [InlineData("FFFFFFFF,FFCCCCCC")]
    public void TryParse_Invalid(string? text)
    {
        Assert.False(CheckerColors.TryParse(text, out var c));
        Assert.Equal(default, c);
    }

    [Fact]
    public void ToString_IsUppercasePaddedHex_AndRoundTrips()
    {
        var c = new CheckerColors(0xFF, 0xABCDEF01);
        Assert.Equal("000000FF;ABCDEF01", c.ToString());
        Assert.True(CheckerColors.TryParse(c.ToString(), out var back));
        Assert.Equal(c, back);
        foreach (var entry in Catalog.DefaultCheckerLibrary)
        {
            Assert.True(CheckerColors.TryParse(entry, out var e));
            Assert.Equal(entry, e.ToString());
        }
    }

    [Fact]
    public void Swapped_ExchangesColors()
    {
        var c = new CheckerColors(1, 2);
        Assert.Equal(new CheckerColors(2, 1), c.Swapped());
        Assert.Equal(c, c.Swapped().Swapped());
    }
}

public class SettingDefinitionTests
{
    [Fact]
    public void IsValid_Choice()
    {
        Assert.True(Catalog.Mode.IsValid(Values.Generation.Fallback));
        Assert.True(Catalog.MaxFileSize.IsValid(2048));
        Assert.False(Catalog.MaxFileSize.IsValid(2047));
        Assert.False(Catalog.Mode.IsValid(3));
        Assert.False(Catalog.Mode.IsValid("1"));
        Assert.False(Catalog.Mode.IsValid(null));
        Assert.False(Catalog.Mode.IsValid(1L));
    }

    [Fact]
    public void IsValid_Toggle()
    {
        Assert.True(Catalog.Enabled.IsValid(0));
        Assert.True(Catalog.Enabled.IsValid(1));
        Assert.False(Catalog.Enabled.IsValid(2));
        Assert.False(Catalog.Enabled.IsValid(-1));
        Assert.False(Catalog.Enabled.IsValid(true));
        Assert.False(Catalog.Enabled.IsValid(null));
    }

    [Fact]
    public void IsValid_Color()
    {
        Assert.True(Catalog.CanvasColor.IsValid(0));
        Assert.True(Catalog.CanvasColor.IsValid(-1));
        Assert.True(Catalog.CanvasColor.IsValid(int.MaxValue));
        Assert.False(Catalog.CanvasColor.IsValid(0xFFFFFFFFu));
        Assert.False(Catalog.CanvasColor.IsValid("#FFFFFF"));
        Assert.False(Catalog.CanvasColor.IsValid(null));
    }

    [Fact]
    public void IsValid_Checker()
    {
        Assert.True(Catalog.Checker.IsValid("FFFFFFFF;FF000000"));
        Assert.False(Catalog.Checker.IsValid("FFFFFFFF"));
        Assert.False(Catalog.Checker.IsValid(5));
        Assert.False(Catalog.Checker.IsValid(null));
    }

    [Fact]
    public void IsValid_Text()
    {
        Assert.True(Catalog.CustomIconPath.IsValid(""));
        Assert.True(Catalog.CustomIconPath.IsValid(@"C:\x.ico,0"));
        Assert.False(Catalog.CustomIconPath.IsValid(1));
        Assert.False(Catalog.CustomIconPath.IsValid(null));
        Assert.False(Catalog.CustomIconPath.IsValid(new[] { "a" }));
    }

    [Fact]
    public void IsValid_List()
    {
        Assert.True(Catalog.BlockedFolders.IsValid(Array.Empty<string>()));
        Assert.True(Catalog.BlockedFolders.IsValid(new[] { "a", "b" }));
        Assert.False(Catalog.BlockedFolders.IsValid("a"));
        Assert.False(Catalog.BlockedFolders.IsValid(new List<string> { "a" }));
        Assert.False(Catalog.BlockedFolders.IsValid(null));
    }

    [Fact]
    public void FormatValue_Choice()
    {
        Assert.Equal("System provider", Catalog.Mode.FormatValue(Values.Generation.Fallback));
        Assert.Equal("1 GB", Catalog.MaxFileSize.FormatValue(1024));
        Assert.Equal("123", Catalog.MaxFileSize.FormatValue(123)); // unknown option -> raw value
    }

    [Fact]
    public void FormatValue_Toggle()
    {
        Assert.Equal("On", Catalog.Enabled.FormatValue(1));
        Assert.Equal("Off", Catalog.Enabled.FormatValue(0));
    }

    [Fact]
    public void FormatValue_Color()
    {
        Assert.Equal("#FFFFFFFF", Catalog.CanvasColor.FormatValue(-1));
        Assert.Equal("#0000FF00", Catalog.CanvasColor.FormatValue(0x0000FF00));
    }

    [Fact]
    public void FormatValue_Checker()
    {
        Assert.Equal("FFFFFFFF;FFCCCCCC", Catalog.Checker.FormatValue("ffffffff;ffcccccc"));
        Assert.Equal("garbage", Catalog.Checker.FormatValue("garbage"));
    }

    [Fact]
    public void FormatValue_Text()
    {
        Assert.Equal("(none)", Catalog.CustomIconPath.FormatValue(""));
        Assert.Equal(@"C:\x.ico,1", Catalog.CustomIconPath.FormatValue(@"C:\x.ico,1"));
    }

    [Fact]
    public void FormatValue_List()
    {
        Assert.Equal("(empty)", Catalog.BlockedFolders.FormatValue(Array.Empty<string>()));
        Assert.Equal("3 item(s)", Catalog.CheckerLibrary.FormatValue(Catalog.DefaultCheckerLibrary));
    }

    [Fact]
    public void FormatValue_Null_MeansInherited()
    {
        foreach (var s in Catalog.FormatSettings.Concat(Catalog.GlobalSettings))
            Assert.Equal("Inherited from the format", s.FormatValue(null));
    }

    [Fact]
    public void FormatValue_EveryDefault_IsNonEmpty()
    {
        foreach (var s in Catalog.FormatSettings.Concat(Catalog.GlobalSettings))
            Assert.False(string.IsNullOrEmpty(s.FormatValue(s.DefaultValue)), s.Key);
    }

    [Fact]
    public void FindOption_And_IsSupportedBy()
    {
        Assert.Equal("2.2", Catalog.Gamma.FindOption(220)!.Label);
        Assert.Null(Catalog.Gamma.FindOption(221));
        Assert.Null(Catalog.Gamma.FindOption("220"));
        Assert.True(Catalog.Gamma.IsSupportedBy(Catalog.Pdf));
        Assert.False(Catalog.Gamma.IsSupportedBy(Catalog.Svg));
        Assert.True(Catalog.Mode.IsSupportedBy(Catalog.Svg));
        Assert.False(Catalog.MaxPixels.IsSupportedBy(Catalog.Illustrator));
        Assert.True(Catalog.MaxPixels.IsSupportedBy(Catalog.Photoshop));
    }

    [Fact]
    public void SettingReading_Accessors()
    {
        Assert.Equal(5, new SettingReading(5, false, false).AsInt());
        Assert.Equal(-1, new SettingReading(null, true, false).AsInt(-1));
        Assert.True(new SettingReading(1, false, false).AsBool());
        Assert.False(new SettingReading(0, false, false).AsBool());
        Assert.False(new SettingReading("1", false, false).AsBool());
        Assert.Equal("x", new SettingReading("x", false, false).AsString());
        Assert.Equal("", new SettingReading(3, false, false).AsString());
    }
}
