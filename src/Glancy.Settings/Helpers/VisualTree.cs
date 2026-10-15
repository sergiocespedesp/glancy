using Glancy.Settings.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Glancy.Settings.Helpers;

public static class VisualTree
{
    public static IEnumerable<T> Descendants<T>(DependencyObject root) where T : DependencyObject
    {
        var count = VisualTreeHelper.GetChildrenCount(root);
        for (var i = 0; i < count; i++)
        {
            var child = VisualTreeHelper.GetChild(root, i);
            if (child is T match) yield return match;
            foreach (var nested in Descendants<T>(child)) yield return nested;
        }
    }

    /// <summary>Scrolls the card bound to <paramref name="key"/> into view and focuses its control. Returns false if not found.</summary>
    public static bool FocusSetting(DependencyObject root, string key)
    {
        var card = Descendants<SettingsCard>(root).FirstOrDefault(c => c.Item?.Key == key && c.Visibility == Visibility.Visible);
        if (card is null) return false;
        card.StartBringIntoView(new BringIntoViewOptions { VerticalAlignmentRatio = 0.3, AnimationDesired = true });
        (card.Action as Control)?.Focus(FocusState.Keyboard);
        return true;
    }
}
