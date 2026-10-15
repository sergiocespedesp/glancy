namespace Glancy.Core;

/// <summary>Product identity shared by the settings app and the thumbnail engine contract.</summary>
public static class AppInfo
{
    public const string ProductName = "Glancy";
    public static string ProductTagline => Resources.CoreStrings.Product_Tagline;

    /// <summary>HKCU sub-key that holds the settings consumed by the thumbnail engine.</summary>
    public const string SettingsKeyPath = @"Software\Glancy\Thumbnails";

    /// <summary>HKCU sub-key for preferences that only affect the settings app itself.</summary>
    public const string AppPreferencesKeyPath = @"Software\Glancy\App";

    /// <summary>CLSID the Glancy IThumbnailProvider DLL registers under (see docs/registry-contract.md).</summary>
    public static readonly Guid ThumbnailProviderClsid = new("6C3A8B2E-5F1D-4E7A-9B0C-2D8E4F61A7C5");

    /// <summary>Shell extension key Explorer queries for a type's thumbnail handler (IThumbnailProvider).</summary>
    public const string ThumbnailHandlerShellExKey = "{e357fccd-a995-4576-b01f-234630154e96}";

    /// <summary>Public source repository (GPL-3.0-or-later; see LICENSE).</summary>
    public const string RepositoryUrl = "https://github.com/sergiocespedesp/glancy";

    public const string LicenseUrl = "https://www.gnu.org/licenses/gpl-3.0.html";

    /// <summary>Where to donate, by platform name (brand names, not translated). Keep .github/FUNDING.yml in sync.</summary>
    public static readonly (string Name, string Url)[] DonationLinks =
    [
        ("GitHub Sponsors", "https://github.com/sponsors/sergiocespedesp"),
        ("Ko-fi", "https://ko-fi.com/sergiocespedesp"),
        ("Buy Me a Coffee", "https://buymeacoffee.com/sergiocespedesp"),
    ];

    public static string Version =>
        typeof(AppInfo).Assembly.GetName().Version?.ToString(3) ?? "0.0.0";
}
