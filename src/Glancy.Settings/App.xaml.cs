using Glancy.Settings.Services;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;
using Glancy.Settings.Resources;

namespace Glancy.Settings;

public partial class App : Application
{
    private readonly LaunchOptions _options;

    public App(LaunchOptions options)
    {
        _options = options;
        InitializeComponent();
        UnhandledException += OnUnhandledException;
    }

    public static new App Current => (App)Application.Current;

    public MainWindow? Window { get; private set; }

    private static string? _earlyRedirectPage;

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        AppServices.Initialize(demo: _options.Demo);
        Window = new MainWindow(_options);
        Window.Activate();
        if (Interlocked.Exchange(ref _earlyRedirectPage, null) is { } page) Window.BringToFront(page);
        if (!_options.IsCapture)
        {
            // The Jump List's on/off task names the current state: rewrite it when an apply changes it.
            AppServices.Session.Changed += (_, _) => _ = QuickActions.PublishAsync(AppServices.Session);
            _ = QuickActions.PublishAsync(AppServices.Session);
        }
    }

    /// <summary>Called on a background thread when a second launch redirects to this instance.</summary>
    internal static void OnRedirectedActivation(AppActivationArguments args)
    {
        var commandLine = args.Data is Windows.ApplicationModel.Activation.ILaunchActivatedEventArgs launch ? launch.Arguments : string.Empty;
        var options = LaunchOptions.Parse(commandLine);
        if (options.Action is { } action)
        {
            // A Jump List action: run it here, where the session lives, and report with a notification
            // (the window may be minimized or behind others, and is not brought forward).
            if (Current?.Window is { } w)
            {
                w.DispatcherQueue.TryEnqueue(async () =>
                {
                    var result = await QuickActions.RunAsync(action, AppServices.Session);
                    Toast.Show(result.Title, result.Message);
                });
            }
            return;
        }
        if (Current?.Window is null)
        {
            // The window is still being created; OnLaunched picks the page up...
            _earlyRedirectPage = options.Page ?? "home";
            // ...unless it was created in between, in which case deliver it here.
            if (Current?.Window is null || Interlocked.Exchange(ref _earlyRedirectPage, null) is not { } late) return;
            options = options with { Page = late };
        }
        var window = Current!.Window!;
        window.DispatcherQueue.TryEnqueue(() => window.BringToFront(options.Page));
    }

    private void OnUnhandledException(object sender, Microsoft.UI.Xaml.UnhandledExceptionEventArgs e)
    {
        Log.Error("Unhandled exception", e.Exception);
        if (_options.IsCapture) return;

        // Keep the window alive for recoverable UI errors and tell the user, instead of vanishing.
        e.Handled = true;
        Window?.Notify(Microsoft.UI.Xaml.Controls.InfoBarSeverity.Error,
            AppStrings.Error_Unexpected_Title,
            AppStrings.Error_Unexpected_Message);
    }
}
