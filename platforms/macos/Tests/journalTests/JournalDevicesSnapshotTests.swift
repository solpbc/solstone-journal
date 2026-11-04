// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import Foundation
import JournalMarkKit
import SwiftUI
import Testing
@testable import journal

private func runDevicesSnapshotGit(_ args: String...) throws -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/git")
    process.arguments = Array(args)
    let pipe = Pipe()
    process.standardOutput = pipe
    try process.run()
    process.waitUntilExit()
    return String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)!
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

private func makeDevicesSnapshotOutputDir() throws -> URL {
    // Renders land under this package (platforms/macos/snapshots, ignored), not the
    // repository root: the package root and the Git root are different directories.
    let repoRoot = journalPackageRoot().path
    let gitHash = try runDevicesSnapshotGit("rev-parse", "--short", "HEAD")

    let dirtyCheck = Process()
    dirtyCheck.executableURL = URL(fileURLWithPath: "/usr/bin/git")
    dirtyCheck.arguments = ["diff", "--quiet", "HEAD"]
    dirtyCheck.currentDirectoryURL = URL(fileURLWithPath: repoRoot)
    try dirtyCheck.run()
    dirtyCheck.waitUntilExit()
    let isDirty = dirtyCheck.terminationStatus != 0

    let formatter = DateFormatter()
    formatter.dateFormat = "yyyy-MM-dd_HHmmss"
    let dirName = "\(formatter.string(from: Date()))_\(gitHash)\(isDirty ? "-dirty" : "")"

    let url = URL(fileURLWithPath: repoRoot).appendingPathComponent("snapshots/\(dirName)")
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

@Suite("JournalDevicesSnapshot")
@MainActor
struct JournalDevicesSnapshotTests {
    private static nonisolated(unsafe) var _outputDir: URL?
    private let paneSize = CGSize(width: 560, height: 520)
    // Tall enough for the pairing sheet's persistent mark card (journal-mark.md section 7.3)
    // above the QR/link content, on top of the prior QR-only sheet's height.
    private let sheetSize = CGSize(width: 460, height: 720)

    private var outputDir: URL {
        get throws {
            if let dir = Self._outputDir { return dir }
            let dir = try makeDevicesSnapshotOutputDir()
            Self._outputDir = dir
            return dir
        }
    }

    init() {
        _ = NSApplication.shared
        JournalMarkFont.register()
    }

    @Test func paneMatrix() async throws {
        try await renderDevicesList()
        try await renderDevicesEmpty()
        try await renderDevicesNotRunning()
        try await renderDevicesNotReady()
        try await renderPairingOpen()
        try await renderPairingPaired()
        try await renderPairingExpired()
    }

    private func renderDevicesList() async throws {
        let model = paneModel(
            state: .loaded,
            devices: [
                DeviceRow(
                    displayLabel: "iphone",
                    deviceLabel: "iphone",
                    role: "phone",
                    network: "local",
                    pairedAt: "2026-08-01T12:00:00Z",
                    lastSeenAt: "2026-08-01T12:00:00Z",
                    fingerprint: "phone-a"
                ),
                DeviceRow(
                    displayLabel: "iphone (2)",
                    deviceLabel: "iphone",
                    role: "phone",
                    network: "local",
                    pairedAt: "2026-07-30T12:00:00Z",
                    lastSeenAt: nil,
                    fingerprint: "phone-b"
                ),
                DeviceRow(
                    displayLabel: "kitchen journal",
                    role: "peer",
                    fingerprint: "peer-a"
                ),
            ]
        )
        try await renderPane(model, to: "journal-devices-list.png")
    }

    private func renderDevicesEmpty() async throws {
        let model = paneModel(state: .empty)
        try await renderPane(model, to: "journal-devices-empty.png")
    }

    private func renderDevicesNotRunning() async throws {
        let model = paneModel(state: .notRunning)
        try await renderPane(model, to: "journal-devices-not-running.png")
    }

    private func renderDevicesNotReady() async throws {
        let model = paneModel(state: .notReady)
        model.loadErrorDetail = "pairing routes are still starting"
        try await renderPane(model, to: "journal-devices-not-ready.png")
    }

    private func renderPairingOpen() async throws {
        let model = pairingModel()
        let link = "https://journal.example/pair/abc123"
        model.pairingNow = .seconds(40)
        model.pairingState = .open(
            link: link,
            qr: try JournalQRImage.make(from: link),
            deadline: .seconds(125),
            nonce: "nonce-a"
        )
        try await renderSheet(model, to: "journal-devices-pairing-open.png")
    }

    private func renderPairingPaired() async throws {
        let model = pairingModel()
        let device = NonceStatusResponse.Device(cid: "new-phone", displayLabel: "studio phone")
        model.pairingState = .paired(device: device, relay: false)
        try await renderSheet(model, to: "journal-devices-pairing-paired.png")
        model.pairingState = .paired(device: device, relay: true)
        try await renderSheet(model, to: "journal-devices-pairing-paired-relay.png")
        model.pairedUnpairState = .failed
        try await renderSheet(model, to: "journal-devices-pairing-paired-unpair-failed.png")
        model.pairedUnpairState = .removed
        try await renderSheet(model, to: "journal-devices-pairing-paired-unpaired.png")
    }

    private func renderPairingExpired() async throws {
        let model = pairingModel()
        model.pairingState = .expired(previousLink: "https://journal.example/pair/expired")
        try await renderSheet(model, to: "journal-devices-pairing-expired.png")
    }

    private func paneModel(
        state: JournalDevicesLoadState,
        devices: [DeviceRow] = []
    ) -> JournalDevicesModel {
        let model = JournalDevicesModel(client: SnapshotDevicesClient(devices: devices), copyToClipboard: { _ in })
        model.devices = devices
        model.loadState = state
        return model
    }

    private func pairingModel() -> JournalDevicesModel {
        let model = JournalDevicesModel(client: SnapshotDevicesClient(devices: []), copyToClipboard: { _ in })
        model.isPairingPresented = true
        model.markPresentation = .mark(.uiTestSample)
        return model
    }

    private func renderPane(_ model: JournalDevicesModel, to filename: String) async throws {
        try await render(
            JournalDevicesPane(model: model)
                .frame(width: paneSize.width, height: paneSize.height)
                .padding(24)
                .background(Color(nsColor: .windowBackgroundColor)),
            size: paneSize,
            to: filename
        )
    }

    private func renderSheet(_ model: JournalDevicesModel, to filename: String) async throws {
        try await render(
            JournalPairingWindow(model: model)
                .frame(width: sheetSize.width, height: sheetSize.height)
                .background(Color(nsColor: .windowBackgroundColor)),
            size: sheetSize,
            to: filename
        )
    }

    private func render<V: View>(_ view: V, size: CGSize, to filename: String) async throws {
        let hostingView = NSHostingView(rootView: view)
        hostingView.frame = NSRect(origin: .zero, size: size)
        hostingView.layoutSubtreeIfNeeded()

        guard let bitmapRep = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
            throw RenderError.noBitmap
        }
        hostingView.cacheDisplay(in: hostingView.bounds, to: bitmapRep)

        guard let pngData = bitmapRep.representation(using: .png, properties: [:]) else {
            throw RenderError.noPNG
        }

        let url = try outputDir.appendingPathComponent(filename)
        let bytesPerPixel = bitmapRep.bitsPerPixel / 8
        let bytesPerRow = bitmapRep.bytesPerRow
        let width = bitmapRep.pixelsWide
        let height = bitmapRep.pixelsHigh

        let bitmap: SnapshotBitmap?
        if let bitmapData = bitmapRep.bitmapData {
            bitmap = SnapshotBitmap(
                bytes: Array(UnsafeBufferPointer(start: bitmapData, count: bytesPerRow * height)),
                bytesPerPixel: bytesPerPixel,
                bytesPerRow: bytesPerRow,
                width: width,
                height: height
            )
        } else {
            bitmap = nil
        }

        try await SnapshotRenderPostProcessor.writePNGAndValidateContent(
            pngData: pngData,
            outputURL: url,
            filename: filename,
            bitmap: bitmap,
            emptyContent: RenderError.emptyContent
        )
    }

    private enum RenderError: Error, Sendable, CustomStringConvertible {
        case noBitmap
        case noPNG
        case emptyContent(filename: String, contentPixelCount: Int, minimum: Int)

        var description: String {
            switch self {
            case .noBitmap: return "failed to create bitmap rep"
            case .noPNG: return "failed to encode PNG"
            case let .emptyContent(filename, contentPixelCount, minimum):
                return "\(filename): \(contentPixelCount) content pixels below \(minimum) minimum"
            }
        }
    }
}

private struct SnapshotDevicesClient: JournalDevicesClientProtocol {
    let devices: [DeviceRow]

    func listDevices() async throws -> [DeviceRow] {
        devices
    }

    func startPairing() async throws -> PairStartResponse {
        throw JournalDevicesClientError.serverStatus(599)
    }

    func nonceStatus(nonce: String) async throws -> NonceStatusResponse {
        throw JournalDevicesClientError.serverStatus(599)
    }

    func unpairDevice(fingerprint: String) async throws -> UnpairResponse {
        throw JournalDevicesClientError.serverStatus(599)
    }
}
