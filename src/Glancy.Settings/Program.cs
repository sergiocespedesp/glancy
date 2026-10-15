using System.ComponentModel;
using System.Runtime.InteropServices;
using Glancy.Core.Localization;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;

namespace Glancy.Settings;

public static partial class Program
{
    private const string InstanceKey = "Glancy.Settings.Main";

    [STAThread]
    private static int Main(string[] args)
    {
        WinRT.ComWrappersSupport.InitializeComWrappers();
        var options = LaunchOptions.Parse(args);
        if (options.Cleanup)
            return RunCleanup();
        ApplyLanguage(options.Language);
        if (options.Action is { } action)
            return RunQuickAction(action);

        // A second launch hands its arguments (e.g. --page psd) to the running window instead of opening another.
        if (!options.IsCapture && RedirectToExistingInstance())
            return 0;

        Application.Start(p =>
        {
            var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            _ = new App(options);
        });
        return Environment.ExitCode;
    }

    private static bool RedirectToExistingInstance()
    {
        var instance = AppInstance.FindOrRegisterForKey(InstanceKey);
        if (instance.IsCurrent)
        {
            instance.Activated += (_, e) => App.OnRedirectedActivation(e);
            return false;
        }

        AllowSetForegroundWindow((uint)instance.ProcessId);
        var activation = AppInstance.GetCurrent().GetActivatedEventArgs();
        Task.Run(() => instance.RedirectActivationToAsync(activation).AsTask()).Wait();
        return true;
    }

    /// <summary>The language currently applied (a <see cref="AppLanguage"/> code).</summary>
    public static string Language { get; private set; } = "en";

    /// <summary>
    /// Chooses the UI language before anything is created: <c>--lang</c>, else the preference, else Windows, else
    /// English. Sets the .NET UI culture (our strings), the process's preferred UI language (WinUI's <c>.mui</c>
    /// strings: text box menu, control type names) and WinUI's override (the strings in its PRI).
    /// </summary>
    private static void ApplyLanguage(string? commandLine)
    {
        var preference = commandLine ?? Services.AppServices.Preferences.Language;
        var windowsLanguages = Windows.System.UserProfile.GlobalizationPreferences.Languages;
        // The display language first: the preferred languages may be a tag no language pack has (es-419 → es-MX).
        var displayLanguage = System.Globalization.CultureInfo.CurrentUICulture.Name;
        Language = AppLanguage.Resolve(preference, windowsLanguages);
        AppLanguage.Apply(Language);
        var culture = AppLanguage.WinUICulture(Language, [displayLanguage, .. windowsLanguages]);
        // A double-null-terminated list: the marshaller adds the last null. The file pickers run in PickerHost.exe and
        // keep the Windows display language.
        if (!SetProcessPreferredUILanguages(MuiLanguageName, culture + "\0", out _))
            Services.Log.Error($"Couldn't set the process UI language to {culture}", new Win32Exception());
        try
        {
            Microsoft.Windows.Globalization.ApplicationLanguages.PrimaryLanguageOverride = Language;
        }
        catch (Exception ex) when (ex is COMException or InvalidOperationException)
        {
            // Without the override WinUI's built-in strings follow Windows; ours still use the chosen language.
            Services.Log.Error("Couldn't set WinUI's language override", ex);
        }
    }

    /// <summary>
    /// A Jump List action (<c>--action</c>). If the settings window is open its process runs it, so the window's
    /// session sees the change; otherwise it runs here without a window. This process never registers as the main
    /// instance, so a launch meanwhile still opens the window normally.
    /// </summary>
    private static int RunQuickAction(string action)
    {
        if (!Services.QuickActions.IsKnown(action)) return 2;
        var main = AppInstance.GetInstances().FirstOrDefault(i => i.Key == InstanceKey);
        if (main is not null)
        {
            var activation = AppInstance.GetCurrent().GetActivatedEventArgs();
            Task.Run(() => main.RedirectActivationToAsync(activation).AsTask()).Wait();
            return 0;
        }
        return Task.Run(() => Services.QuickActions.RunHeadlessAsync(action)).GetAwaiter().GetResult() ? 0 : 1;
    }

    /// <summary>
    /// <c>--cleanup</c>, run by the uninstaller: removes what Windows keeps for the app outside its folder (the Jump
    /// List and the notification identity). No window, no visible text. Exit code 0, or 1 if a step failed.
    /// </summary>
    private static int RunCleanup()
    {
        var ok = true;
        try
        {
            Task.Run(Glancy.Core.Shell.JumpList.DeleteAsync).GetAwaiter().GetResult();
        }
        catch (COMException ex)
        {
            Services.Log.Error("Couldn't delete the Jump List", ex);
            ok = false;
        }
        try
        {
            Services.Toast.Unregister();
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or System.Security.SecurityException or IOException)
        {
            Services.Log.Error("Couldn't remove the notification identity", ex);
            ok = false;
        }
        return ok ? 0 : 1;
    }

    private const uint MuiLanguageName = 0x8;

    [LibraryImport("kernel32.dll", SetLastError = true, StringMarshalling = StringMarshalling.Utf16)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool SetProcessPreferredUILanguages(uint flags, string languages, out uint count);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool AllowSetForegroundWindow(uint processId);
}

/// <summary>
/// Command-line options. <c>--page</c> deep-links; <c>--action</c> runs a Jump List action without a window;
/// <c>--capture*</c> render the UI to PNG for QA; <c>--cleanup</c> is for the uninstaller.
/// </summary>
public sealed record LaunchOptions
{
    public string? Page { get; init; }
    public string? Action { get; init; }
    public bool Cleanup { get; init; }

    /// <summary><c>--lang &lt;code&gt;</c>: UI language for this run only (captures, end-to-end tests).</summary>
    public string? Language { get; init; }
    public string? CapturePath { get; init; }
    public string? CaptureSetDirectory { get; init; }
    public string? Theme { get; init; }
    public int Width { get; init; } = 1240;
    public int Height { get; init; } = 840;
    public bool Demo { get; init; }

    public bool IsCapture => CapturePath is not null || CaptureSetDirectory is not null;

    public static LaunchOptions Parse(IReadOnlyList<string> args)
    {
        var o = new LaunchOptions();
        for (var i = 0; i < args.Count; i++)
        {
            string? Next() => i + 1 < args.Count ? args[++i] : null;
            o = args[i].ToLowerInvariant() switch
            {
                "--page" => o with { Page = Next() },
                "--action" => o with { Action = Next()?.ToLowerInvariant() },
                "--lang" => o with { Language = Next() },
                "--capture" => o with { CapturePath = Next() },
                "--capture-set" => o with { CaptureSetDirectory = Next() },
                "--theme" => o with { Theme = Next() },
                "--width" => o with { Width = int.TryParse(Next(), out var w) ? w : o.Width },
                "--height" => o with { Height = int.TryParse(Next(), out var h) ? h : o.Height },
                "--demo" => o with { Demo = true },
                "--cleanup" => o with { Cleanup = true },
                _ => o,
            };
        }
        return o;
    }

    public static LaunchOptions Parse(string commandLine) =>
        Parse(commandLine.Split(' ', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries));
}
