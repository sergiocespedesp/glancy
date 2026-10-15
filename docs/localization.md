# Localization

Glancy is written in English and translated into Spanish, German, French, Portuguese (Brazil), Japanese and Chinese
(Simplified). This page explains where the strings live, how the language is chosen and how to add or fix a
translation.

## Where strings live

| File | What it holds |
|---|---|
| `src/Glancy.Core/Resources/CoreStrings.resx` | Setting titles, descriptions and options, groups, format categories, format names and descriptions, values shown in summaries, cache and registry messages. |
| `src/Glancy.Settings/Resources/AppStrings.resx` | Everything else the app shows: pages, dialogs, notifications, the Jump List, About. |
| `CoreStrings.<lang>.resx`, `AppStrings.<lang>.resx` | One translation per language: `es`, `de`, `fr`, `pt-BR`, `ja`, `zh-Hans`. |

The `.resx` files without a language suffix are the English source. `src/Glancy.Core/Settings/formats.json` is the
format catalog shared with the engine; its names and descriptions are English and must match `CoreStrings.resx`
(a test checks it).

Not translated, on purpose: the engine's messages in `history.log` (technical diagnostics, English only), the logs,
the command line, brand names ("Glancy", "Adobe Photoshop", "CorelDRAW"…) and file extensions.

## Keys

| Pattern | Example |
|---|---|
| `Setting_<key>_Title`, `Setting_<key>_Description` | `Setting_transparency_Title` |
| `Setting_<key>_Option_<value>` (+ `_Description`) | `Setting_transparency_Option_3` = "Checkerboard" |
| `Group_<id>`, `Category_<id>` | `Category_photo` = "Photography" |
| `Format_<id>_Name` (only names that aren't brands), `Format_<id>_Description` | `Format_pdf_Name` = "PDF document" |
| `<Area>_<Thing>` for the UI | `Home_Subtitle`, `Maintenance_ClearThumbs` |

Rules for translators:

* Keep every placeholder: `{0}`, `{1}`, `{0:0.#}`. Their order may change.
* `*_One` / `*_Many` pairs are singular and plural.
* `*_Keywords` are search synonyms (space-separated), not visible text.
* `Sample_*` are file base names for the preview (one lowercase word).
* `List_Separator`, `List_LastSeparator`, `List_End` join lists ("A, B and C."); `List_LowercaseFollowingItems` is
  `true` when items after the first start in lowercase (English, Spanish, French, Portuguese) and `false` otherwise.
* Follow Microsoft's terminology for Windows in your language (glossary below), sentence case where the language uses
  it, and keep button labels short.

## How the language is chosen

At start-up (`Program.ApplyLanguage`): `--lang <code>` if given, else the "Language" preference
(`HKCU\Software\Glancy\App\Language`, empty = automatic), else the first Windows display language Glancy supports,
else English. Regional variants map to the supported language (`es-MX` → `es`, `pt-PT` → `pt-BR`, `zh-CN` →
`zh-Hans`). The choice sets three things:

* the .NET UI culture, for our strings;
* the process's preferred UI language (`SetProcessPreferredUILanguages`), for WinUI's `.mui` strings: the text box
  menu's Cut/Copy/Paste and the control type names screen readers announce. The culture comes from
  `AppLanguage.WinUICulture`: the Windows display language (or else a preferred Windows language) if it is one of the
  language's shipped variants (`es-MX` for Spanish on a Latin American Windows), else the default one (`es-ES`, `en-US`, `de-DE`, `fr-FR`, `pt-BR`, `ja-JP`,
  `zh-CN`);
* WinUI's `PrimaryLanguageOverride`, for the strings in WinUI's PRI (the title bar's "Back" and "Toggle Navigation",
  for example).

Windows' file and folder pickers run in their own process (`PickerHost.exe`), so they stay in the Windows display
language whatever Glancy's language is. Number and date formats keep following Windows. Changing the language in Preferences asks for a restart.

The app ships WinUI's `.mui` files only for the cultures in `AppLanguage.WinUICultures` (`GlancyWinUILanguages` in
`Glancy.Settings.csproj`, which removes the other ~76 when building; a test keeps both lists equal).

## Adding a language

1. Add it to `AppLanguage.Supported` (`src/Glancy.Core/Localization/AppLanguage.cs`) with its native name, and to
   `SatelliteResourceLanguages` in `src/Glancy.Settings/Glancy.Settings.csproj`. List the cultures whose WinUI `.mui`
   files it uses in `AppLanguage.WinUICultures`, the default first (for example `it-IT`), and add them to
   `GlancyWinUILanguages` in the same file, spelled like the folders the Windows App SDK publishes.
2. Copy `CoreStrings.resx` and `AppStrings.resx` to `CoreStrings.<code>.resx` and `AppStrings.<code>.resx` and
   translate the values (Weblate and Crowdin read `.resx`).
3. Run the tests: `CoreStringsTests` fails if a key is missing, empty or has different placeholders.
4. Check the layout: `Glancy.exe --capture-set <folder> --lang <code>` renders every page in light and dark.

## Glossary

| English | Español | Deutsch | Français | Português (Brasil) | 日本語 | 简体中文 |
|---|---|---|---|---|---|---|
| File Explorer | Explorador | Datei-Explorer | Explorateur de fichiers | Explorador de Arquivos | エクスプローラー | 文件资源管理器 |
| thumbnail | miniatura | Miniaturansicht | miniature | miniatura | 縮小版 | 缩略图 |
| thumbnail cache | caché de miniaturas | Miniaturansichtencache | cache des miniatures | cache de miniaturas | 縮小版のキャッシュ | 缩略图缓存 |
| icon cache | caché de iconos | Symbolcache | cache des icônes | cache de ícones | アイコン キャッシュ | 图标缓存 |
| Disk Cleanup | Liberador de espacio en disco | Datenträgerbereinigung | Nettoyage de disque | Limpeza de Disco | ディスク クリーンアップ | 磁盘清理 |
| Jump List | Jump List | Sprungliste | liste de raccourcis | Lista de Atalhos | ジャンプ リスト | 跳转列表 |
| context menu | menú contextual | Kontextmenü | menu contextuel | menu de contexto | コンテキスト メニュー | 上下文菜单 |
| pending changes | cambios pendientes | ausstehende Änderungen | modifications en attente | alterações pendentes | 保留中の変更 | 待应用的更改 |
| Apply / Discard | Aplicar / Descartar | Übernehmen / Verwerfen | Appliquer / Ignorer | Aplicar / Descartar | 適用 / 破棄 | 应用 / 放弃 |
| engine | motor | Engine | moteur | mecanismo | エンジン | 引擎 |
| checkerboard | cuadrícula | Schachbrettmuster | damier | quadriculado | 市松模様 | 棋盘格 |
| file extension | extensión | Erweiterung | extension | extensão | 拡張子 | 扩展名 |
| Preferences | Preferencias | Einstellungen | Préférences | Preferências | 基本設定 | 首选项 |

The translations other than Spanish were machine-made with this glossary and still need review by native speakers.
