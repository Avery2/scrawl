// swift-tools-version: 6.2
import PackageDescription

let package = Package(
  name: "ScrawlKit",
  // macOS only so `swift test` can run the platform-independent parts (the outbox) on a Mac;
  // everything UIKit is `#if os(iOS)`.
  platforms: [.iOS(.v18), .macOS(.v15)],
  products: [
    .library(name: "ScrawlKit", targets: ["ScrawlKit"]),
  ],
  targets: [
    .target(
      name: "ScrawlKit",
      path: "Sources/ScrawlKit",
      swiftSettings: [.swiftLanguageMode(.v5)]
    ),
    .testTarget(
      name: "ScrawlKitTests",
      dependencies: ["ScrawlKit"],
      path: "Tests/ScrawlKitTests",
      swiftSettings: [.swiftLanguageMode(.v5)]
    ),
  ]
)
