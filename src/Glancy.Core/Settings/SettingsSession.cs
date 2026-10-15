using Glancy.Core.Resources;
namespace Glancy.Core.Settings;

public interface ISettingsStore
{
    SettingsSnapshot Load();
    void Save(SettingsSnapshot snapshot);
}

/// <summary>Settings copied from one scope so they can be pasted into another.</summary>
public sealed record SettingsClipboard(string SourceLabel, IReadOnlyDictionary<string, object> Values);

/// <summary>
/// The editing session behind the settings app. Edits change <see cref="Working"/> only; nothing reaches the
/// engine until <see cref="Apply"/> writes the working copy to the store. This is the single "pending changes"
/// rule the UI explains to the user.
/// </summary>
public sealed class SettingsSession
{
    private const int MaxUndo = 100;
    private readonly ISettingsStore _store;
    private readonly Stack<SettingsSnapshot> _undo = new();

    public SettingsSession(ISettingsStore store)
    {
        _store = store;
        Applied = store.Load();
        Working = Applied.Clone();
    }

    /// <summary>What the engine currently uses.</summary>
    public SettingsSnapshot Applied { get; private set; }

    /// <summary>What the user is editing.</summary>
    public SettingsSnapshot Working { get; private set; }

    /// <summary>Raised after any change to <see cref="Working"/> or <see cref="Applied"/>.</summary>
    public event EventHandler? Changed;

    // ------------------------------------------------------------------ reading

    public SettingReading Read(SettingsScope scope, SettingDefinition setting) => ReadFrom(Working, scope, setting);

    public SettingReading ReadGlobal(SettingDefinition setting) => new(Working.Global[setting.Key], false, false);

    /// <summary>The formats in the scope that support the setting (a setting can be hidden for some formats).</summary>
    public static IEnumerable<FormatDefinition> SupportingFormats(SettingsScope scope, SettingDefinition setting) =>
        scope.Formats.Where(setting.IsSupportedBy);

    public static bool IsSupported(SettingsScope scope, SettingDefinition setting) =>
        SupportingFormats(scope, setting).Any();

    public bool IsModifiedFromDefault(SettingsScope scope, SettingDefinition setting) => scope.Kind switch
    {
        ScopeKind.Extension => HasOverride(Working, scope.Extension!, setting.Key),
        _ => SupportingFormats(scope, setting).Any(f =>
                 !SettingsSnapshot.ValueEquals(Working.Formats[f.Id][setting.Key], f.DefaultFor(setting))
                 || f.Extensions.Any(e => HasOverride(Working, e, setting.Key))),
    };

    public bool IsGlobalModifiedFromDefault(SettingDefinition setting) =>
        !SettingsSnapshot.ValueEquals(Working.Global[setting.Key], setting.DefaultValue);

    /// <summary>True when the scope's value differs from what is applied.</summary>
    public bool IsPending(SettingsScope scope, SettingDefinition setting)
    {
        var a = ReadFrom(Applied, scope, setting);
        var w = ReadFrom(Working, scope, setting);
        return a.IsMixed != w.IsMixed || a.IsOverridden != w.IsOverridden || !SettingsSnapshot.ValueEquals(a.Value, w.Value);
    }

    public bool IsGlobalPending(SettingDefinition setting) =>
        !SettingsSnapshot.ValueEquals(Applied.Global[setting.Key], Working.Global[setting.Key]);

    public bool HasOverrides(string extension) =>
        Working.Overrides.TryGetValue(extension, out var o) && o.Count > 0;

    // ------------------------------------------------------------------ writing

    public void Write(SettingsScope scope, SettingDefinition setting, object value)
    {
        if (!setting.IsValid(value))
            throw new ArgumentException($"Invalid value for '{setting.Key}': {value}", nameof(value));
        Mutate(() => WriteCore(scope, setting, value));
    }

    public void WriteGlobal(SettingDefinition setting, object value)
    {
        if (!setting.IsValid(value))
            throw new ArgumentException($"Invalid value for '{setting.Key}': {value}", nameof(value));
        Mutate(() => Working.Global[setting.Key] = SettingsSnapshot.CloneValue(value));
    }

    public void Reset(SettingsScope scope, SettingDefinition setting) => Mutate(() => ResetCore(scope, setting));

    public void ResetGlobal(SettingDefinition setting) => WriteGlobal(setting, setting.DefaultValue);

    /// <summary>Restores every per-format setting in the scope to its default.</summary>
    public void ResetScope(SettingsScope scope)
    {
        Mutate(() =>
        {
            foreach (var setting in Catalog.FormatSettings) ResetCore(scope, setting);
        });
    }

    /// <summary>Restores everything, global settings included, to factory defaults (still pending until applied).</summary>
    public void ResetAll()
    {
        Mutate(() => Working = SettingsSnapshot.CreateDefaults());
    }

    public SettingsClipboard Copy(SettingsScope scope)
    {
        var values = new Dictionary<string, object>(StringComparer.Ordinal);
        foreach (var setting in Catalog.FormatSettings)
        {
            if (!IsSupported(scope, setting)) continue;
            var r = Read(scope, setting);
            if (!r.IsMixed && r.Value is not null) values[setting.Key] = SettingsSnapshot.CloneValue(r.Value);
        }
        return new SettingsClipboard(scope.Label, values);
    }

    /// <summary>Pastes every copied value the target scope supports. Returns the number of settings pasted.</summary>
    public int Paste(SettingsScope scope, SettingsClipboard clipboard)
    {
        var count = 0;
        Mutate(() =>
        {
            foreach (var (key, value) in clipboard.Values)
            {
                var setting = Catalog.FindSetting(key);
                if (setting is null || setting.Scope != SettingScope.Format || !IsSupported(scope, setting) || !setting.IsValid(value))
                    continue;
                WriteCore(scope, setting, value);
                count++;
            }
        });
        return count;
    }

    /// <summary>Replaces the working copy (e.g. after importing a file). The result is pending until applied.</summary>
    public void LoadWorking(SettingsSnapshot snapshot)
    {
        Mutate(() => Working = snapshot.Clone());
    }

    // ------------------------------------------------------------------ undo

    /// <summary>True when there is an edit since the last apply/discard that <see cref="Undo"/> can revert.</summary>
    public bool CanUndo => _undo.Count > 0;

    /// <summary>Reverts the most recent edit (one write, reset, paste or import).</summary>
    public bool Undo()
    {
        if (_undo.Count == 0) return false;
        Working = _undo.Pop();
        OnChanged();
        return true;
    }

    // ------------------------------------------------------------------ pending changes

    public bool IsDirty => GetChanges().Count > 0;

    public IReadOnlyList<SettingChange> GetChanges()
    {
        var list = new List<SettingChange>();
        foreach (var g in Catalog.GlobalSettings)
        {
            if (IsGlobalPending(g))
                list.Add(new SettingChange(CoreStrings.Scope_General, g, Applied.Global[g.Key], Working.Global[g.Key]));
        }

        foreach (var f in Catalog.Formats)
        {
            foreach (var s in Catalog.FormatSettings.Where(s => s.IsSupportedBy(f)))
            {
                var from = Applied.Formats[f.Id][s.Key];
                var to = Working.Formats[f.Id][s.Key];
                if (!SettingsSnapshot.ValueEquals(from, to)) list.Add(new SettingChange(f.DisplayName, s, from, to));
            }

            foreach (var ext in f.Extensions)
            {
                foreach (var s in Catalog.FormatSettings.Where(s => s.IsSupportedBy(f)))
                {
                    var from = OverrideValue(Applied, ext, s.Key);
                    var to = OverrideValue(Working, ext, s.Key);
                    if (!SettingsSnapshot.ValueEquals(from, to))
                        list.Add(new SettingChange($"{f.DisplayName} › {ext}", s, from, to));
                }
            }
        }
        return list;
    }

    /// <summary>Extensions whose effective thumbnail settings change when applying (used to refresh Explorer).</summary>
    public IReadOnlyList<string> GetAffectedExtensions()
    {
        var globalChanged = Catalog.GlobalSettings.Any(IsGlobalPending);
        var result = new List<string>();
        foreach (var f in Catalog.Formats)
        {
            foreach (var ext in f.Extensions)
            {
                var changed = globalChanged || Catalog.FormatSettings.Any(s =>
                    !SettingsSnapshot.ValueEquals(Applied.EffectiveValue(f, ext, s.Key), Working.EffectiveValue(f, ext, s.Key)));
                if (changed) result.Add(ext);
            }
        }
        return result;
    }

    public void Discard()
    {
        Working = Applied.Clone();
        _undo.Clear();
        OnChanged();
    }

    /// <summary>Writes the working copy to the store. Throws <see cref="SettingsStoreException"/> on failure.</summary>
    public void Apply()
    {
        _store.Save(Working);
        Applied = Working.Clone();
        _undo.Clear();
        OnChanged();
    }

    /// <summary>
    /// Saves one global setting right away (the Jump List's on/off task) without applying the other pending edits,
    /// which stay pending. Undo history adopts the new value so undoing an edit cannot revert it.
    /// </summary>
    public void ApplyGlobalNow(SettingDefinition setting, object value)
    {
        if (!setting.IsValid(value))
            throw new ArgumentException($"Invalid value for '{setting.Key}': {value}", nameof(value));
        var target = Applied.Clone();
        target.Global[setting.Key] = SettingsSnapshot.CloneValue(value);
        _store.Save(target);
        Applied = target;
        Working.Global[setting.Key] = SettingsSnapshot.CloneValue(value);
        foreach (var snapshot in _undo) snapshot.Global[setting.Key] = SettingsSnapshot.CloneValue(value);
        OnChanged();
    }

    /// <summary>Re-reads the store (e.g. another tool changed it). Pending edits are lost.</summary>
    public void Reload()
    {
        Applied = _store.Load();
        Working = Applied.Clone();
        _undo.Clear();
        OnChanged();
    }

    // ------------------------------------------------------------------ internals

    private static SettingReading ReadFrom(SettingsSnapshot snap, SettingsScope scope, SettingDefinition setting)
    {
        switch (scope.Kind)
        {
            case ScopeKind.Extension:
            {
                var overridden = HasOverride(snap, scope.Extension!, setting.Key);
                return new SettingReading(snap.EffectiveValue(scope.Format!, scope.Extension!, setting.Key), false, overridden);
            }
            case ScopeKind.Format:
            {
                var overridden = scope.Format!.Extensions.Any(e => HasOverride(snap, e, setting.Key));
                return new SettingReading(snap.Formats[scope.Format.Id][setting.Key], false, overridden);
            }
            default:
            {
                var formats = SupportingFormats(scope, setting).ToList();
                if (formats.Count == 0) return new SettingReading(null, false, false);
                var first = snap.Formats[formats[0].Id][setting.Key];
                var mixed = formats.Skip(1).Any(f => !SettingsSnapshot.ValueEquals(snap.Formats[f.Id][setting.Key], first));
                var overridden = formats.Any(f => f.Extensions.Any(e => HasOverride(snap, e, setting.Key)));
                return new SettingReading(mixed ? null : first, mixed, overridden);
            }
        }
    }

    private void WriteCore(SettingsScope scope, SettingDefinition setting, object value)
    {
        if (scope.Kind == ScopeKind.Extension)
        {
            // Same rule as format scope: a setting the format does not support is never stored.
            if (!setting.IsSupportedBy(scope.Format!)) return;
            var formatValue = Working.Formats[scope.Format!.Id][setting.Key];
            if (SettingsSnapshot.ValueEquals(formatValue, value)) RemoveOverride(scope.Extension!, setting.Key);
            else GetOrCreateOverrides(scope.Extension!)[setting.Key] = SettingsSnapshot.CloneValue(value);
            return;
        }

        foreach (var f in SupportingFormats(scope, setting))
        {
            Working.Formats[f.Id][setting.Key] = SettingsSnapshot.CloneValue(value);
            // Editing a format (or all formats) applies to all of its extensions, as in the original.
            foreach (var ext in f.Extensions) RemoveOverride(ext, setting.Key);
        }
    }

    private void ResetCore(SettingsScope scope, SettingDefinition setting)
    {
        if (scope.Kind == ScopeKind.Extension)
        {
            RemoveOverride(scope.Extension!, setting.Key);
            return;
        }
        foreach (var f in SupportingFormats(scope, setting))
        {
            Working.Formats[f.Id][setting.Key] = SettingsSnapshot.CloneValue(f.DefaultFor(setting));
            foreach (var ext in f.Extensions) RemoveOverride(ext, setting.Key);
        }
    }

    private static bool HasOverride(SettingsSnapshot snap, string ext, string key) =>
        snap.Overrides.TryGetValue(ext, out var o) && o.ContainsKey(key);

    private static object? OverrideValue(SettingsSnapshot snap, string ext, string key) =>
        snap.Overrides.TryGetValue(ext, out var o) && o.TryGetValue(key, out var v) ? v : null;

    private Dictionary<string, object> GetOrCreateOverrides(string ext)
    {
        if (!Working.Overrides.TryGetValue(ext, out var o))
            Working.Overrides[ext] = o = new Dictionary<string, object>(StringComparer.Ordinal);
        return o;
    }

    private void RemoveOverride(string ext, string key)
    {
        if (!Working.Overrides.TryGetValue(ext, out var o)) return;
        o.Remove(key);
        if (o.Count == 0) Working.Overrides.Remove(ext);
    }

    /// <summary>Runs an edit, recording an undo step only if it actually changed something.</summary>
    private void Mutate(Action edit)
    {
        var before = Working.Clone();
        edit();
        if (before.ContentEquals(Working)) return;
        _undo.Push(before);
        if (_undo.Count > MaxUndo)
        {
            // Drop the oldest step: rebuild without the bottom of the stack.
            var keep = _undo.Take(MaxUndo).Reverse().ToList();
            _undo.Clear();
            foreach (var step in keep) _undo.Push(step);
        }
        OnChanged();
    }

    private void OnChanged() => Changed?.Invoke(this, EventArgs.Empty);
}

public sealed class SettingsStoreException(string message, Exception? inner = null) : Exception(message, inner);
