using System.Globalization;

namespace Glancy.Core.Shell;

public sealed record HistoryEntry(DateTimeOffset Time, bool Succeeded, string Path, int Milliseconds, string? Detail);

/// <summary>
/// Reads the engine's processed-files history: <c>%LOCALAPPDATA%\Glancy\Logs\history.log</c>, one tab-separated
/// line per thumbnail — <c>ISO-8601 time, ok|error, milliseconds, full path[, detail]</c> (docs/registry-contract.md).
/// </summary>
public static class EngineHistory
{
    public static string FilePath { get; } = System.IO.Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Glancy", "Logs", "history.log");

    private const long TailBytes = 64 * 1024;

    /// <summary>The most recent entries, newest first. Malformed lines are skipped; a missing file yields none.</summary>
    public static IReadOnlyList<HistoryEntry> ReadRecent(int count, string? path = null)
    {
        path ??= FilePath;
        if (!File.Exists(path)) return [];
        try
        {
            using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            if (stream.Length > TailBytes) stream.Seek(-TailBytes, SeekOrigin.End);
            using var reader = new StreamReader(stream);
            var lines = reader.ReadToEnd().Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
            return lines.Reverse().Select(Parse).OfType<HistoryEntry>().Take(count).ToList();
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return [];
        }
    }

    internal static HistoryEntry? Parse(string line)
    {
        var parts = line.Split('\t');
        if (parts.Length < 4) return null;
        if (!DateTimeOffset.TryParse(parts[0], CultureInfo.InvariantCulture, DateTimeStyles.RoundtripKind, out var time)) return null;
        if (!int.TryParse(parts[2], NumberStyles.Integer, CultureInfo.InvariantCulture, out var ms)) return null;
        var ok = string.Equals(parts[1], "ok", StringComparison.OrdinalIgnoreCase);
        return new HistoryEntry(time, ok, parts[3], ms, parts.Length > 4 ? parts[4] : null);
    }
}
