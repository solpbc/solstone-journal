// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import Darwin
import Foundation
import JournalMarkKit
import os

@MainActor
protocol JournalIconApplying {
    func clearCustomIcon(atBundleAt path: String) -> Bool
    func setDockImage(_ image: NSImage?)
}

@MainActor
struct NSWorkspaceIconApplier: JournalIconApplying {
    func clearCustomIcon(atBundleAt path: String) -> Bool {
        NSWorkspace.shared.setIcon(nil, forFile: path, options: [])
    }

    func setDockImage(_ image: NSImage?) {
        NSApp?.applicationIconImage = image ?? JournalIconCompositor.appIcon(spec: nil)
    }
}

@MainActor
protocol JournalBundleIconInspecting {
    func hasCustomIcon(atBundleAt path: String) -> Bool
}

@MainActor
struct FinderInfoBundleIconInspector: JournalBundleIconInspecting {
    func hasCustomIcon(atBundleAt path: String) -> Bool {
        let iconFile = URL(fileURLWithPath: path, isDirectory: true)
            .appendingPathComponent("Icon\r")
        guard FileManager.default.fileExists(atPath: iconFile.path) else {
            return false
        }

        var finderInfo = [UInt8](repeating: 0, count: 32)
        let bytesRead = path.withCString { filePath in
            XATTR_FINDERINFO_NAME.withCString { attributeName in
                finderInfo.withUnsafeMutableBytes { buffer in
                    getxattr(filePath, attributeName, buffer.baseAddress, buffer.count, 0, 0)
                }
            }
        }
        guard bytesRead >= 10 else {
            return false
        }

        let flags = UInt16(finderInfo[8]) << 8 | UInt16(finderInfo[9])
        return flags & 0x0400 != 0
    }
}

@MainActor
protocol JournalIconLogging {
    func notice(_ message: String)
}

@MainActor
struct JournalLoggerIconLog: JournalIconLogging {
    func notice(_ message: String) {
        Logger.journalApp.notice("\(message, privacy: .public)")
    }
}

@MainActor
final class JournalIconManager {
    private let applier: any JournalIconApplying
    private let inspector: any JournalBundleIconInspecting
    private let logger: any JournalIconLogging
    private let bundlePath: String

    init(
        applier: any JournalIconApplying = NSWorkspaceIconApplier(),
        inspector: any JournalBundleIconInspecting = FinderInfoBundleIconInspector(),
        logger: any JournalIconLogging = JournalLoggerIconLog(),
        bundlePath: String = Bundle.main.bundlePath
    ) {
        self.applier = applier
        self.inspector = inspector
        self.logger = logger
        self.bundlePath = bundlePath
    }

    func start() {
        reassertOnLaunch()
    }

    func reassertOnLaunch() {
        // Older versions published an owner's mark on this shared bundle. Remove it
        // before any identity read; cached per-account marks may have come from TCP.
        if inspector.hasCustomIcon(atBundleAt: bundlePath), !applier.clearCustomIcon(atBundleAt: bundlePath) {
            logger.notice("the journal app's old custom icon could not be cleared. reinstall the journal app to remove it.")
        }
        applier.setDockImage(nil)
    }

    func handleIdentityMark(_ mark: JournalMark) {
        guard let validated = JournalMark.validate(mark) else {
            applier.setDockImage(nil)
            return
        }
        applier.setDockImage(JournalIconCompositor.appIcon(spec: validated))
    }
}
