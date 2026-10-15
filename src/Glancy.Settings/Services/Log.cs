namespace Glancy.Settings.Services;

/// <summary>Minimal append-only log for the settings app (%LOCALAPPDATA%\Glancy\Logs\settings.log).</summary>
public static class Log
{
    public static string Directory { get; } =
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Glancy", "Logs");

    private static readonly object Gate = new();

    public static void Info(string message) => Write("INFO", message, null);

    public static void Error(string message, Exception? ex) => Write("ERROR", message, ex);

    private static void Write(string level, string message, Exception? ex)
    {
        try
        {
            lock (Gate)
            {
                System.IO.Directory.CreateDirectory(Directory);
                var line = $"{DateTimeOffset.Now:O} {level} {message}{(ex is null ? string.Empty : Environment.NewLine + ex)}{Environment.NewLine}";
                File.AppendAllText(Path.Combine(Directory, "settings.log"), line);
            }
        }
        catch (IOException)
        {
            // Logging must never take the app down.
        }
        catch (UnauthorizedAccessException)
        {
        }
    }
}
