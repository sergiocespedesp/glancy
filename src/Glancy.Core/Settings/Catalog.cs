namespace Glancy.Core.Settings;

/// <summary>The single source of truth for every setting and format Glancy knows about.</summary>
public static class Catalog
{
    /// <summary>Group ids; their display names are CoreStrings Group_&lt;id&gt;.</summary>
    public static class Groups
    {
        public const string Generation = "generation";
        public const string Appearance = "appearance";
        public const string Performance = "performance";
        public const string Icon = "icon";
        public const string Advanced = "advanced";
        public const string Explorer = "explorer";
        public const string ContextMenu = "contextMenu";
        public const string Checkerboard = "checkerboard";
        public const string Privacy = "privacy";
        public const string Diagnostics = "diagnostics";
    }

    public const uint White = 0xFFFFFFFF;

    public static readonly string[] DefaultCheckerLibrary =
    [
        "FFFFFFFF;FFCCCCCC",
        "FFB4B4B4;FF8C8C8C",
        "FF3C3C3C;FF282828",
    ];


    // ---------------------------------------------------------------- per-format settings

    public static readonly SettingDefinition Mode = new()
    {
        Key = "mode", Scope = SettingScope.Format, Kind = SettingKind.Choice, GroupId = Groups.Generation, Glyph = "",
        DefaultValue = Values.Generation.Enabled, Options =
        [
            new("mode", Values.Generation.Enabled), new("mode", Values.Generation.Fallback),
            new("mode", Values.Generation.Disabled),
        ],
    };

    public static readonly SettingDefinition SmallIcons = new()
    {
        Key = "smallIcons", Scope = SettingScope.Format, Kind = SettingKind.Choice, GroupId = Groups.Generation, Glyph = "",
        DefaultValue = Values.Generation.Disabled,
        Options =
        [
            new("smallIcons", Values.Generation.Enabled),
            new("smallIcons", Values.Generation.Fallback),
            new("smallIcons", Values.Generation.Disabled),
        ],
    };

    public static readonly SettingDefinition Transparency = new()
    {
        Key = "transparency", Scope = SettingScope.Format, Kind = SettingKind.Choice, GroupId = Groups.Appearance, Glyph = "",
        DefaultValue = Values.Transparency.Transparent,
        Options =
        [
            new("transparency", Values.Transparency.Transparent),
            new("transparency", Values.Transparency.Checkerboard),
            new("transparency", Values.Transparency.Canvas),
            new("transparency", Values.Transparency.Opaque),
        ],
    };

    public static readonly SettingDefinition CanvasColor = new()
    {
        Key = "canvasColor", Scope = SettingScope.Format, Kind = SettingKind.Color, GroupId = Groups.Appearance, Glyph = "",
        DefaultValue = unchecked((int)White),
    };

    public static readonly SettingDefinition Checker = new()
    {
        Key = "checkerColors", Scope = SettingScope.Format, Kind = SettingKind.Checker, GroupId = Groups.Appearance, Glyph = "",
        DefaultValue = DefaultCheckerLibrary[0],
    };

    public static readonly SettingDefinition ScaleMode = new()
    {
        Key = "scale", Scope = SettingScope.Format, Kind = SettingKind.Choice, GroupId = Groups.Appearance, Glyph = "",
        DefaultValue = Values.Scale.ActualSize,
        Options =
        [
            new("scale", Values.Scale.ActualSize),
            new("scale", Values.Scale.ScaleToFit),
        ],
    };

    public static readonly SettingDefinition FullFill = new()
    {
        Key = "fullFill", Scope = SettingScope.Format, Kind = SettingKind.Choice, GroupId = Groups.Appearance, Glyph = "",
        DefaultValue = Values.FullFill.None,
        Options =
        [
            new("fullFill", Values.FullFill.None),
            new("fullFill", Values.FullFill.Transparent),
            new("fullFill", Values.FullFill.Checkerboard),
            new("fullFill", Values.FullFill.Canvas),
        ],
    };

    public static readonly SettingDefinition Adornment = new()
    {
        Key = "adornment", Scope = SettingScope.Format, Kind = SettingKind.Choice, GroupId = Groups.Appearance, Glyph = "",
        DefaultValue = Values.Adornment.DropShadow,
        Options =
        [
            new("adornment", Values.Adornment.DropShadow),
            new("adornment", Values.Adornment.None),
        ],
    };

    public static readonly SettingDefinition Embedded = new()
    {
        Key = "embedded", Scope = SettingScope.Format, Kind = SettingKind.Choice, GroupId = Groups.Performance, Glyph = "",
        DefaultValue = Values.Embedded.UseImage, Requires = FormatCapability.EmbeddedThumbnail,
        Options =
        [
            new("embedded", Values.Embedded.UseImage),
            new("embedded", Values.Embedded.UseEmbeddedIfLarger),
            new("embedded", Values.Embedded.UseEmbedded),
        ],
    };

    public static readonly SettingDefinition MaxFileSize = new()
    {
        Key = "maxFileSizeMB", Scope = SettingScope.Format, Kind = SettingKind.Choice, GroupId = Groups.Performance, Glyph = "",
        DefaultValue = 0,
        Options =
        [
            new("maxFileSizeMB", 0), new("maxFileSizeMB", 50), new("maxFileSizeMB", 100), new("maxFileSizeMB", 250),
            new("maxFileSizeMB", 500), new("maxFileSizeMB", 1024), new("maxFileSizeMB", 2048),
        ],
    };

    public static readonly SettingDefinition MaxPixels = new()
    {
        Key = "maxPixels", Scope = SettingScope.Format, Kind = SettingKind.Choice, GroupId = Groups.Performance, Glyph = "",
        DefaultValue = 0, Requires = FormatCapability.PixelLimit,
        Options =
        [
            new("maxPixels", 0), new("maxPixels", 4096), new("maxPixels", 8192), new("maxPixels", 16384),
            new("maxPixels", 32768),
        ],
    };

    public static readonly SettingDefinition ShowAssociationIcon = new()
    {
        Key = "showAssocIcon", Scope = SettingScope.Format, Kind = SettingKind.Toggle, GroupId = Groups.Icon, Glyph = "",
        DefaultValue = 1,
    };

    public static readonly SettingDefinition CustomIcon = new()
    {
        Key = "customIcon", Scope = SettingScope.Format, Kind = SettingKind.Toggle, GroupId = Groups.Icon, Glyph = "",
        DefaultValue = 0,
    };

    public static readonly SettingDefinition CustomIconPath = new()
    {
        Key = "customIconPath", Scope = SettingScope.Format, Kind = SettingKind.Text, GroupId = Groups.Icon, Glyph = "",
        DefaultValue = string.Empty,
    };

    public static readonly SettingDefinition Sampling = new()
    {
        Key = "sampling", Scope = SettingScope.Format, Kind = SettingKind.Choice, GroupId = Groups.Advanced, Glyph = "",
        DefaultValue = Values.Sampling.Cubic, Requires = FormatCapability.Sampling,
        Options =
        [
            new("sampling", Values.Sampling.Cubic),
            new("sampling", Values.Sampling.Linear),
            new("sampling", Values.Sampling.Nearest),
        ],
    };

    public static readonly SettingDefinition Gamma = new()
    {
        Key = "gamma", Scope = SettingScope.Format, Kind = SettingKind.Choice, GroupId = Groups.Advanced, Glyph = "",
        DefaultValue = 100, Requires = FormatCapability.Gamma,
        Options =
        [
            new("gamma", 100), new("gamma", 180), new("gamma", 220), new("gamma", 240),
        ],
    };

    public static readonly SettingDefinition Alpha = new()
    {
        Key = "alpha", Scope = SettingScope.Format, Kind = SettingKind.Choice, GroupId = Groups.Advanced, Glyph = "",
        DefaultValue = Values.Alpha.Straight, Requires = FormatCapability.PremultipliedAlpha,
        Options =
        [
            new("alpha", Values.Alpha.Straight),
            new("alpha", Values.Alpha.Premultiplied),
        ],
    };

    // ---------------------------------------------------------------- global settings

    public static readonly SettingDefinition Enabled = new()
    {
        Key = "enabled", Scope = SettingScope.Global, Kind = SettingKind.Toggle, GroupId = Groups.Explorer, Glyph = "",
        DefaultValue = 1,
    };

    public static readonly SettingDefinition ForceDesktopTransparency = new()
    {
        Key = "forceDesktopTransparency", Scope = SettingScope.Global, Kind = SettingKind.Toggle, GroupId = Groups.Explorer, Glyph = "",
        DefaultValue = 0,
    };

    public static readonly SettingDefinition RefreshOnApply = new()
    {
        Key = "refreshOnApply", Scope = SettingScope.Global, Kind = SettingKind.Toggle, GroupId = Groups.Explorer, Glyph = "",
        DefaultValue = 1,
    };

    public static readonly SettingDefinition ContextMenu = new()
    {
        Key = "contextMenu", Scope = SettingScope.Global, Kind = SettingKind.Choice, GroupId = Groups.ContextMenu, Glyph = "",
        DefaultValue = Values.ContextMenu.Submenu,
        Options =
        [
            new("contextMenu", Values.ContextMenu.Submenu),
            new("contextMenu", Values.ContextMenu.Expanded),
            new("contextMenu", Values.ContextMenu.Disabled),
        ],
    };

    public static readonly SettingDefinition ContextMenuRequireShift = new()
    {
        Key = "contextMenuShift", Scope = SettingScope.Global, Kind = SettingKind.Toggle, GroupId = Groups.ContextMenu, Glyph = "",
        DefaultValue = 0,
    };

    public static readonly SettingDefinition FolderRecursion = new()
    {
        Key = "folderRecursion", Scope = SettingScope.Global, Kind = SettingKind.Toggle, GroupId = Groups.ContextMenu, Glyph = "",
        DefaultValue = 0,
    };

    public static readonly SettingDefinition CheckerAnchor = new()
    {
        Key = "checkerAnchor", Scope = SettingScope.Global, Kind = SettingKind.Choice, GroupId = Groups.Checkerboard, Glyph = "",
        DefaultValue = 0,
        Options =
        [
            new("checkerAnchor", 0), new("checkerAnchor", 1), new("checkerAnchor", 2),
            new("checkerAnchor", 3), new("checkerAnchor", 4), new("checkerAnchor", 5),
            new("checkerAnchor", 6), new("checkerAnchor", 7), new("checkerAnchor", 8),
        ],
    };

    public static readonly SettingDefinition CheckerScale = new()
    {
        Key = "checkerScale", Scope = SettingScope.Global, Kind = SettingKind.Choice, GroupId = Groups.Checkerboard, Glyph = "",
        DefaultValue = 1,
        Options = [new("checkerScale", 0), new("checkerScale", 1), new("checkerScale", 2)],
    };

    public static readonly SettingDefinition CheckerLibrary = new()
    {
        Key = "checkerLibrary", Scope = SettingScope.Global, Kind = SettingKind.List, GroupId = Groups.Checkerboard, Glyph = "",
        DefaultValue = DefaultCheckerLibrary,
    };

    public static readonly SettingDefinition BlockedFolders = new()
    {
        Key = "blockedFolders", Scope = SettingScope.Global, Kind = SettingKind.List, GroupId = Groups.Privacy, Glyph = "",
        DefaultValue = Array.Empty<string>(),
    };

    public static readonly SettingDefinition DebugLog = new()
    {
        Key = "debugLog", Scope = SettingScope.Global, Kind = SettingKind.Toggle, GroupId = Groups.Diagnostics, Glyph = "",
        DefaultValue = 0,
    };

    // ---------------------------------------------------------------- formats

    /// <summary>
    /// Every format, in catalog order, loaded from the embedded <c>formats.json</c> (shared with the engine, which
    /// embeds the same file). Categories group them for navigation; ids, extensions and defaults are the contract.
    /// </summary>
    public static IReadOnlyList<FormatDefinition> Formats => FormatCatalog.Instance.Formats;

    /// <summary>Format categories, in display order (each has at least one format).</summary>
    public static IReadOnlyList<FormatCategory> Categories => FormatCatalog.Instance.Categories;

    // The five original formats, kept as named lookups (shortcuts Ctrl+1…5, demo data, tests).
    public static FormatDefinition Photoshop => FormatCatalog.Instance.Require("psd");
    public static FormatDefinition Illustrator => FormatCatalog.Instance.Require("ai");
    public static FormatDefinition Eps => FormatCatalog.Instance.Require("eps");
    public static FormatDefinition Pdf => FormatCatalog.Instance.Require("pdf");
    public static FormatDefinition Svg => FormatCatalog.Instance.Require("svg");

    /// <summary>The formats reachable with Ctrl+1…5, in shortcut order.</summary>
    public static IReadOnlyList<FormatDefinition> QuickFormats => [Photoshop, Illustrator, Eps, Pdf, Svg];

    public static IReadOnlyList<SettingDefinition> FormatSettings { get; } =
    [
        Mode, SmallIcons,
        Transparency, CanvasColor, Checker, ScaleMode, FullFill, Adornment,
        Embedded, MaxFileSize, MaxPixels,
        ShowAssociationIcon, CustomIcon, CustomIconPath,
        Sampling, Gamma, Alpha,
    ];

    public static IReadOnlyList<SettingDefinition> GlobalSettings { get; } =
    [
        Enabled, ForceDesktopTransparency, RefreshOnApply,
        ContextMenu, ContextMenuRequireShift, FolderRecursion,
        CheckerAnchor, CheckerScale, CheckerLibrary,
        BlockedFolders, DebugLog,
    ];

    public static FormatDefinition? FindFormat(string id) =>
        FormatCatalog.Instance.ById.GetValueOrDefault(id ?? string.Empty);

    public static FormatDefinition? FormatForExtension(string extension) =>
        string.IsNullOrWhiteSpace(extension) ? null : FormatCatalog.Instance.ByExtension.GetValueOrDefault(FormatDefinition.Normalize(extension));

    public static FormatCategory? FindCategory(string id) =>
        Categories.FirstOrDefault(c => string.Equals(c.Id, id, StringComparison.OrdinalIgnoreCase));

    /// <summary>The formats of a category, in catalog order.</summary>
    public static IEnumerable<FormatDefinition> FormatsIn(FormatCategory category) =>
        Formats.Where(f => string.Equals(f.Category, category.Id, StringComparison.Ordinal));

    public static SettingDefinition? FindSetting(string key) =>
        FormatSettings.Concat(GlobalSettings).FirstOrDefault(s => s.Key == key);

    /// <summary>Every extension of every format (lower case, with the dot), in catalog order.</summary>
    public static IReadOnlyList<string> AllExtensions => FormatCatalog.Instance.AllExtensions;
}
