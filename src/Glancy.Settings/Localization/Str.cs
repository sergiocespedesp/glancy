using Glancy.Settings.Resources;
using Microsoft.UI.Xaml.Markup;

namespace Glancy.Settings.Localization;

/// <summary>
/// XAML text from AppStrings in the UI language: <c>Text="{l:Str Key=Home_Title}"</c>. A missing key shows as
/// <c>[Key]</c> so it is visible in captures (CoreStringsTests checks that every translation has every key).
/// </summary>
[MarkupExtensionReturnType(ReturnType = typeof(string))]
public sealed partial class Str : MarkupExtension
{
    public string Key { get; set; } = string.Empty;

    protected override object ProvideValue() =>
        AppStrings.ResourceManager.GetString(Key, AppStrings.Culture) ?? $"[{Key}]";
}
