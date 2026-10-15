using System.Security;
using Glancy.Core;
using Microsoft.Win32;
using Windows.UI.Notifications;

namespace Glancy.Settings.Services;

/// <summary>
/// Windows notifications for the Jump List actions, which have no window to report in. Uses the system toast API:
/// the Windows App SDK's AppNotificationManager fails in a self-contained unpackaged app (0x8007007E). An unpackaged
/// app needs its notification identity registered: name and icon under <c>HKCU\Software\Classes\AppUserModelId\Glancy</c>
/// (the uninstaller deletes that key through <c>--cleanup</c>).
/// </summary>
public static class Toast
{
    public const string AppUserModelId = "Glancy";

    public static void Show(string title, string message)
    {
        try
        {
            RegisterIdentity();
            var xml = new Windows.Data.Xml.Dom.XmlDocument();
            xml.LoadXml($"""
                <toast><visual><binding template="ToastGeneric">
                <text>{SecurityElement.Escape(title)}</text><text>{SecurityElement.Escape(message)}</text>
                </binding></visual></toast>
                """);
            ToastNotificationManager.CreateToastNotifier(AppUserModelId).Show(new ToastNotification(xml));
        }
        catch (Exception ex)
        {
            // A notification is a courtesy: the action itself already happened and is logged.
            Log.Error("Couldn't show the notification", ex);
        }
    }

    /// <summary>Removes the notification identity (the uninstaller runs this through <c>--cleanup</c>).</summary>
    public static void Unregister() =>
        Registry.CurrentUser.DeleteSubKeyTree($@"Software\Classes\AppUserModelId\{AppUserModelId}", throwOnMissingSubKey: false);

    /// <summary>Written on every use so it follows the app if its folder moves.</summary>
    private static void RegisterIdentity()
    {
        using var key = Registry.CurrentUser.CreateSubKey($@"Software\Classes\AppUserModelId\{AppUserModelId}");
        key.SetValue("DisplayName", AppInfo.ProductName);
        key.SetValue("IconUri", Path.Combine(AppContext.BaseDirectory, "Assets", "Glancy-64.png"));
    }
}
