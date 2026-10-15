# Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by
[SignPath Foundation](https://signpath.org/).

> Status: Glancy has applied to the SignPath Foundation program. Until the application is approved, releases are
> unsigned (Glancy 1.0.0 is unsigned). This page describes how signed releases will be produced.

## What is signed

Only files built from this repository: `Glancy.exe`, `Glancy.dll` and `Glancy.Core.dll` (the settings app),
`Glancy.Thumbnails.dll` and `glancy-thumb.exe` (the thumbnail engine) and the installer `GlancySetup-<version>.exe`.
Third-party files shipped with Glancy keep their publishers' own signatures or none: the .NET runtime and the Windows
App SDK (Microsoft) and `pdfium.dll` (built by [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)).

## How releases are built

Every release is built from a tagged commit of this repository by GitHub Actions
([`.github/workflows/build.yml`](../.github/workflows/build.yml)) on a GitHub-hosted runner: the .NET and engine tests
run first, then the engine, the app and the installer are built. No release is built on a personal computer. SignPath
verifies that the files it signs come from that workflow and this repository, and every signing request is approved
manually by a project approver before it is signed.

## Team roles

| Role | Members |
|---|---|
| Committers and reviewers | [Sergio Cespedes](https://github.com/sergiocespedesp) |
| Approvers | [Sergio Cespedes](https://github.com/sergiocespedesp) |

Every change from someone outside the team (a pull request) is reviewed by a committer before it is merged. All team
members use multi-factor authentication on GitHub and SignPath.

## Privacy

This program will not transfer any information to other networked systems unless specifically requested by the user
or the person installing or operating it.

Glancy has no telemetry, update checks or online services. The settings app only opens web pages (source code,
license, donation pages) in your browser when you select those links. The thumbnail engine reads files on your computer
and never loads external resources: for example, SVG images are rendered without fetching linked files. Files stored
only in the cloud (OneDrive placeholders) are not read, so they are not downloaded.

## What the installer changes

The installer works per user, without administrator rights. It copies Glancy to `%LOCALAPPDATA%\Programs\Glancy` and
registers the engine as File Explorer's thumbnail provider for the file types it supports, for that user only, keeping
a copy of the previous providers. The installer says so before installing. Uninstalling from Settings › Apps restores
the previous providers and removes Glancy; it asks whether to delete your settings too. See
[architecture.md](architecture.md), Registration.

## Verifying a download

Right-click the file › Properties › Digital Signatures: the signer is "SignPath Foundation". In PowerShell:

```powershell
Get-AuthenticodeSignature .\GlancySetup-1.0.1.exe | Format-List Status, SignerCertificate
```

`Status` must be `Valid`. Download Glancy only from the [releases page](https://github.com/sergiocespedesp/glancy/releases).

## Reporting a problem

If you think a signed Glancy file is malicious or was not built from this repository, do not run it and open an
[issue](https://github.com/sergiocespedesp/glancy/issues) with where you downloaded it and its SHA-256
(`Get-FileHash <file>`). You can also report it to SignPath Foundation.
