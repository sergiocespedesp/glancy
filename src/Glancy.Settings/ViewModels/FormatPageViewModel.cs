using Glancy.Core.Settings;
using Glancy.Settings.Controls;
using Glancy.Settings.Services;
using Glancy.Settings.Resources;

namespace Glancy.Settings.ViewModels;

/// <summary>State of a format page ("Adobe Photoshop") or of the all-formats page, in its current scope.</summary>
public sealed class FormatPageViewModel : ObservableObject, IDisposable
{
    private readonly SettingsSession _session = AppServices.Session;

    public FormatPageViewModel(FormatDefinition? format, string? extension = null)
    {
        Format = format;
        Scope = format is null ? SettingsScope.AllFormats
              : extension is null ? SettingsScope.ForFormat(format)
              : SettingsScope.ForExtension(format, extension);

        Mode = Item(Catalog.Mode);
        SmallIcons = Item(Catalog.SmallIcons);
        Transparency = Item(Catalog.Transparency);
        CanvasColor = Item(Catalog.CanvasColor);
        Checker = Item(Catalog.Checker);
        Scale = Item(Catalog.ScaleMode);
        FullFill = Item(Catalog.FullFill);
        Adornment = Item(Catalog.Adornment);
        Embedded = Item(Catalog.Embedded);
        MaxFileSize = Item(Catalog.MaxFileSize);
        MaxPixels = Item(Catalog.MaxPixels);
        ShowAssociationIcon = Item(Catalog.ShowAssociationIcon);
        CustomIcon = Item(Catalog.CustomIcon);
        CustomIconPath = Item(Catalog.CustomIconPath);
        Sampling = Item(Catalog.Sampling);
        Gamma = Item(Catalog.Gamma);
        Alpha = Item(Catalog.Alpha);

        _session.Changed += OnSessionChanged;
    }

    public FormatDefinition? Format { get; }
    public SettingsScope Scope { get; private set; }
    public List<SettingItem> Items { get; } = [];

    public SettingItem Mode { get; }
    public SettingItem SmallIcons { get; }
    public SettingItem Transparency { get; }
    public SettingItem CanvasColor { get; }
    public SettingItem Checker { get; }
    public SettingItem Scale { get; }
    public SettingItem FullFill { get; }
    public SettingItem Adornment { get; }
    public SettingItem Embedded { get; }
    public SettingItem MaxFileSize { get; }
    public SettingItem MaxPixels { get; }
    public SettingItem ShowAssociationIcon { get; }
    public SettingItem CustomIcon { get; }
    public SettingItem CustomIconPath { get; }
    public SettingItem Sampling { get; }
    public SettingItem Gamma { get; }
    public SettingItem Alpha { get; }

    public bool IsAllFormats => Format is null;
    public string FormatId => Format?.Id ?? "all";
    public string Title => Format?.DisplayName ?? AppStrings.Nav_AllFormats;

    public string Description => Format?.Description
        ?? AppStrings.AllFormats_Description;

    public string ExtensionsText => Format is { } f
        ? string.Join("  ", f.Extensions)
        : string.Format(AppStrings.AllFormats_Counts, Catalog.Formats.Count, Catalog.AllExtensions.Count);

    public bool HasMultipleExtensions => Format is { Extensions.Count: > 1 };

    public bool IsExtensionScope => Scope.Kind == ScopeKind.Extension;

    public string ScopeLabel => Scope.Label;

    public string ExtensionScopeMessage => IsExtensionScope
        ? string.Format(AppStrings.Format_ExtensionScope_Help, Scope.Extension, Format!.DisplayName)
        : string.Empty;

    public bool HasExtensionOverrides => IsExtensionScope && _session.HasOverrides(Scope.Extension!);

    // ---- conditional sections

    public bool ShowCanvasColor =>
        Transparency.IsMixed || FullFill.IsMixed
        || Transparency.SelectedOption?.Value == Values.Transparency.Canvas
        || FullFill.SelectedOption?.Value == Values.FullFill.Canvas;

    public bool ShowChecker =>
        Transparency.IsMixed || FullFill.IsMixed
        || Transparency.SelectedOption?.Value == Values.Transparency.Checkerboard
        || FullFill.SelectedOption?.Value == Values.FullFill.Checkerboard;

    public bool ShowCustomIconPath => CustomIcon.IsOn || CustomIcon.IsMixed;

    public bool HasAdvanced => Sampling.IsSupported || Gamma.IsSupported || Alpha.IsSupported;

    public bool HasPerformanceExtras => Embedded.IsSupported || MaxPixels.IsSupported;

    /// <summary>With "system provider" or "disabled", Glancy's appearance settings are not used.</summary>
    public bool AppearanceInactive => !Mode.IsMixed && Mode.SelectedOption?.Value != Values.Generation.Enabled;

    public string AppearanceInactiveMessage => Mode.SelectedOption?.Value switch
    {
        Values.Generation.Fallback => AppStrings.Format_Inactive_Fallback,
        Values.Generation.Disabled => AppStrings.Format_Inactive_Disabled,
        _ => string.Empty,
    };

    public string SampleFileName => Scope.Kind switch
    {
        ScopeKind.Extension => SampleBaseName + Scope.Extension,
        ScopeKind.Format => SampleBaseName + Format!.Extensions[0],
        _ => AppStrings.Sample_Design + ".psd",
    };

    /// <summary>Base name of the preview's sample file, in the vocabulary of the format's category.</summary>
    private string SampleBaseName => Format?.Category switch
    {
        "design" or "vector" => AppStrings.Sample_Design,
        "photo" or "hdr" => AppStrings.Sample_Photo,
        "image" or "texture" or "retro" => AppStrings.Sample_Image,
        "3d" => AppStrings.Sample_Model,
        "document" or "text" => AppStrings.Sample_Document,
        _ => AppStrings.Sample_File,
    };

    public PreviewState Preview
    {
        get
        {
            var previewFormat = Format ?? Catalog.Photoshop;
            var cell = _session.ReadGlobal(Catalog.CheckerScale).AsInt(1) switch { 0 => 4, 2 => 16, _ => 8 };
            int Choice(SettingItem item, SettingDefinition d) =>
                item.SelectedOption?.Value ?? (int)previewFormat.DefaultFor(d);
            return new PreviewState(
                Transparency: Choice(Transparency, Catalog.Transparency),
                CanvasColor: CanvasColor.IsMixed ? Catalog.White : CanvasColor.ColorValue,
                Checker: Checker.CheckerValue,
                CheckerCell: cell,
                Scale: Choice(Scale, Catalog.ScaleMode),
                FullFill: Choice(FullFill, Catalog.FullFill),
                Adornment: Choice(Adornment, Catalog.Adornment),
                ShowBadge: ShowAssociationIcon.IsOn || ShowAssociationIcon.IsMixed,
                CustomIcon: CustomIcon.IsOn && !string.IsNullOrEmpty(CustomIconPath.TextValue),
                Monogram: Format?.Monogram ?? "Ps",
                BadgeColor: previewFormat.BadgeColor);
        }
    }

    public void SetScope(SettingsScope scope)
    {
        Scope = scope;
        foreach (var item in Items) item.SetScope(scope);
        RaiseComputed();
    }

    public SettingsClipboard Copy() => _session.Copy(Scope);

    public int Paste(SettingsClipboard clipboard) => _session.Paste(Scope, clipboard);

    public void ResetScope() => _session.ResetScope(Scope);

    public void Dispose() => _session.Changed -= OnSessionChanged;

    private SettingItem Item(SettingDefinition d)
    {
        var item = new SettingItem(_session, d, Scope);
        Items.Add(item);
        return item;
    }

    private void OnSessionChanged(object? sender, EventArgs e)
    {
        foreach (var item in Items) item.Refresh();
        RaiseComputed();
    }

    private void RaiseComputed() => OnPropertiesChanged(
        nameof(Scope), nameof(ScopeLabel), nameof(IsExtensionScope), nameof(ExtensionScopeMessage), nameof(HasExtensionOverrides),
        nameof(ShowCanvasColor), nameof(ShowChecker), nameof(ShowCustomIconPath), nameof(AppearanceInactive),
        nameof(AppearanceInactiveMessage), nameof(SampleFileName), nameof(Preview));
}
