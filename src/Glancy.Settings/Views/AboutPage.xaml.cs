using System.Runtime.InteropServices;
using Glancy.Core;
using Glancy.Core.Storage;
using Glancy.Settings.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Views;

public sealed partial class AboutPage : Page
{
    /// <summary>Single source for the shortcut list shown here; MainWindow registers the same keys.</summary>
    public static (string Keys, string Action)[] Shortcuts =>
    [
        ("Ctrl + S", AppStrings.Shortcut_Apply),
        ("Ctrl + Z", AppStrings.Pending_Undo_Name),
        ("Ctrl + F", AppStrings.Search_Placeholder),
        ("Ctrl + 0", AppStrings.Nav_AllFormats),
        ("Ctrl + 1 … 5", "Photoshop, Illustrator, EPS, PDF, SVG"),
        ("Ctrl + E", AppStrings.Nav_Explorer),
        ("Ctrl + ,", AppStrings.Nav_Preferences),
        (AppStrings.Shortcut_CopyPaste_Keys, AppStrings.Shortcut_CopyPaste),
        ("Ctrl + T", AppStrings.TopMost_Name),
        ("Alt + ←", AppStrings.Shortcut_Back),
        ("F1", AppStrings.Shortcut_Help),
    ];

    /// <summary>Third-party components shipped with (or required by) the app, with their licenses.</summary>
    public static (string Name, string Detail)[] Credits =>
    [
        (AppStrings.Credit_WinAppSdk_Name, AppStrings.Credit_WinAppSdk),
        (".NET", AppStrings.Credit_DotNet),
        (AppStrings.Credit_Segoe_Name, AppStrings.Credit_Segoe),
        ("Sora", AppStrings.Credit_Sora),
        // Thumbnail engine (src/Glancy.Thumbnails). Licenses checked against `cargo metadata` and the
        // PDFium license files that tools/build-engine.ps1 copies to Engine\licenses.
        ("PDFium", AppStrings.Credit_Pdfium),
        (AppStrings.Credit_PdfiumLibs_Name, AppStrings.Credit_PdfiumLibs),
        ("resvg", AppStrings.Credit_Resvg),
        ("image", AppStrings.Credit_Image),
        ("jxl-oxide", AppStrings.Credit_JxlOxide),
        ("hayro-jpeg2000", AppStrings.Credit_Hayro),
        ("texture2ddecoder", AppStrings.Credit_Texture2d),
        ("quick-xml", AppStrings.Credit_QuickXml),
        ("zip", AppStrings.Credit_Zip),
        ("flate2", AppStrings.Credit_Flate2),
        ("ruzstd", AppStrings.Credit_Ruzstd),
        ("cfb", AppStrings.Credit_Cfb),
        ("fax", AppStrings.Credit_Fax),
        (AppStrings.Credit_Serde_Name, AppStrings.Credit_Serde),
        ("windows-rs", AppStrings.Credit_WindowsRs),
        (AppStrings.Credit_OtherRust_Name, AppStrings.Credit_OtherRust),
        (AppStrings.Credit_Ghostscript_Name, AppStrings.Credit_Ghostscript),
    ];

    public AboutPage()
    {
        InitializeComponent();
        VersionText.Text = string.Format(AppStrings.About_Version, AppInfo.Version, AppInfo.ProductTagline);
        TechText.Text = string.Join(Environment.NewLine,
            $"Windows {Environment.OSVersion.Version}  ·  {RuntimeInformation.OSArchitecture}",
            $".NET {Environment.Version}  ·  Windows App SDK 2.5 (WinUI 3)",
            string.Format(AppStrings.About_Tech_Settings, $"HKEY_CURRENT_USER\\{AppInfo.SettingsKeyPath}"),
            string.Format(AppStrings.About_Tech_Logs, Log.Directory),
            string.Format(AppStrings.About_Tech_Clsid, $"{AppInfo.ThumbnailProviderClsid:B}"),
            string.Format(AppStrings.About_Tech_ExportFile, SettingsFile.Extension));

        SourceLink.NavigateUri = new Uri(AppInfo.RepositoryUrl);
        LicenseLink.NavigateUri = new Uri(AppInfo.LicenseUrl);
        foreach (var (name, url) in AppInfo.DonationLinks)
            DonationLinks.Children.Add(new HyperlinkButton { Content = name, NavigateUri = new Uri(url) });

        foreach (var (keys, action) in Shortcuts)
        {
            var row = new Grid { ColumnSpacing = 16, Padding = new Thickness(0, 6, 0, 6) };
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(180) });
            row.ColumnDefinitions.Add(new ColumnDefinition());
            var key = new Border
            {
                Style = (Style)Application.Current.Resources["GlancyKeyCapStyle"],
                Child = new TextBlock { Text = keys, Style = (Style)Application.Current.Resources["CaptionTextBlockStyle"] },
            };
            row.Children.Add(key);
            var text = new TextBlock { Text = action, VerticalAlignment = VerticalAlignment.Center, TextWrapping = TextWrapping.Wrap };
            Grid.SetColumn(text, 1);
            row.Children.Add(text);
            ShortcutsList.Items.Add(row);
        }

        foreach (var (name, detail) in Credits)
        {
            var item = new StackPanel { Padding = new Thickness(0, 6, 0, 6) };
            item.Children.Add(new TextBlock { Text = name, Style = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"] });
            item.Children.Add(new TextBlock { Text = detail, Style = (Style)Application.Current.Resources["GlancyCaptionStyle"] });
            CreditsList.Items.Add(item);
        }
    }
}
