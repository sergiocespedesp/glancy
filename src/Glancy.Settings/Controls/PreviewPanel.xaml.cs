using Glancy.Settings.Helpers;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Controls;

/// <summary>Side panel with the simulated thumbnail at three Explorer sizes, over a light or dark Explorer surface.</summary>
public sealed partial class PreviewPanel : UserControl
{
    private PreviewState? _state;
    private bool? _darkSurface;

    public PreviewPanel()
    {
        InitializeComponent();
        ActualThemeChanged += (_, _) => { if (_darkSurface is null) ApplySurface(); };
        Loaded += (_, _) => ApplySurface();
    }

    /// <summary>Inline (narrow window) mode: smaller largest tile so settings stay close to the top.</summary>
    public bool IsCompact
    {
        get => LargeViewbox.MaxWidth < 256;
        set => LargeViewbox.MaxWidth = value ? 160 : 256;
    }

    public void Update(PreviewState state, string scopeLabel, string sampleFileName)
    {
        _state = state;
        ScopeText.Text = scopeLabel;
        LargeCaption.Text = string.Format(AppStrings.Preview_ExtraLarge, sampleFileName);
        MediumCaption.Text = AppStrings.Preview_Large;
        SmallCaption.Text = AppStrings.Preview_Medium;
        LargeTile.Render(state);
        MediumTile.Render(state);
        SmallTile.Render(state);
    }

    private void OnSurfaceClick(object sender, RoutedEventArgs e)
    {
        _darkSurface = DarkSurfaceButton.IsChecked == true;
        ApplySurface();
    }

    private void ApplySurface()
    {
        var dark = _darkSurface ?? ActualTheme == ElementTheme.Dark;
        DarkSurfaceButton.IsChecked = dark;
        Surface.Background = ColorHelper.Brush(dark ? 0xFF191919 : 0xFFFFFFFF);
        var text = ColorHelper.Brush(dark ? 0xFFFFFFFF : 0xFF1A1A1A);
        LargeCaption.Foreground = MediumCaption.Foreground = SmallCaption.Foreground = text;
        if (_state is not null)
        {
            LargeTile.Render(_state);
            MediumTile.Render(_state);
            SmallTile.Render(_state);
        }
    }
}
