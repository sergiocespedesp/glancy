using System.Globalization;
using System.Text;

namespace Glancy.Settings.Helpers;

public static class TextSearch
{
    /// <summary>Lower-cases and strips diacritics so "cafe" matches "Café" and "ai" matches "AI".</summary>
    public static string Normalize(string text)
    {
        var decomposed = text.Normalize(NormalizationForm.FormD);
        var sb = new StringBuilder(decomposed.Length);
        foreach (var ch in decomposed)
            if (CharUnicodeInfo.GetUnicodeCategory(ch) != UnicodeCategory.NonSpacingMark) sb.Append(char.ToLowerInvariant(ch));
        return sb.ToString();
    }

    /// <summary>True if every word of the query appears in the text.</summary>
    public static bool Matches(string normalizedText, string query) =>
        Normalize(query).Split(' ', StringSplitOptions.RemoveEmptyEntries).All(normalizedText.Contains);
}
