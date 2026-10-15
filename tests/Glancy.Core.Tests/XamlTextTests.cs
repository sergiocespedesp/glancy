using System.Text.RegularExpressions;

namespace Glancy.Core.Tests;

/// <summary>Every visible text in the app's XAML comes from AppStrings ({l:Str}) or a binding, never a literal.</summary>
public partial class XamlTextTests
{
    [Fact]
    public void Xaml_HasNoLiteralVisibleText()
    {
        var root = Repo.Path("src", "Glancy.Settings");
        var files = Directory.GetFiles(root, "*.xaml", SearchOption.AllDirectories)
            .Where(f => !f.Contains($"{Path.DirectorySeparatorChar}obj{Path.DirectorySeparatorChar}"))
            .ToList();
        Assert.NotEmpty(files);

        var offenders = new List<string>();
        foreach (var file in files)
        {
            var text = File.ReadAllText(file);
            foreach (Match m in VisibleTextAttribute().Matches(text))
            {
                var value = m.Groups["value"].Value;
                if (value == "Glancy" || !value.Any(char.IsLetter)) continue;
                var line = text[..m.Index].Count(c => c == '\n') + 1;
                offenders.Add($"{Path.GetRelativePath(root, file)}:{line}: {m.Value}");
            }
        }
        Assert.True(offenders.Count == 0, "Literal UI text in XAML:\n" + string.Join("\n", offenders));
    }

    [GeneratedRegex(@"\b(?<attr>Text|Header|Content|PlaceholderText|Title|Subtitle|Message|Description|ToolTipService\.ToolTip|AutomationProperties\.Name|AutomationProperties\.HelpText|OnContent|OffContent|Label|PrimaryButtonText|SecondaryButtonText|CloseButtonText|CloseButtonContent|ActionButtonContent)=""(?<value>[^""{][^""]*)""")]
    private static partial Regex VisibleTextAttribute();
}
