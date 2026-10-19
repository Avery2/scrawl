// The appcast is rewritten by scripts/release-app.sh before every site build,
// so the version and requirements shown next to the button come from it.
import appcast from "../public/appcast.xml?raw"
import { latestRelease } from "./appcast"

export const MAC_DMG = "/releases/Scrawl-latest-arm64.dmg"
export const MAC_ZIP = "/releases/Scrawl-latest-arm64.zip"
export const BREW = "brew install --cask kitlangton/tap/scrawl"
export const SKILL = "bunx skills add kitlangton/scrawl"

// Set this to the Windows build (e.g. "/releases/latest/Scrawl-windows-x64.zip"
// or an installer) when it ships. While it's null the site says "coming soon".
export const WINDOWS_DOWNLOAD: string | null = null

export const RELEASE = latestRelease(appcast)
