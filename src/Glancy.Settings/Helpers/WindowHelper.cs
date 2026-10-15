using System.Runtime.InteropServices;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.Storage;
using Windows.Storage.Pickers;
using Glancy.Settings.Resources;

namespace Glancy.Settings.Helpers;

/// <summary>Win32 glue for an unpackaged WinUI window: pickers, foreground activation and dialogs.</summary>
public static partial class WindowHelper
{
    public static IntPtr Hwnd => WinRT.Interop.WindowNative.GetWindowHandle(App.Current.Window!);

    public static XamlRoot XamlRoot => App.Current.Window!.Content.XamlRoot;

    public static ElementTheme CurrentTheme => ((FrameworkElement)App.Current.Window!.Content).ActualTheme;

    public static async Task<StorageFile?> PickOpenFileAsync(params string[] extensions)
    {
        var picker = new FileOpenPicker { SuggestedStartLocation = PickerLocationId.DocumentsLibrary, ViewMode = PickerViewMode.List };
        foreach (var e in extensions) picker.FileTypeFilter.Add(e);
        WinRT.Interop.InitializeWithWindow.Initialize(picker, Hwnd);
        return await picker.PickSingleFileAsync();
    }

    public static async Task<StorageFile?> PickSaveFileAsync(string suggestedName, string typeName, string extension)
    {
        var picker = new FileSavePicker { SuggestedStartLocation = PickerLocationId.DocumentsLibrary, SuggestedFileName = suggestedName };
        picker.FileTypeChoices.Add(typeName, [extension]);
        WinRT.Interop.InitializeWithWindow.Initialize(picker, Hwnd);
        return await picker.PickSaveFileAsync();
    }

    public static async Task<StorageFolder?> PickFolderAsync()
    {
        var picker = new FolderPicker { SuggestedStartLocation = PickerLocationId.ComputerFolder };
        picker.FileTypeFilter.Add("*");
        WinRT.Interop.InitializeWithWindow.Initialize(picker, Hwnd);
        return await picker.PickSingleFolderAsync();
    }

    /// <summary>True while a ContentDialog is open (WinUI allows only one at a time).</summary>
    public static bool IsDialogOpen { get; private set; }

    /// <summary>Shows a dialog themed like the window. Returns None without showing if another dialog is open.</summary>
    public static async Task<ContentDialogResult> ShowDialogAsync(ContentDialog dialog)
    {
        if (IsDialogOpen) return ContentDialogResult.None;
        dialog.XamlRoot = XamlRoot;
        dialog.RequestedTheme = ((FrameworkElement)App.Current.Window!.Content).RequestedTheme;
        dialog.Style ??= (Style)Application.Current.Resources["DefaultContentDialogStyle"];
        IsDialogOpen = true;
        try
        {
            return await dialog.ShowAsync();
        }
        finally
        {
            IsDialogOpen = false;
        }
    }

    /// <summary>Asks for confirmation of a consequential action. The primary button names the action.</summary>
    public static async Task<bool> ConfirmAsync(string title, string message, string action, bool destructive = false)
    {
        var dialog = new ContentDialog
        {
            Title = title,
            Content = new TextBlock { Text = message, TextWrapping = TextWrapping.Wrap },
            PrimaryButtonText = action,
            CloseButtonText = AppStrings.Common_Cancel,
            DefaultButton = destructive ? ContentDialogButton.Close : ContentDialogButton.Primary,
        };
        return await ShowDialogAsync(dialog) == ContentDialogResult.Primary;
    }

    public static void ForceForeground(IntPtr hwnd)
    {
        if (IsIconic(hwnd)) ShowWindow(hwnd, 9 /* SW_RESTORE */);
        SetForegroundWindow(hwnd);
    }

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool SetForegroundWindow(IntPtr hwnd);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool ShowWindow(IntPtr hwnd, int cmd);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool IsIconic(IntPtr hwnd);
}
