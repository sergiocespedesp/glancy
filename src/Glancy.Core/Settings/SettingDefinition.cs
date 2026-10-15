using Glancy.Core.Localization;
using Glancy.Core.Resources;

namespace Glancy.Core.Settings;

/// <summary>Where a setting lives: once for the whole system, or once per file format (overridable per extension).</summary>
public enum SettingScope { Global, Format }

/// <summary>How a setting's value is represented and edited.</summary>
public enum SettingKind
{
    /// <summary>One of a fixed set of integer options.</summary>
    Choice,
    /// <summary>0 / 1.</summary>
    Toggle,
    /// <summary>ARGB color stored as a 32-bit integer.</summary>
    Color,
    /// <summary>Two ARGB colors serialized as "AARRGGBB;AARRGGBB".</summary>
    Checker,
    /// <summary>Free text.</summary>
    Text,
    /// <summary>List of strings.</summary>
    List,
}

/// <summary>Capabilities a format may or may not have; settings that require a missing capability are hidden.</summary>
[Flags]
public enum FormatCapability
{
    None = 0,
    EmbeddedThumbnail = 1,
    PremultipliedAlpha = 2,
    Sampling = 4,
    Gamma = 8,
    PixelLimit = 16,
}

/// <summary>One choice of a <see cref="SettingKind.Choice"/> setting. Its texts come from CoreStrings in the UI language.</summary>
public sealed record SettingOption(string SettingKey, int Value)
{
    public string Label => CoreText.Get($"Setting_{SettingKey}_Option_{Value}");
    public string? Description => CoreText.Find($"Setting_{SettingKey}_Option_{Value}_Description");
}

/// <summary>
/// Describes one setting: identity, presentation and default. Values are plain <see cref="int"/>,
/// <see cref="string"/> or <see cref="string"/>[] so they map 1:1 to REG_DWORD, REG_SZ and REG_MULTI_SZ.
/// Title, description and group name are looked up by <see cref="Key"/> in the active UI language.
/// </summary>
public sealed class SettingDefinition
{
    public required string Key { get; init; }
    public required SettingScope Scope { get; init; }
    public required SettingKind Kind { get; init; }

    /// <summary>Id of the group the setting is listed under (<see cref="Catalog.Groups"/>).</summary>
    public required string GroupId { get; init; }
    public required object DefaultValue { get; init; }

    public string Title => CoreText.Get($"Setting_{Key}_Title");
    public string Description => CoreText.Get($"Setting_{Key}_Description");
    public string Group => CoreText.Get($"Group_{GroupId}");

    /// <summary>Segoe Fluent Icons glyph shown on the settings card.</summary>
    public string Glyph { get; init; } = "";

    public IReadOnlyList<SettingOption> Options { get; init; } = [];
    public FormatCapability Requires { get; init; } = FormatCapability.None;

    public bool IsSupportedBy(FormatDefinition format) =>
        Scope == SettingScope.Format && (format.Capabilities & Requires) == Requires;

    public SettingOption? FindOption(object? value) =>
        value is int i ? Options.FirstOrDefault(o => o.Value == i) : null;

    /// <summary>Returns true when <paramref name="value"/> has the right shape for this setting.</summary>
    public bool IsValid(object? value) => Kind switch
    {
        SettingKind.Choice => value is int i && Options.Any(o => o.Value == i),
        SettingKind.Toggle => value is 0 or 1,
        SettingKind.Color => value is int,
        SettingKind.Checker => value is string s && CheckerColors.TryParse(s, out _),
        SettingKind.Text => value is string,
        SettingKind.List => value is string[],
        _ => false,
    };

    /// <summary>Human readable rendering used by the pending-changes list.</summary>
    public string FormatValue(object? value) => value switch
    {
        null => CoreStrings.Value_Inherited,
        _ when Kind == SettingKind.Choice => FindOption(value)?.Label ?? value.ToString()!,
        int t when Kind == SettingKind.Toggle => t != 0 ? CoreStrings.Value_On : CoreStrings.Value_Off,
        int c when Kind == SettingKind.Color => "#" + ((uint)c).ToString("X8"),
        string s when Kind == SettingKind.Checker => CheckerColors.TryParse(s, out var cc) ? cc.ToString() : s,
        string s when Kind == SettingKind.Text => string.IsNullOrEmpty(s) ? CoreStrings.Value_NoText : s,
        string[] l => l.Length == 0 ? CoreStrings.Value_EmptyList : string.Format(CoreStrings.Value_ItemCount, l.Length),
        _ => value.ToString() ?? string.Empty,
    };
}

/// <summary>A pair of checkerboard colors.</summary>
public readonly record struct CheckerColors(uint Light, uint Dark)
{
    public static bool TryParse(string? text, out CheckerColors colors)
    {
        colors = default;
        var parts = text?.Split(';');
        if (parts is not { Length: 2 }) return false;
        if (!uint.TryParse(parts[0], System.Globalization.NumberStyles.HexNumber, null, out var a)) return false;
        if (!uint.TryParse(parts[1], System.Globalization.NumberStyles.HexNumber, null, out var b)) return false;
        colors = new CheckerColors(a, b);
        return true;
    }

    public CheckerColors Swapped() => new(Dark, Light);

    public override string ToString() => $"{Light:X8};{Dark:X8}";
}

/// <summary>Values shared by the engine and the UI. Integer values are part of the registry contract; do not renumber.</summary>
public static class Values
{
    public static class Generation { public const int Disabled = 0, Enabled = 1, Fallback = 2; }
    public static class Embedded { public const int UseImage = 0, UseEmbedded = 1, UseEmbeddedIfLarger = 2; }
    public static class Sampling { public const int Nearest = 0, Linear = 1, Cubic = 2; }
    public static class Transparency { public const int Opaque = 0, Canvas = 1, Transparent = 2, Checkerboard = 3; }
    public static class Scale { public const int ActualSize = 0, ScaleToFit = 1; }
    public static class FullFill { public const int None = 0, Transparent = 1, Checkerboard = 2, Canvas = 3; }
    public static class Alpha { public const int Straight = 0, Premultiplied = 1; }
    public static class Adornment { public const int None = 0, DropShadow = 1; }
    public static class ContextMenu { public const int Disabled = 0, Expanded = 1, Submenu = 2; }
}
