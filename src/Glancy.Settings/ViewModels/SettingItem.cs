using Glancy.Core.Settings;
using Glancy.Settings.Resources;

namespace Glancy.Settings.ViewModels;

/// <summary>
/// Binds one setting, seen from one scope (or globally), to a settings card. Every read goes to the session,
/// so the card always shows the working value; writes go straight into the session's pending changes.
/// </summary>
public sealed class SettingItem : ObservableObject
{
    private static readonly string[] AllProperties =
    [
        nameof(SelectedOption), nameof(IsOn), nameof(ColorValue), nameof(TextValue), nameof(ListValue),
        nameof(CheckerValue), nameof(IsMixed), nameof(IsModified), nameof(IsPending), nameof(IsOverridden),
        nameof(IsSupported), nameof(StatusText), nameof(ResetToolTip), nameof(OptionHint),
    ];

    private readonly SettingsSession _session;

    /// <summary>
    /// True while change notifications are being raised. Two-way x:Bind writes target values straight back to
    /// the source; without this guard a refresh (e.g. after Discard) could turn a displayed value into a new edit.
    /// </summary>
    private bool _refreshing;

    public SettingItem(SettingsSession session, SettingDefinition definition, SettingsScope? scope)
    {
        _session = session;
        Definition = definition;
        Scope = scope;
    }

    public SettingDefinition Definition { get; }

    /// <summary>Null for global settings.</summary>
    public SettingsScope? Scope { get; private set; }

    public string Key => Definition.Key;
    public string Title => Definition.Title;
    public string Glyph => Definition.Glyph;
    public IReadOnlyList<SettingOption> Options => Definition.Options;

    public string Description => Definition.Description;

    /// <summary>Explanation of the currently selected option, shown as its own line under the description.</summary>
    public string? OptionHint => Definition.Kind == SettingKind.Choice && !IsMixed ? SelectedOption?.Description : null;

    private SettingReading Reading => Scope is { } s ? _session.Read(s, Definition) : _session.ReadGlobal(Definition);

    public bool IsSupported => Scope is not { } s || SettingsSession.IsSupported(s, Definition);
    public bool IsMixed => Reading.IsMixed;
    public bool IsOverridden => Reading.IsOverridden;

    public bool IsModified => Scope is { } s ? _session.IsModifiedFromDefault(s, Definition) : _session.IsGlobalModifiedFromDefault(Definition);

    public bool IsPending => Scope is { } s ? _session.IsPending(s, Definition) : _session.IsGlobalPending(Definition);

    public string? StatusText => (Scope?.Kind, IsMixed, IsOverridden) switch
    {
        (_, true, _) => AppStrings.Setting_Status_Mixed,
        (ScopeKind.Extension, false, true) => string.Format(AppStrings.Setting_Status_CustomizedFor, Scope!.Value.Extension),
        (ScopeKind.Format or ScopeKind.AllFormats, false, true) => AppStrings.Setting_Status_SomeExtension,
        _ => null,
    };

    public string ResetToolTip => Scope switch
    {
        { Kind: ScopeKind.Extension } => AppStrings.Setting_Reset_UseFormat,
        { Kind: ScopeKind.AllFormats } => AppStrings.Setting_Reset_EachFormat,
        { Kind: ScopeKind.Format } s => string.Format(AppStrings.Setting_Reset_To, Definition.FormatValue(s.Format!.DefaultFor(Definition))),
        _ => string.Format(AppStrings.Setting_Reset_To, Definition.FormatValue(Definition.DefaultValue)),
    };

    public SettingOption? SelectedOption
    {
        get => Definition.FindOption(Reading.Value);
        set
        {
            if (_refreshing || value is null || (Reading.Value is int current && current == value.Value)) return;
            Write(value.Value);
        }
    }

    public bool IsOn
    {
        get => Reading.AsBool();
        set
        {
            // No mixed-value bypass: a mixed switch displays Off, and that display must never be written back.
            if (_refreshing || value == IsOn) return;
            Write(value ? 1 : 0);
        }
    }

    public uint ColorValue
    {
        get => Reading.Value is int c ? unchecked((uint)c) : Catalog.White;
        set
        {
            if (_refreshing || (!IsMixed && value == ColorValue)) return;
            Write(unchecked((int)value));
        }
    }

    public string TextValue
    {
        get => Reading.AsString();
        set
        {
            if (_refreshing || (!IsMixed && value == TextValue)) return;
            Write(value ?? string.Empty);
        }
    }

    public string[] ListValue
    {
        get => Reading.Value as string[] ?? [];
        set => Write(value);
    }

    public CheckerColors CheckerValue
    {
        get => CheckerColors.TryParse(Reading.Value as string, out var c) ? c : new CheckerColors(0xFFFFFFFF, 0xFFCCCCCC);
        set
        {
            if (_refreshing || (!IsMixed && value == CheckerValue)) return;
            Write(value.ToString());
        }
    }

    public void SetScope(SettingsScope scope)
    {
        Scope = scope;
        Refresh();
    }

    public void Reset()
    {
        if (Scope is { } s) _session.Reset(s, Definition);
        else _session.ResetGlobal(Definition);
    }

    public void Refresh()
    {
        _refreshing = true;
        try
        {
            foreach (var p in AllProperties) OnPropertyChanged(p);
        }
        finally
        {
            _refreshing = false;
        }
    }

    private void Write(object value)
    {
        if (Scope is { } s) _session.Write(s, Definition, value);
        else _session.WriteGlobal(Definition, value);
    }
}
