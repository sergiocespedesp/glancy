# Settings contract: settings app ⇄ thumbnail engine

The app **writes**; the engine (`Glancy.Thumbnails.dll`, an `IThumbnailProvider` in Rust, `src/Glancy.Thumbnails`) **only
reads**. Everything lives under `HKEY_CURRENT_USER\Software\Glancy\Thumbnails`. Sources of truth:

* **`src/Glancy.Core/Settings/formats.json`**: the format catalog (87 formats, 265 extensions, 11 categories). The app
  embeds it as a resource (`Glancy.Core.Settings.formats.json`) and the engine with `include_str!`: one file for both.
  Per format: `id` (the key name), `name`, `monogram`, `color` (`AARRGGBB`), `category`, `description`, `extensions`
  (lower case, with the dot, each in exactly one format), optional `capabilities` and `defaults`; `decoder` is used only
  by the engine. The app validates the file when it loads it and the tests pin its totals.
* `src/Glancy.Core/Settings/Catalog.cs`: keys, values and defaults of every setting.
* `src/Glancy.Core/Storage/RegistrySettingsStore.cs`: the registry layout.

## Layout

```
HKCU\Software\Glancy\Thumbnails
│  Schema    REG_DWORD  1
│  Revision  REG_DWORD  incremented on every Apply (written last)
├─ Global\                       system-wide settings
└─ Formats\
   ├─ psd\                       the FULL set of the format's settings
   │  └─ Extensions\             may be missing if no extension has its own values
   │     └─ .psb\                ONLY the values that differ from psd
   ├─ ai\  eps\  pdf\  svg\  jpeg\  raw\  png\  blender\ …   one key per `id` in formats.json (87)
```

**Effective value for a file** `x.psb`: `Formats\psd\Extensions\.psb\<key>` if present, otherwise `Formats\psd\<key>`,
otherwise the format's default (`defaults`) or, failing that, the catalog default.

**Reloading**: the engine may cache the settings and reread them only when `Revision` changes.

## Per-format keys (`Formats\<id>`)

| Key | Type | Values | Default |
|---|---|---|---|
| `mode` | DWORD | 0 off · 1 Glancy · 2 system provider | 1 (see next table) |
| `smallIcons` | DWORD | same as `mode` | 0 |
| `transparency` | DWORD | 0 opaque · 1 canvas color · 2 transparent · 3 checkerboard | 2 (see next table) |
| `canvasColor` | DWORD | ARGB | 0xFFFFFFFF |
| `checkerColors` | SZ | `AARRGGBB;AARRGGBB` (light;dark) | `FFFFFFFF;FFCCCCCC` |
| `scale` | DWORD | 0 actual size · 1 enlarge to fit | 0 |
| `fullFill` | DWORD | 0 no · 1 transparent canvas · 2 checkerboard · 3 color | 0 |
| `adornment` | DWORD | 0 none · 1 shadow (drawn by Explorer through `WTSAT_*`) | 1 |
| `embedded` | DWORD | 0 render · 1 always use the embedded preview · 2 use it if ≥ the requested size | 0 |
| `maxFileSizeMB` | DWORD | 0 no limit, 50…2048 | 0 |
| `maxPixels` | DWORD | 0 no limit, 4096…32768 | 0 |
| `showAssocIcon` | DWORD | 0/1 | 1 |
| `customIcon` | DWORD | 0/1 | 0 |
| `customIconPath` | SZ | `path,index` | "" |
| `sampling` | DWORD | 0 nearest · 1 bilinear · 2 bicubic | 2 |
| `gamma` | DWORD | ×100: 100 (no correction), 180, 220, 240 | 100 |
| `alpha` | DWORD | 0 straight · 1 premultiplied | 0 |

**Per-format defaults** (`defaults` in formats.json; everything else uses the table above). The app applies them when
it creates the configuration, when it resets one format or all of them, when it imports a file and when it decides
whether a value "is the default":

| Key | Value | Formats |
|---|---|---|
| `mode` | 2 (system provider first; Glancy draws the thumbnail only if there is no other provider or it fails) | jpeg, raw, jxl, jxr, thm, png, gif, webp, bmp, tiff, ico, cursor, officexml, web, mhtml, archive |
| `transparency` | 1 (canvas color) | ai, eps, pdf, indesign, g3, cals, officexml, office, publisher, rtf, text, xml, email, web, mhtml, macpaint, firstpublisher |

**Capabilities** (`capabilities`): a setting that needs a capability is shown and saved only for the formats that
declare it. `embedded` → `embedded` (embedded preview) · `alpha` → `alpha` · `sampling` → `sampling` ·
`gamma` → `gamma` · `pixelLimit` → `maxPixels`. Examples: psd = all five; ai = embedded, gamma; pdf = gamma;
svg and the 3D formats = none.

## Global keys (`Global`)

| Key | Type | Meaning | Default |
|---|---|---|---|
| `enabled` | DWORD | Master switch | 1 |
| `forceDesktopTransparency` | DWORD | On the desktop, always a transparent background | 0 |
| `refreshOnApply` | DWORD | The app refreshes open windows on Apply (does not affect the engine) | 1 |
| `contextMenu` | DWORD | 0 off · 1 expanded · 2 submenu | 2 |
| `contextMenuShift` | DWORD | Only with Shift | 0 |
| `folderRecursion` | DWORD | Folder actions are recursive | 0 |
| `checkerAnchor` | DWORD | 0–8 (3×3 grid, row by row from the top left) | 0 |
| `checkerScale` | DWORD | 0 = 4 px · 1 = 8 px · 2 = 16 px | 1 |
| `checkerLibrary` | MULTI_SZ | `AARRGGBB;AARRGGBB` pairs (UI only) | 3 grays |
| `blockedFolders` | MULTI_SZ | Paths; the engine returns `E_FAIL` for files inside them | empty |
| `debugLog` | DWORD | The engine logs to `%LOCALAPPDATA%\Glancy\Logs` | 0 |

## Engine registration (what the app checks, without writing)

* Provider CLSID: `{6C3A8B2E-5F1D-4E7A-9B0C-2D8E4F61A7C5}` with `InprocServer32` pointing at the DLL
  (`HKCU\Software\Classes\CLSID`, `ThreadingModel = Apartment`, `DisableProcessIsolation = 1`).
* For every extension in formats.json (265), the CLSID in
  `HKCU\Software\Classes\<class>\ShellEx\{e357fccd-a995-4576-b01f-234630154e96}` for four classes: the effective ProgID
  returned by `AssocQueryString(ASSOCSTR_PROGID)` (which includes Windows 11's `UserChoiceLatest`), the `UserChoice`
  one, the default one and the extension itself. Never on generic classes (`Unknown`, `*`, `AllFilesystemObjects`…).
  Explorer gives the ProgID priority over the extension ([architecture.md](architecture.md), Registration).
* Check: the provider Explorer resolves is queried with `AssocQueryString(ASSOCSTR_SHELLEXTENSION, "{e357fccd-…}")`;
  the engine uses it when it registers and the app for its status.
* The engine keeps what it replaces in `HKCU\Software\Glancy\Engine` (the engine's key; the app does not write it):
  `DllPath`; `NotResolved` (extensions that still resolve to another provider after registering); per extension,
  `Extensions\<ext>\PreviousHandler` (the provider Explorer used, for `mode = 2`), `PreviousTreatment`,
  `PreviousTypeOverlay` and `Locations\<n>\{Key, PreviousUserValue}` (each class's value, restored by `regsvr32 /u`).
* Shadow and overlay icon: the engine writes `Treatment` (= `adornment`) and `TypeOverlay` (`""` if
  `showAssocIcon = 0`; `customIconPath` if `customIcon = 1`) to `HKCU\Software\Classes\SystemFileAssociations\<ext>`
  when it registers and whenever `Revision` changes. `regsvr32 /n /i:sync Glancy.Thumbnails.dll` does it at once.

## History of processed files (engine → app)

The engine appends one line per thumbnail to `%LOCALAPPDATA%\Glancy\Logs\history.log` (UTF-8, tab-separated), always,
even with `debugLog` off; it rotates the file past ~1 MB:

```
2026-10-14T01:00:05Z<TAB>ok|error<TAB>milliseconds<TAB>full path[<TAB>error detail]
```

The path is the file's full path. Requests the engine drops without reading the file (master switch off, format off,
excluded folder) are not logged: an excluded folder must leave no trace. On rotation the previous file becomes
`history.log.1`. With `debugLog = 1` the engine also writes `engine.log` in the same folder. Messages are in English.

The app reads the last 64 KB (`Glancy.Core.Shell.EngineHistory`) and shows the 12 most recent entries under
*Maintenance › Diagnostics*. Malformed lines are ignored.

## Export file (`.glancysettings`)

Readable JSON: `{ "product": "Glancy", "version": 1, "exported": "...", "global": {...}, "formats": { "psd": {...} }, "overrides": { ".psb": {...} } }`.
Import ignores unknown keys and invalid values (and counts them), and leaves the result as pending changes.
