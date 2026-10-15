using Glancy.Core.Localization;
using Glancy.Core.Storage;
using Glancy.Settings.Resources;
using Glancy.Settings.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Glancy.Settings.Views;

public sealed partial class PreferencesPage : Page
{
    private bool _loading = true;

    public PreferencesPage()
    {
        InitializeComponent();
        var prefs = AppServices.Preferences;
        ThemeBox.SelectedIndex = (int)prefs.Theme;
        TopMostSwitch.IsOn = prefs.AlwaysOnTop;
        TipsSwitch.IsOn = prefs.ShowTips;
        LoadLanguages(prefs.Language);
        _loading = false;
    }

    /// <summary>"Windows language (English)" first, then every language by its own name.</summary>
    private void LoadLanguages(string preference)
    {
        var windows = AppLanguage.Resolve(null, Windows.System.UserProfile.GlobalizationPreferences.Languages);
        LanguageBox.Items.Add(new ComboBoxItem
        {
            Content = string.Format(AppStrings.Preferences_Language_Auto, NativeName(windows)),
            Tag = string.Empty,
        });
        foreach (var language in AppLanguage.Supported)
            LanguageBox.Items.Add(new ComboBoxItem { Content = language.NativeName, Tag = language.Code });

        var selected = AppLanguage.Match(preference) ?? string.Empty;
        LanguageBox.SelectedItem = LanguageBox.Items.Cast<ComboBoxItem>().First(i => (string)i.Tag == selected);
        RestartBar.IsOpen = false;
    }

    private static string NativeName(string code) => AppLanguage.Supported.First(l => l.Code == code).NativeName;

    private void OnLanguageChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_loading || LanguageBox.SelectedItem is not ComboBoxItem { Tag: string code }) return;
        AppServices.Preferences.Language = code;
        // The language is applied at start-up: offer a restart only when it would change what is shown.
        RestartBar.IsOpen = AppLanguage.Resolve(code, Windows.System.UserProfile.GlobalizationPreferences.Languages) != Program.Language;
    }

    private void OnRestartNow(object sender, RoutedEventArgs e) => App.Current.Window?.Restart();

    private void OnThemeChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_loading || ThemeBox.SelectedIndex < 0) return;
        App.Current.Window?.SetTheme((AppTheme)ThemeBox.SelectedIndex);
    }

    private void OnTopMostToggled(object sender, RoutedEventArgs e)
    {
        if (!_loading) App.Current.Window?.SetAlwaysOnTop(TopMostSwitch.IsOn);
    }

    private void OnTipsToggled(object sender, RoutedEventArgs e)
    {
        if (_loading) return;
        AppServices.Preferences.ShowTips = TipsSwitch.IsOn;
        if (TipsSwitch.IsOn) AppServices.Preferences.FirstRunCompleted = false;
    }
}
