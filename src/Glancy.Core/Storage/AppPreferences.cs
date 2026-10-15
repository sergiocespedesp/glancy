using Microsoft.Win32;

namespace Glancy.Core.Storage;

public enum AppTheme { System = 0, Light = 1, Dark = 2 }

/// <summary>Preferences of the settings app itself. They apply immediately and never affect thumbnails.</summary>
public sealed class AppPreferences(string keyPath = AppInfo.AppPreferencesKeyPath)
{
    public AppTheme Theme
    {
        get => (AppTheme)Math.Clamp(ReadInt(nameof(Theme), 0), 0, 2);
        set => WriteInt(nameof(Theme), (int)value);
    }

    public bool AlwaysOnTop
    {
        get => ReadInt(nameof(AlwaysOnTop), 0) != 0;
        set => WriteInt(nameof(AlwaysOnTop), value ? 1 : 0);
    }

    public bool ShowTips
    {
        get => ReadInt(nameof(ShowTips), 1) != 0;
        set => WriteInt(nameof(ShowTips), value ? 1 : 0);
    }

    /// <summary>UI language code (see <see cref="Localization.AppLanguage"/>); empty follows Windows. Applies on restart.</summary>
    public string Language
    {
        get => ReadString(nameof(Language)) ?? string.Empty;
        set => WriteString(nameof(Language), value);
    }

    public bool FirstRunCompleted
    {
        get => ReadInt(nameof(FirstRunCompleted), 0) != 0;
        set => WriteInt(nameof(FirstRunCompleted), value ? 1 : 0);
    }

    /// <summary>Last window bounds in device-independent pixels: x, y, width, height.</summary>
    public int[]? WindowBounds
    {
        get
        {
            using var k = Registry.CurrentUser.OpenSubKey(keyPath);
            return k?.GetValue(nameof(WindowBounds)) is string s
                   && s.Split(',').Select(p => int.TryParse(p, out var v) ? v : (int?)null).ToArray() is { Length: 4 } parts
                   && parts.All(p => p.HasValue)
                ? parts.Select(p => p!.Value).ToArray()
                : null;
        }
        set
        {
            using var k = Registry.CurrentUser.CreateSubKey(keyPath, writable: true);
            if (value is { Length: 4 }) k.SetValue(nameof(WindowBounds), string.Join(',', value));
            else k.DeleteValue(nameof(WindowBounds), throwOnMissingValue: false);
        }
    }

    private int ReadInt(string name, int fallback)
    {
        try
        {
            using var k = Registry.CurrentUser.OpenSubKey(keyPath);
            return k?.GetValue(name) is int i ? i : fallback;
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or System.Security.SecurityException)
        {
            return fallback;
        }
    }

    private string? ReadString(string name)
    {
        try
        {
            using var k = Registry.CurrentUser.OpenSubKey(keyPath);
            return k?.GetValue(name) as string;
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or System.Security.SecurityException)
        {
            return null;
        }
    }

    private void WriteString(string name, string value)
    {
        try
        {
            using var k = Registry.CurrentUser.CreateSubKey(keyPath, writable: true);
            if (string.IsNullOrEmpty(value)) k.DeleteValue(name, throwOnMissingValue: false);
            else k.SetValue(name, value, RegistryValueKind.String);
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or System.Security.SecurityException)
        {
            // Preferences are cosmetic; failing to persist them must never interrupt the user.
        }
    }

    private void WriteInt(string name, int value)
    {
        try
        {
            using var k = Registry.CurrentUser.CreateSubKey(keyPath, writable: true);
            k.SetValue(name, value, RegistryValueKind.DWord);
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or System.Security.SecurityException)
        {
            // Preferences are cosmetic; failing to persist them must never interrupt the user.
        }
    }
}
