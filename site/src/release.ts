// The appcast is rewritten by scripts/release-app.sh before every site build,
// so the version and requirements shown next to the button come from it.
import appcast from "../public/appcast.xml?raw"

export const MAC_DMG = "/releases/Scrawl-latest-arm64.dmg"
export const MAC_ZIP = "/releases/Scrawl-latest-arm64.zip"
export const BREW = "brew install --cask kitlangton/tap/scrawl"
export const SKILL = "bunx skills add kitlangton/scrawl"

// Set this to the Windows build (e.g. "/releases/latest/Scrawl-windows-x64.zip"
// or an installer) when it ships. While it's null the site says "coming soon".
export const WINDOWS_DOWNLOAD: string | null = null

export type Release = { version: string; macos: number }

function latest(xml: string): Release {
  const items = xml.split("<item>").slice(1)
  let best: (Release & { build: number }) | null = null
  for (const item of items) {
    const tag = (name: string) => item.match(new RegExp(`<sparkle:${name}>([^<]+)</sparkle:${name}>`))?.[1]
    const build = Number(tag("version") ?? 0)
    const version = tag("shortVersionString")
    if (!version || (best && build <= best.build)) continue
    best = { build, version, macos: parseInt(tag("minimumSystemVersion") ?? "26", 10) }
  }
  return best ?? { version: "", macos: 26 }
}

export const RELEASE = latest(appcast)
