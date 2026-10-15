# Architecture

Glancy has three components that share one format catalog and talk only through the registry:

| Component | What it is |
|---|---|
| `src/Glancy.Thumbnails` | The **thumbnail engine**: `Glancy.Thumbnails.dll`, a COM `IThumbnailProvider` in Rust that File Explorer loads, plus the diagnostic tool `glancy-thumb.exe`. |
| `src/Glancy.Core` | A .NET library with no UI: the format catalog, the settings model, pending changes, the registry store, import/export and shell services (engine status, Explorer refresh, caches, icons, Jump List). |
| `src/Glancy.Settings` | The **settings app** "Glancy": WinUI 3 on the Windows App SDK, .NET 10. |

Why it is built this way:

* **The engine is native and separate from .NET.** It runs inside `explorer.exe`, so it must not bring a runtime with
  it, and a failure in the UI can never affect Explorer. The app and the engine share only the registry contract
  ([registry-contract.md](registry-contract.md)) and the engine's `history.log`.
* **One catalog.** `src/Glancy.Core/Settings/formats.json` is embedded in the app as a resource and in the engine with
  `include_str!`; a test requires every format's decoder to be wired up.
* **The app is unpackaged** (`WindowsPackageType=None`): no MSIX to sign and no store dependency. It references only the
  Windows App SDK component packages it needs (Base, Foundation, WinUI, DWrite, InteractiveExperiences, Runtime), not
  the metapackage, which would add about 170 MB of AI, ML, search and widgets. There is no MVVM framework or toolkit:
  the settings controls and `ObservableObject` are small and local. `Glancy.Core` does not reference WinUI.
* **Everything is per user.** Settings live in `HKCU`, the engine is registered in `HKCU\Software\Classes` and installed
  in `%LOCALAPPDATA%\Programs\Glancy\Engine`: no administrator rights. The consequences are explained under
  [Registration](#registration).
* **One way to apply changes.** Everything that affects Explorer collects as pending changes and is applied with one
  button (which increments `Revision`); what only affects the app (theme, language) is immediate.

## Formats and decoders

The engine draws thumbnails for **87 formats and 265 extensions**. Each entry in `formats.json` has an id, name,
category, extensions, capabilities, defaults and, for the engine only, the decoder that draws it. The engine reads the
settings from `HKCU\Software\Glancy\Thumbnails`, applies the appearance (background, checkerboard, square canvas,
gamma…) and logs every thumbnail to `%LOCALAPPDATA%\Glancy\Logs\history.log`.

Formats that Windows already draws (JPEG, PNG, GIF, BMP, TIFF, WebP, JPEG XL/XR, RAW, icons, modern Office, HTML/MHT,
ZIP/RAR, `.thm`) default to **System provider** mode: Glancy passes the request to the previous provider (Windows,
Edge, Office…) and draws the thumbnail itself only if there is none or it fails. Switching the mode to **Glancy**
applies the appearance settings to those formats too.

| Family (decoder) | Formats | How |
|---|---|---|
| Adobe (`psd`, `ai`, `eps`) | PSD/PSB/PDD/PSDT, AI, EPS/PS | Own PSD reader (composite image row by row with box downsampling); PDFium; Ghostscript ≥ 10.03.1 as a separate process; embedded previews (JPEG, XMP, TIFF/WMF/EPSI). |
| PDF and SVG | PDF, SVG/SVGZ | PDFium (20 s, progressive); resvg without external references and with a cap on `<use>`. |
| Windows Imaging Component (`wic`, `ico`, `webp`, `jxl`) | JPEG, HEIF/AVIF, camera RAW, JPEG XR, THM, PNG, GIF, BMP/DIB, TIFF, ICO/CUR, WebP, JPEG XL | Windows codecs with scaling in the decoder and EXIF orientation; RAW through the camera's preview; pure-Rust fallbacks for WebP (`image-webp`), JPEG XL (`jxl-oxide`) and unusual icons. |
| `image` | TGA, OpenEXR, Radiance HDR/XYZE, Netpbm (PBM…PAM, PFM) | The `image` crate; HDR with automatic exposure and sRGB. |
| `raster` | PCX/DCX, WBMP, DPX, Cineon, FITS, TIM/TIM2, Amiga IFF (ILBM/PBM/HAM/EHB, Maya IFF), C64, MacPaint, Photo CD, SGI, AVS, AAI, IPL, SSTV, 1st Publisher | Own readers, row by row with memory and time limits. |
| Technical | DDS, KTX/KTX2, PVR, Fax G3, CALS, JPEG 2000/PGX, JBIG, PICT, DICOM, WMF/EMF/WMZ/EMZ | `texture2ddecoder` plus an own BC6H; `fax`; `hayro-jpeg2000`; own JBIG1 (T.82); GDI on two backgrounds to recover metafile transparency. |
| Containers | Office (OOXML and 97–2003), Publisher, 3ds Max, iWork, XD, Krita/ORA, SketchUp LayOut/materials, ZIP/CBZ, RAR/CBR (uncompressed), EPUB, MOBI, Blender, InDesign, CorelDRAW | The preview each format stores: `docProps/thumbnail.*`, `PIDSI_THUMBNAIL` in `SummaryInformation`, `QuickLook`, `mergedimage.png`, the OPF cover, EXTH 201, the `TEST` block, XMP, `DISP`… Own ZIP reader with limits. |
| `scan` | Affinity, Xara, Cinema 4D, SketchUp, Maya, Kubotek, SmartCAM, SoftImage, VinylMaster, Pulse | Looks for embedded PNG/JPEG/BMP images (largest first, with limits). |
| Design | GIMP (XCF), Paint Shop Pro, ABR brushes | Own layer compositing (XCF), composite bank or layers (PSP), brush tips in a grid. |
| Drawn by Glancy | Text (TXT, LOG, MD, JSON, NFO, REG…), XML, RTF, EML, HTML/MHT, WAV, PES embroidery, 3MF | An A4 page drawn with GDI (detected encoding, CP437 for NFO), waveform, stitches in their thread colors, 3D mesh with an own rasterizer when there is no thumbnail. |

Format limits (the history explains each case):

* PGF is not supported: there is no public specification apart from libPGF's C++ code, and no test files.
* Compressed RAR/CBR: there is no pure-Rust RAR decompressor; only uncompressed RAR is read.
* KTX2 with Basis Universal (BasisLZ/UASTC), "twiddled" PVR v2, C64 ESH/SHI and tile-compressed FITS: a clear error.
* Office and Publisher files without a stored thumbnail (normal for Word and Excel): Glancy defers to the system
  provider and, if there is none, Explorer shows the icon.
* Checked with real files: PSD, AI, PDF, SVG, JPEG, PNG, GIF, BMP, WebP, ICO, NEF, HTML, TXT, JSON, MD, XML, LOG, WAV,
  ZIP, PPTX, ABR, RTF. The rest is tested with synthetic files (and PCX/SGI/DDS against files generated with Pillow);
  JBIG, Maya IFF and the `scan` formats have not been checked against real files.

## Decision order

For every thumbnail: master switch → excluded folders → format mode (`mode`, or `smallIcons` if Explorer asks for
≤ 32 px) → size limits (`maxFileSizeMB`, `maxPixels`) → embedded preview or rendering (`embedded`) → size (`scale`,
`sampling`) → appearance (`gamma`, `transparency`, `canvasColor`, `checkerColors`, `fullFill`, `checkerAnchor`,
`checkerScale`, `forceDesktopTransparency`). `adornment`, `showAssocIcon` and `customIcon` are drawn by Explorer: the
engine writes `Treatment` and `TypeOverlay` to `HKCU\Software\Classes\SystemFileAssociations\<ext>` when it registers
and whenever `Revision` changes.

Engine rules that the rest of the design relies on:

* It implements `IInitializeWithItem` and `IInitializeWithFile` and rejects with `STG_E_ACCESSDENIED` any open mode
  that includes writing (`grfMode & 3`).
* The final HBITMAP is **premultiplied BGRA** with `WTSAT_ARGB` (`bitmap.rs`).
* System provider mode: ask the previous provider (initialized with the item or the file) and, if it fails, draw the
  thumbnail (`delegate.rs`). The previous provider is looked up in the saved backup, the ProgID,
  `SystemFileAssociations\<ext>` and `SystemFileAssociations\<PerceivedType>`, in Explorer's order.
* Off mode returns `E_FAIL`, so Explorer shows the file's icon.
* Desktop transparency covers both the user's and the public desktop (`SHGetKnownFolderPath`).
* The canvas anchor uses the contract's numbering: 0–8, row by row from the top left.

## Registration

* **Explorer gives the ProgID priority over the extension.** A handler registered only on
  `HKCU\Software\Classes\.psd\ShellEx` loses to one registered on the ProgID (for example `SageThumbsImage.psd`). Glancy
  registers on the effective ProgID, the `UserChoice` and default ProgIDs and the extension, and keeps each class's
  previous value in `HKCU\Software\Glancy\Engine\Extensions\<ext>\Locations\<n>`.
* **The isolated thumbnail process does not see CLSIDs registered only in HKCU.** With the default isolation the cache
  returned `REGDB_E_CLASSNOTREG`. Registered per user, the engine is marked `DisableProcessIsolation = 1` and loads
  inside Explorer. As a consequence a serious engine failure would affect Explorer, which is why every panic is caught
  at the COM boundary, PDFium and Ghostscript have time limits, Ghostscript runs in its own process and reads are capped
  (see [Safety](#safety)). A **per-machine** installer (HKLM, with administrator rights) could isolate it again.
* **`IInitializeWithStream` is not offered.** The thumbnail cache prefers it, and the stream only carries the file name
  (`Stat` → "icon2.psd"). Without the path, excluded folders, desktop transparency and the OneDrive protection would not
  work, and the history would not have the full path. With `IInitializeWithItem` the history gets `C:\…\icon2.psd`.
  For items without a path (inside ZIP files, phones) the engine asks the item itself for a stream.
* **Cloud-only OneDrive files** (`FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`) are not read, so they are not downloaded; the
  history logs them as errors.
* **The ProgID that counts is the one Windows returns.** Windows 11 stores the default app in `UserChoiceLatest`, and
  some apps register their own ProgID (`Google.PhotoViewer.3.0`, `Antigravity.json`). Registration asks
  `AssocQueryString(ASSOCSTR_PROGID)` for the effective ProgID and, when it finishes, checks with
  `AssocQueryString(ASSOCSTR_SHELLEXTENSION)` which provider Explorer resolves for each extension; those that do not
  resolve to Glancy are listed in `HKCU\Software\Glancy\Engine\NotResolved`. The app uses the same query.
* **Never on generic classes.** For unassociated extensions Windows returns `Unknown` as the ProgID; registering there
  would make Glancy the provider for every unknown file. `Unknown`, `*`, `AllFilesystemObjects`, `Folder`, `Directory`,
  `Drive` and `SystemFileAssociations\…` are excluded, and cleaned up if an earlier version touched them.
* **Other extensions that share a ProgID** (for example `MSEdgeHTM`, used by `.svg` and `.html`) are handed back to the
  previous provider without logging them in the history.

## Safety

The engine runs inside Explorer. An independent code review looked for inputs that could hang or crash it; everything
it found is fixed and covered by tests:

* PSD/PSB: offsets read from the file are added with checks (a block with a crafted 64-bit length caused an endless
  loop); RLE rows larger than the PackBits maximum are rejected (they used to reserve up to 4 GB); reading stops at the
  end of the file (a 200-byte PSB declaring 300,000 × 300,000 walked every row) and has a 20 s limit.
* Memory: box downsampling works on the original bytes (no full-size `f32` copy), WIC previews are capped at 32 Mpx and
  any result larger than the requested size is reduced.
* Ghostscript: a minimum version, `gswin64c.exe` out of process in a Job with a memory limit and no inherited handles. A
  PostScript file that hangs Ghostscript cannot block Explorer. Glancy does not bundle or download it (it is AGPL).
* Delegation: a per-thread flag stops two providers from delegating to each other until the stack overflows.
* Registration: if any restore fails while unregistering, the CLSID and the backups are kept.
* Other extensions sharing a ProgID are never identified by content.
* Release builds use `overflow-checks`: an overflow is a panic, and every panic is caught at the COM boundary.

## Building and installing

```bash
powershell -NoProfile -ExecutionPolicy Bypass -File tools/build-engine.ps1
```

```bash
powershell -NoProfile -ExecutionPolicy Bypass -File tools/install-engine.ps1
```

`build-engine.ps1` runs the tests, builds in Release and puts in `dist/Engine` the DLL, `pdfium.dll` (pinned version
`chromium/8100` from bblanchon/pdfium-binaries, checked by SHA-256), `glancy-thumb.exe` and the licenses.
`install-engine.ps1` copies to `%LOCALAPPDATA%\Programs\Glancy\Engine` and runs `regsvr32` (per user, no administrator);
with `-Uninstall` it unregisters and restores the previous providers. If Explorer has the DLL loaded, the old one is
renamed to `*.old-<date>` and deleted on the next install.

The setup (`tools/build-installer.ps1` → `installer/Glancy.iss`, Inno Setup 6) does the same for end users and adds the
self-contained app: per user in `%LOCALAPPDATA%\Programs\Glancy` (app at the root, engine in `Engine\`). Because Explorer
keeps the engine loaded, the setup never overwrites or deletes it in place. On upgrade, files that cannot be deleted are
renamed to `*.old-<date>` (removed by the next install). On uninstall it runs `Glancy.exe --cleanup` (Jump List,
notification identity) and `regsvr32 /u`, then moves locked files to the temp folder and deletes them at the next
sign-in through `HKCU\…\RunOnce`. Restart Manager may close only `Glancy.exe`, never Explorer. If unregistering fails,
the setup keeps the engine's backups (`HKCU\Software\Glancy\Engine`) even when asked to delete the settings.
`tools/installer-e2e.ps1` tests all of this on the real system.

* `regsvr32 /u` restores each key to its previous value (or deletes it if it did not exist) and `Treatment`/`TypeOverlay`.
* `regsvr32 /n /i:sync Glancy.Thumbnails.dll` applies `Treatment`/`TypeOverlay` from the settings at once (otherwise
  the engine does it on the next thumbnail after `Revision` changes).
* Toolchain: Rust `x86_64-pc-windows-gnu` per user, no Visual Studio. The `windows` crate stays at 0.58 because 0.59+
  needs `dlltool` with an assembler, which that toolchain does not include. The DLL imports only system DLLs.

## Testing

| Test | What it covers |
|---|---|
| `cargo test` (300) | PSD in every mode and depth, PackBits, PSB, resources, truncated and crafted files; resampling with alpha; compositing; settings and validation; history and rotation; SVG (opens no external files, rejects `<use>` bombs); Ghostscript (minimum version, contained process); EPS/EPSI/DOS; PDF with real PDFium (rotation, corrupt); premultiplied HBITMAP; GUIDs. |
| `tools/engine-e2e.ps1` (15) | The installed DLL through COM with real registry settings (copied and restored exactly): transparency, full canvas, per-extension settings, off mode, excluded folder, master switch, small icons, corrupt file, `history.log` format. |
| `glancy-thumb shell` | Extraction forced through the Windows thumbnail cache (`IThumbnailCache`), the same path Explorer uses. |
| Multi-format sweep | 658 real files of 27 extensions (up to 40 per extension): no panics or hangs; the errors are files without an embedded thumbnail, ZIP files without images, compressed RAR or files with the wrong extension. |
| Real-file sweep | Every local PSD, AI, PDF and SVG (without downloading OneDrive files): 345 PDF, 34 SVG, 14 PSD (up to 212 MB), 12 AI: 405/405 without errors; at most 6.3 s (a 17 MB PDF plan), median < 160 ms. |

`glancy-thumb render|com|clsid|shell <file> [size] [out.png]` diagnoses a single file: `shell` asks the Windows cache,
like Explorer; `render`, `com` and `clsid` isolate the engine or compare it with another registered provider.

The .NET side: `dotnet test --project tests/Glancy.Core.Tests/Glancy.Core.Tests.csproj` (xUnit v3, uses its own test
registry key and deletes it) and `tools/e2e.ps1`, which drives the real app with UI Automation.

## Known limitations

* **EPS without a preview and AI files older than CS** need Ghostscript 10.03.1 or later (AGPL, not bundled). Without
  it the history logs "Ghostscript is not installed".
* **PSD without "Maximize compatibility" and without an embedded thumbnail**: there is no image to show (layers are not
  composited). ZIP-compressed composite image: the preview is used.
* **EPSI**: the specification does not say whether 0 is black or white; Glancy decides by looking at the border
  (normally white).
* **If the user changes an extension's default app**, the new `UserChoice` ProgID may have its own provider and win
  over Glancy: run `install-engine.ps1` (or `regsvr32`) again.
* **Small icons** (`smallIcons`): Explorer does not ask for thumbnails in the List and Details views; the setting only
  affects requests of ≤ 32 px. An own icon handler is left for later.
* **x64 only.** Dialogs in 32-bit applications will not show Glancy thumbnails.
