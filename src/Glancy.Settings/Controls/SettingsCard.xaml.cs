using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Markup;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Controls;

/// <summary>
/// A Windows 11 settings row: icon, title, description and an action control. It shows a pending-change dot,
/// a reset button when the value differs from its default, and stacks the action below the text when narrow.
/// </summary>
[ContentProperty(Name = nameof(Action))]
public sealed partial class SettingsCard : UserControl
{
    private const double StackBelowWidth = 560;

    public SettingsCard()
    {
        InitializeComponent();
        SizeChanged += (_, e) => UpdateLayoutMode(e.NewSize.Width);
        // ResetRequested handlers are attached after properties are set, so re-evaluate once loaded.
        Loaded += (_, _) => UpdateState();
    }

    public static readonly DependencyProperty HeaderProperty =
        DependencyProperty.Register(nameof(Header), typeof(string), typeof(SettingsCard), new PropertyMetadata(string.Empty, OnTextChanged));

    public static readonly DependencyProperty DescriptionProperty =
        DependencyProperty.Register(nameof(Description), typeof(string), typeof(SettingsCard), new PropertyMetadata(string.Empty, OnTextChanged));

    public static readonly DependencyProperty GlyphProperty =
        DependencyProperty.Register(nameof(Glyph), typeof(string), typeof(SettingsCard), new PropertyMetadata(""));

    public static readonly DependencyProperty ActionProperty =
        DependencyProperty.Register(nameof(Action), typeof(object), typeof(SettingsCard), new PropertyMetadata(null, OnTextChanged));

    public static readonly DependencyProperty StatusTextProperty =
        DependencyProperty.Register(nameof(StatusText), typeof(string), typeof(SettingsCard), new PropertyMetadata(null, OnStateChanged));

    public static readonly DependencyProperty IsModifiedProperty =
        DependencyProperty.Register(nameof(IsModified), typeof(bool), typeof(SettingsCard), new PropertyMetadata(false, OnStateChanged));

    public static readonly DependencyProperty IsPendingProperty =
        DependencyProperty.Register(nameof(IsPending), typeof(bool), typeof(SettingsCard), new PropertyMetadata(false, OnStateChanged));

    public static readonly DependencyProperty ResetToolTipProperty =
        DependencyProperty.Register(nameof(ResetToolTip), typeof(string), typeof(SettingsCard), new PropertyMetadata(AppStrings.Format_Reset, OnStateChanged));

    public string Header { get => (string)GetValue(HeaderProperty); set => SetValue(HeaderProperty, value); }
    public string Description { get => (string)GetValue(DescriptionProperty); set => SetValue(DescriptionProperty, value); }
    public string Glyph { get => (string)GetValue(GlyphProperty); set => SetValue(GlyphProperty, value); }
    public object? Action { get => GetValue(ActionProperty); set => SetValue(ActionProperty, value); }
    public string? StatusText { get => (string?)GetValue(StatusTextProperty); set => SetValue(StatusTextProperty, value); }
    public bool IsModified { get => (bool)GetValue(IsModifiedProperty); set => SetValue(IsModifiedProperty, value); }
    public bool IsPending { get => (bool)GetValue(IsPendingProperty); set => SetValue(IsPendingProperty, value); }
    public string ResetToolTip { get => (string)GetValue(ResetToolTipProperty); set => SetValue(ResetToolTipProperty, value); }

    /// <summary>Raised when the user asks to restore the default value.</summary>
    public event EventHandler? ResetRequested;

    public static readonly DependencyProperty ItemProperty =
        DependencyProperty.Register(nameof(Item), typeof(ViewModels.SettingItem), typeof(SettingsCard), new PropertyMetadata(null, OnItemChanged));

    /// <summary>
    /// Optional: binds header, description, glyph, state, visibility and reset to a setting, so pages only
    /// declare the action control.
    /// </summary>
    public ViewModels.SettingItem? Item { get => (ViewModels.SettingItem?)GetValue(ItemProperty); set => SetValue(ItemProperty, value); }

    private static void OnItemChanged(DependencyObject d, DependencyPropertyChangedEventArgs e)
    {
        var card = (SettingsCard)d;
        if (e.OldValue is ViewModels.SettingItem old)
        {
            old.PropertyChanged -= card.OnItemPropertyChanged;
            card.ResetRequested -= card.OnItemReset;
        }
        if (e.NewValue is ViewModels.SettingItem item)
        {
            item.PropertyChanged += card.OnItemPropertyChanged;
            card.ResetRequested += card.OnItemReset;
            card.Header = item.Title;
            card.Glyph = item.Glyph;
            card.SyncFromItem();
        }
    }

    private void OnItemReset(object? sender, EventArgs e) => Item?.Reset();

    private void OnItemPropertyChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e) => SyncFromItem();

    private void SyncFromItem()
    {
        if (Item is not { } item) return;
        Description = item.Description;
        HintText.Text = item.OptionHint ?? string.Empty;
        HintText.Visibility = string.IsNullOrEmpty(item.OptionHint) ? Visibility.Collapsed : Visibility.Visible;
        StatusText = item.StatusText;
        IsModified = item.IsModified;
        IsPending = item.IsPending;
        ResetToolTip = item.ResetToolTip;
        Visibility = item.IsSupported ? Visibility.Visible : Visibility.Collapsed;
    }

    private static void OnTextChanged(DependencyObject d, DependencyPropertyChangedEventArgs e) => ((SettingsCard)d).UpdateAutomation();

    private static void OnStateChanged(DependencyObject d, DependencyPropertyChangedEventArgs e) => ((SettingsCard)d).UpdateState();

    private void UpdateState()
    {
        PendingDot.Visibility = IsPending ? Visibility.Visible : Visibility.Collapsed;
        ResetButton.Visibility = IsModified && ResetRequested is not null ? Visibility.Visible : Visibility.Collapsed;
        ToolTipService.SetToolTip(ResetButton, ResetToolTip);
        AutomationProperties.SetName(ResetButton, $"{ResetToolTip} ({Header})");
        StatusTextBlock.Visibility = string.IsNullOrEmpty(StatusText) ? Visibility.Collapsed : Visibility.Visible;
        UpdateAutomation();
    }

    /// <summary>Gives the action control the card's title and description so screen readers announce them.</summary>
    private void UpdateAutomation()
    {
        if (Action is not DependencyObject target) return;
        AutomationProperties.SetName(target, IsPending ? string.Format(AppStrings.SettingsCard_PendingName, Header) : Header);
        var help = string.Join(" ", new[] { Description, HintText.Text, StatusText }.Where(t => !string.IsNullOrWhiteSpace(t)));
        AutomationProperties.SetHelpText(target, help);
    }

    private void UpdateLayoutMode(double width)
    {
        var stacked = width < StackBelowWidth;
        Grid.SetRow(ActionPresenter, stacked ? 1 : 0);
        Grid.SetColumn(ActionPresenter, stacked ? 1 : 3);
        Grid.SetColumnSpan(ActionPresenter, stacked ? 3 : 1);
        ActionPresenter.HorizontalAlignment = stacked ? HorizontalAlignment.Left : HorizontalAlignment.Right;
    }

    private void OnResetClick(object sender, RoutedEventArgs e) => ResetRequested?.Invoke(this, EventArgs.Empty);
}
