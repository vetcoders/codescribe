// swift-tools-version: 5.9

import PackageDescription

let package = Package(
    name: "CodeQLBuildStub",
    platforms: [
        .macOS(.v13),
    ],
    products: [
        .executable(name: "codeql-build-stub", targets: ["CodeQLBuildStub"]),
    ],
    targets: [
        .executableTarget(
            name: "CodeQLBuildStub",
            path: "Sources/CodeQLBuildStub"
        ),
    ]
)
