// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import CoreGraphics
import Foundation
import JournalMarkKit

enum JournalIconGenError: Error {
    case chipLayoutUnavailable
    case wordLayoutUnavailable
}

@main
enum JournalIconGen {
    @MainActor
    static func main() throws {
        let outputPath = CommandLine.arguments.dropFirst().first ?? "assets/icon-journal.svg"
        let outputURL = URL(fileURLWithPath: outputPath)

        JournalMarkFont.register()

        let svg = try Self.svg()
        try FileManager.default.createDirectory(
            at: outputURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try svg.write(to: outputURL, atomically: true, encoding: .utf8)
    }

    @MainActor
    private static func svg() throws -> String {
        let tileSide: CGFloat = 1024
        guard let chipLayout = JournalIconTileGeometry.chipLayout(tileSide: tileSide, rotations: [0, 45]) else {
            throw JournalIconGenError.chipLayoutUnavailable
        }
        guard let wordLayout = JournalIconWordLayout.outlinedPaths(
            for: [
                JournalIconWordInput(word: "your", baselineY: 70),
                JournalIconWordInput(word: "journal", baselineY: 86)
            ],
            tileSide: tileSide
        ) else {
            throw JournalIconGenError.wordLayoutUnavailable
        }

        let chip1 = JournalIconTileGeometry.chipPath(
            center: chipLayout.centers[0],
            side: chipLayout.side,
            rotationDegrees: 0
        )
        let chip2 = JournalIconTileGeometry.chipPath(
            center: chipLayout.centers[1],
            side: chipLayout.side,
            rotationDegrees: 45
        )
        let borderWidth = JournalIconTileGeometry.ownerChipBorderWidth(chipSide: chipLayout.side)
        let dash = JournalIconTileGeometry.genericDashLengths(chipSide: chipLayout.side)
        let wordPaths = wordLayout.paths.map { JournalIconSVGPathData.string(from: $0) }

        return """
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" role="img" aria-label="journal">
          <title>journal</title>
          \(JournalIconTileGeometry.platePathElement)
          \(chipElement(path: chip1, color: JournalIconTileGeometry.genericChip1Hex, borderWidth: borderWidth, dash: dash))
          \(chipElement(path: chip2, color: JournalIconTileGeometry.genericChip2Hex, borderWidth: borderWidth, dash: dash))
          <path fill="\(JournalIconTileGeometry.wordInkHex)" d="\(wordPaths[0])"/>
          <path fill="\(JournalIconTileGeometry.wordInkHex)" d="\(wordPaths[1])"/>
        </svg>
        """
    }

    private static func chipElement(
        path: CGPath,
        color: String,
        borderWidth: CGFloat,
        dash: [CGFloat]
    ) -> String {
        let d = JournalIconSVGPathData.string(from: path)
        let strokeWidth = JournalIconSVGPathData.format(borderWidth)
        let dashText = dash.map { JournalIconSVGPathData.format($0) }.joined(separator: " ")
        let paper = JournalIconTileGeometry.genericPaperHex
        let backingWidth = JournalIconSVGPathData.format(borderWidth * JournalIconTileGeometry.genericBackingWidthRatio)
        return #"<path fill="\#(paper)" stroke="\#(paper)" stroke-width="\#(backingWidth)" stroke-linejoin="round" d="\#(d)"/>"#
            + "\n  "
            + #"<path fill="none" stroke="\#(color)" stroke-width="\#(strokeWidth)" stroke-dasharray="\#(dashText)" stroke-linejoin="round" d="\#(d)"/>"#
    }
}
