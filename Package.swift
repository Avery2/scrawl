// swift-tools-version: 6.2
import PackageDescription

let package = Package(
  name: "ScrawlKit",
  platforms: [.iOS(.v18)],
  products: [
    .library(name: "ScrawlKit", targets: ["ScrawlKit"]),
  ],
  targets: [
    .target(
      name: "ScrawlKit",
      path: "Sources/ScrawlKit",
      swiftSettings: [.swiftLanguageMode(.v5)]
    ),
  ]
)
