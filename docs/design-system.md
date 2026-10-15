# Design system

The single source of truth for how Glancy looks. Values live in `src/Glancy.Settings/Styles/Tokens.xaml` (tokens) and
`Styles/Controls.xaml` (styles); components in `src/Glancy.Settings/Controls/`. Every text, surface and stroke color
comes from the system Fluent resources, so **light, dark and high contrast work with no extra code**.

## 1. Visual principles

| Principle | How it applies |
|---|---|
| Native before decorated | Standard WinUI controls (ComboBox, ToggleSwitch, InfoBar, NavigationView, TitleBar). No hand-drawn buttons. |
| Windows sets the color | Controls and focus use the user's accent color. The brand lives only in the icon, the Home hero and the format badges. Palette, symbol and logo: [brand-identity.md](brand-identity.md). |
| Layered surfaces | Mica on the window → content layer (`LayerFillColorDefault`) → cards (`CardBackgroundFillColorDefault`). No hard shadows. |
| Text you read, not icons you guess | Every option shows its value as text. Icons accompany, never replace. |
| Quiet motion | Only the pending-changes bar animates (167 ms, decelerate); it is off when Windows animations are off. |

## 2. Tokens

### 2.1 Typography (Segoe UI Variable)

| Use | Style | Font / size |
|---|---|---|
| Hero (Home) | `GlancyHeroTitleStyle` | Sora SemiBold, the brand typeface ([brand-identity.md](brand-identity.md)), only for the "Glancy" title |
| Page title | `GlancyPageTitleStyle` | Segoe UI Variable **Display**, 28 px semibold |
| Overline ("FORMAT", "GENERAL") | `GlancyOverlineStyle` | 12 px semibold, +60 tracking, secondary |
| Section header | `GlancySectionHeaderStyle` | 14 px semibold, 28 top margin |
| Body | `BodyTextBlockStyle` | Segoe UI Variable Text 14 px |
| Description / help | `GlancyCaptionStyle` | 12 px, `TextFillColorSecondary` |
| File extensions | — | Cascadia Mono / Consolas, so `.psd` stands apart from ordinary text |

### 2.2 Spacing (4 px grid)

`GlancySpace1…8` = 4, 8, 12, 16, 24, 32. Rules:

* Page: 36 px margin (16 px below 640 epx wide).
* Between cards in a group: **4 px** (the Windows 11 Settings pattern).
* Between groups: 28 px above the section header, 8 px below.
* Inside a card: 16 × 12 px; minimum height 68 px.
* Maximum content width: 1000 epx (long lines are tiring).

### 2.3 Shape

| Token | Value | Use |
|---|---|---|
| `GlancyCardCornerRadius` | 8 | Cards, InfoBar |
| `GlancyTileCornerRadius` | 12 | Format tiles, hero |
| Format badge | 24 % of the side | Monograms "Ps", "Ai"… |

### 2.4 Color

| Role | Resource |
|---|---|
| Primary / secondary text | `TextFillColorPrimaryBrush` / `TextFillColorSecondaryBrush` |
| Card surface | `CardBackgroundFillColorDefaultBrush` + `CardStrokeColorDefaultBrush` |
| Accent (Apply, selection) | Windows accent color (`AccentButtonStyle`) |
| Pending change (dot) | `GlancyPendingDotBrush` → `SystemAccentColorDark1` (light) / `Light2` (dark) / `Highlight` (high contrast) |
| Hero | `GlancyHeroBrush`: coral → amber gradient at low opacity (system window color in high contrast) |
| Explorer surface (preview) | `#FFFFFF` light / `#191919` dark, the same as File Explorer |

**Format badges.** Each format's brand color (`color` in `formats.json`) is darkened to 78 % so the white monogram
passes 4.5:1 (`ColorHelper.Darken` + `ContrastingText`); for example Photoshop `#31A8FF`, Illustrator `#FF9A00`,
EPS `#8E6CF5`, PDF `#E5484D`, SVG `#2BB673`. They are decorative (hidden from screen readers); the format name always
appears as text.

### 2.5 Iconography

* **Segoe Fluent Icons** (MDL2 fallback), 20 px in cards, 16 px in navigation and menus.
* One glyph per concept, never reused for a different concept (checked by rendering the glyphs).
* App icon (`Assets/Glancy.ico`, 16–256 px): the brand symbol, with a simplified drawing for 16 px
  ([brand-identity.md](brand-identity.md)).

## 3. Components

| Component | File | Use |
|---|---|---|
| `SettingsCard` | `Controls/SettingsCard.xaml` | Setting row: icon, title, description, control. Shows the **pending-change dot**, a **↺ button** when the value is not the default, a status note ("Differs between formats.", "Customized for .psb") and stacks the control under the text below 560 epx. With `Item=` it binds itself to a `SettingItem`. Passes name and description to UI Automation. |
| `PreviewPanel` + `ThumbnailTile` | `Controls/` | Simulated thumbnail at 256/96/48 px on a light or dark Explorer background. Compact mode in narrow windows. |
| `ColorButton` | `Controls/ColorButton.cs` | Swatch + hex; flyout with quick colors and a `ColorPicker`. |
| `CheckerButton` | `Controls/CheckerButton.cs` | Checkerboard library, editing both colors, swap, random, save to library. |
| `FormatBadge` | `Controls/FormatBadge.cs` | Format monogram (or the "all" icon); the font shrinks for 3–4 letters ("HEIC"). |
| `ExtensionList` | `Helpers/ExtensionList.cs` | Extensions in monospace, in fixed-width cells spread across rows: an extension is never split (Blender has 33, RAW 35). |
| Pending-changes bar | `MainWindow.xaml` | Counter, *Review* (readable list of changes), *Discard*, *Apply* with progress. |
| Notifications | `MainWindow.Notify` | InfoBars stacked at the top right; success/info close by themselves depending on text length (5–12 s), errors stay. At most 3. |
| Dialogs | `WindowHelper.ConfirmAsync` | The primary button names the action ("Restore", "Clear cache"). For destructive actions the default focus is on *Cancel*. |

## 4. Patterns

* **Page**: overline → title (level 1) → description → sections (level 2) → cards.
* **Formats (87)**: navigation groups them into 11 expandable categories (a category only expands, it has no page;
  opening a format expands its category and collapses the others). Home shows one section per category (level 3 header
  with counts) with compact tiles. With more than 5 extensions, "Apply settings to" is a ComboBox instead of a
  SelectorBar, and "Status in Windows" groups the extensions by provider.
* **Conditional setting**: settings that depend on another appear only when they matter (canvas color when the
  background is "Canvas color"; icon file when "Custom icon" is on).
* **Unsupported setting**: hidden, not disabled (SVG does not show "Embedded preview").
* **Mixed values** (*All formats* page): the control shows "Multiple values" as a placeholder and the card adds
  "Differs between formats.".
* **Text**: plain English, short sentences, sentence case, verbs on buttons ("Apply", "Export…"), an ellipsis when the
  action opens a dialog. Every visible string comes from the resource files ([localization.md](localization.md)).

## 5. Accessibility

* Every interactive control is native and reachable with Tab; the order follows reading order.
* Headers set `AutomationProperties.HeadingLevel` (heading navigation in Narrator).
* Each control in a card gets the card title as accessible name and its description as help text (plus "pending
  change" when it applies).
* Click targets ≥ 32 epx (≥ 36 for icon buttons).
* Contrast: system text (≥ 4.5:1); monograms computed for ≥ 4.5:1.
* High contrast: custom brushes have a `HighContrast` variant with system colors.
* Animations only when `UISettings.AnimationsEnabled`.
