import { expect, test } from "bun:test"
import { latestRelease } from "../src/appcast"
import { latestRelease as mapRelease } from "../scripts/feature-audit"

const item = (build: number, version: string, channel = "") => `<item>
  <pubDate>Thu, 22 Oct 2026 12:00:00 -0700</pubDate>
  <sparkle:version>${build}</sparkle:version>
  <sparkle:shortVersionString>${version}</sparkle:shortVersionString>
  ${channel ? `<sparkle:channel>${channel}</sparkle:channel>` : ""}
  <sparkle:minimumSystemVersion>15.0</sparkle:minimumSystemVersion>
</item>`

test("downloads and agent files ignore newer betas regardless of feed order", () => {
  const stable = item(16, "0.3.13")
  const beta = item(18, "0.3.14-beta.2", "beta")
  for (const xml of [stable + beta, beta + stable]) {
    expect(latestRelease(xml)).toEqual({ version: "0.3.13", build: 16, macos: 15 })
  }
})

test("promotion of the same build is the newest stable release", () => {
  expect(latestRelease(item(18, "0.3.14-beta.2", "beta") + item(18, "0.3.14") + item(16, "0.3.13")))
    .toEqual({ version: "0.3.14", build: 18, macos: 15 })
})

test("a beta-only feed does not advertise a beta as stable", () => {
  expect(latestRelease(item(18, "0.3.14-beta.2", "beta")).build).toBe(0)
})

test("the feature map also reports only stable releases", () => {
  expect(mapRelease(item(18, "0.3.14-beta.2", "beta") + item(16, "0.3.13")))
    .toEqual({ version: "0.3.13", date: "2026-10-22" })
  expect(mapRelease(item(18, "0.3.14-beta.2", "beta")))
    .toEqual({ version: "", date: "" })
})
