using Glancy.Core.Resources;
namespace Glancy.Core.Settings;

public enum ScopeKind { AllFormats, Format, Extension }

/// <summary>What a per-format edit applies to: every format, one format (and all its extensions), or one extension.</summary>
public readonly record struct SettingsScope(ScopeKind Kind, FormatDefinition? Format, string? Extension)
{
    public static SettingsScope AllFormats { get; } = new(ScopeKind.AllFormats, null, null);

    public static SettingsScope ForFormat(FormatDefinition format) => new(ScopeKind.Format, format, null);

    public static SettingsScope ForExtension(FormatDefinition format, string extension) =>
        new(ScopeKind.Extension, format, FormatDefinition.Normalize(extension));

    /// <summary>The formats an edit in this scope touches.</summary>
    public IEnumerable<FormatDefinition> Formats =>
        Kind == ScopeKind.AllFormats ? Catalog.Formats : [Format!];

    public string Label => Kind switch
    {
        ScopeKind.AllFormats => CoreStrings.Scope_AllFormats,
        ScopeKind.Format => Format!.DisplayName,
        _ => $"{Format!.DisplayName} › {Extension}",
    };
}

/// <summary>The value of a setting as seen from a scope.</summary>
public readonly record struct SettingReading(object? Value, bool IsMixed, bool IsOverridden)
{
    public int AsInt(int fallback = 0) => Value is int i ? i : fallback;
    public bool AsBool() => Value is int i && i != 0;
    public string AsString() => Value as string ?? string.Empty;
}

/// <summary>One line of the pending-changes list.</summary>
public sealed record SettingChange(string ScopeLabel, SettingDefinition Setting, object? From, object? To)
{
    public string Summary => string.Format(CoreStrings.Change_Summary, Setting.Title, Setting.FormatValue(From), Setting.FormatValue(To));
}
