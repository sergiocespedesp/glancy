using Glancy.Core.Storage;
using Microsoft.Win32;

namespace Glancy.Core.Tests;

// Shares the HKCU\Software\Glancy.Tests parent key with other registry tests: run them sequentially.
[Collection("Registry")]
public sealed class AppPreferencesTests : IDisposable
{
    private readonly TestRegistryKey _key = new();

    public void Dispose() => _key.Dispose();

    [Fact]
    public void Defaults_WhenKeyMissing()
    {
        var p = new AppPreferences(_key.Path);
        Assert.Equal(AppTheme.System, p.Theme);
        Assert.False(p.AlwaysOnTop);
        Assert.True(p.ShowTips);
        Assert.False(p.FirstRunCompleted);
        Assert.Null(p.WindowBounds);
        Assert.Null(_key.Open()); // reading must not create the key
    }

    [Fact]
    public void RoundTrip_AllPreferences()
    {
        var p = new AppPreferences(_key.Path)
        {
            Theme = AppTheme.Dark,
            AlwaysOnTop = true,
            ShowTips = false,
            FirstRunCompleted = true,
            WindowBounds = [10, -20, 800, 600],
        };

        var q = new AppPreferences(_key.Path);
        Assert.Equal(AppTheme.Dark, q.Theme);
        Assert.True(q.AlwaysOnTop);
        Assert.False(q.ShowTips);
        Assert.True(q.FirstRunCompleted);
        Assert.Equal(new[] { 10, -20, 800, 600 }, q.WindowBounds);

        q.Theme = AppTheme.Light;
        q.AlwaysOnTop = false;
        q.ShowTips = true;
        Assert.Equal(AppTheme.Light, p.Theme);
        Assert.False(p.AlwaysOnTop);
        Assert.True(p.ShowTips);

        using var k = _key.Open()!;
        Assert.Equal(RegistryValueKind.DWord, k.GetValueKind(nameof(AppPreferences.Theme)));
        Assert.Equal("10,-20,800,600", k.GetValue(nameof(AppPreferences.WindowBounds)));
    }

    [Fact]
    public void WindowBounds_NullOrWrongLength_DeletesValue()
    {
        var p = new AppPreferences(_key.Path) { WindowBounds = [1, 2, 3, 4] };
        p.WindowBounds = null;
        Assert.Null(p.WindowBounds);

        p.WindowBounds = [1, 2, 3, 4];
        p.WindowBounds = [1, 2, 3];
        Assert.Null(p.WindowBounds);
        using var k = _key.Open()!;
        Assert.DoesNotContain(nameof(AppPreferences.WindowBounds), k.GetValueNames());
    }

    [Theory]
    [InlineData("1,2,x,4")]
    [InlineData("1,2,3")]
    [InlineData("1,2,3,4,5")]
    [InlineData("")]
    public void WindowBounds_Malformed_ReadsNull(string raw)
    {
        using (var k = Registry.CurrentUser.CreateSubKey(_key.Path, writable: true))
            k.SetValue(nameof(AppPreferences.WindowBounds), raw, RegistryValueKind.String);
        Assert.Null(new AppPreferences(_key.Path).WindowBounds);
    }

    [Fact]
    public void WrongTypeOrOutOfRange_FallsBackSafely()
    {
        using (var k = Registry.CurrentUser.CreateSubKey(_key.Path, writable: true))
        {
            k.SetValue(nameof(AppPreferences.ShowTips), "0", RegistryValueKind.String); // wrong type -> default true
            k.SetValue(nameof(AppPreferences.Theme), 7, RegistryValueKind.DWord);
            k.SetValue(nameof(AppPreferences.WindowBounds), 5, RegistryValueKind.DWord);
        }
        var p = new AppPreferences(_key.Path);
        Assert.True(p.ShowTips);
        Assert.True(Enum.IsDefined(p.Theme));
        Assert.Null(p.WindowBounds);
    }
}
