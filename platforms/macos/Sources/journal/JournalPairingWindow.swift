// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import JournalMarkKit
import SolstoneCore
import SwiftUI

struct JournalPairingWindow: View {
    @Bindable var model: JournalDevicesModel
    @State private var copied = false
    @State private var copyResetTask: Task<Void, Never>?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            if !pairedAndRemoved {
                Text(title)
                    .font(.title2.weight(.semibold))
            }

            // Shown continuously alongside the QR/link, the whole time the owner is pairing a
            // new device — matching the connecting device's own confirm dialog, which shows the
            // same mark for comparison (journal-mark.md section 7.3). An unavailable mark renders
            // the unavailable card, and an unconfigured journal renders the section 4.3 generic mark.
            JournalMarkPresentationView(presentation: model.markPresentation)
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier(AXID.Journal.Devices.Pairing.markCard)

            Group {
                switch model.pairingState {
                case .idle:
                    EmptyView()
                case .opening:
                    openingContent
                case let .open(link, qr, _, _):
                    openContent(link: link, qr: qr)
                case let .paired(device, relay):
                    pairedContent(device: device, relay: relay)
                case .expired:
                    expiredContent
                case let .openFailed(detail):
                    failedContent(detail: detail)
                }
            }
            .accessibilityIdentifier(AXID.Journal.Devices.Pairing.status)
            AXStateCompanion(
                id: AXID.Journal.Devices.Pairing.statusState,
                value: model.pairingStatusToken
            )

            HStack {
                Spacer()
                Button(isPaired ? DevicesCopy.pairedDone : DevicesCopy.close) {
                    model.closePairing()
                }
            }
        }
        .padding(24)
        .frame(width: 420)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(AXID.Journal.Devices.Pairing.sheet)
        .onDisappear {
            copyResetTask?.cancel()
            copyResetTask = nil
        }
    }

    private var isPaired: Bool {
        if case .paired = model.pairingState { return true }
        return false
    }

    private var pairedAndRemoved: Bool {
        isPaired && model.pairedUnpairState == .removed
    }

    private var title: String {
        guard case let .paired(device, _) = model.pairingState else { return DevicesCopy.pairingTitle }
        let label = device?.displayLabel?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return label.isEmpty ? DevicesCopy.pairedHeadingUnnamed : DevicesCopy.pairedHeading(label)
    }

    private func pairedContent(device: NonceStatusResponse.Device?, relay: Bool) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if model.pairedUnpairState == .removed {
                Text(DevicesCopy.pairedRemoved)
            } else {
                Text(relay ? DevicesCopy.pairedNoteAnywhere : DevicesCopy.pairedNote)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if model.pairedUnpairState == .failed {
                    Text(DevicesCopy.pairedRemoveFailed)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if device != nil {
                    Button(DevicesCopy.pairedUnpair, role: .destructive) {
                        Task { await model.unpairPairedDevice() }
                    }
                    .disabled(model.pairedUnpairState == .busy)
                    .accessibilityIdentifier(AXID.Journal.Devices.Pairing.unpair)
                }
            }
            AXStateCompanion(
                id: AXID.Journal.Devices.Pairing.unpairState,
                value: model.pairedUnpairState.axToken
            )
        }
    }

    private var openingContent: some View {
        HStack(spacing: 8) {
            ProgressView()
                .scaleEffect(0.7)
            Text(DevicesCopy.pairingOpening)
                .foregroundStyle(.secondary)
        }
    }

    private func openContent(link: String, qr: NSImage) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(DevicesCopy.pairingInstructions)
                .foregroundStyle(.secondary)

            Image(nsImage: qr)
                .interpolation(.none)
                .resizable()
                .frame(width: 192, height: 192)
                .accessibilityIdentifier(AXID.Journal.Devices.Pairing.qr)
                .accessibilityLabel(DevicesCopy.pairingCode)

            VStack(alignment: .leading, spacing: 6) {
                Text(DevicesCopy.pairingLinkLabel)
                    .font(.headline)
                Text(link)
                    .font(.body.monospaced())
                    .lineLimit(2)
                    .textSelection(.enabled)
                    .accessibilityIdentifier(AXID.Journal.Devices.Pairing.linkField)
                    .accessibilityValue(link)
            }

            HStack(spacing: 10) {
                Button {
                    model.copyOpenPairingLink()
                    copied = true
                    copyResetTask?.cancel()
                    copyResetTask = Task { @MainActor in
                        try? await Task.sleep(for: .seconds(2))
                        copied = false
                    }
                } label: {
                    Label(DevicesCopy.copyLink, systemImage: "doc.on.doc")
                }
                .accessibilityIdentifier(AXID.Journal.Devices.Pairing.copyLink)

                if copied {
                    Text(DevicesCopy.copied)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                AXStateCompanion(
                    id: AXID.Journal.Devices.Pairing.copyLinkCopiedState,
                    value: (copied ? JournalDevicesCopiedState.copied : .idle).axToken
                )
            }

            let remainingSeconds = model.remainingPairingSeconds()
            Text(DevicesCopy.countdown(seconds: remainingSeconds))
                .foregroundStyle(.secondary)
                .accessibilityIdentifier(AXID.Journal.Devices.Pairing.countdown)
            AXStateCompanion(
                id: AXID.Journal.Devices.Pairing.countdownState,
                value: String(remainingSeconds)
            )
        }
    }

    private var expiredContent: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(DevicesCopy.linkExpiredTitle)
                .font(.headline)
            Text(DevicesCopy.linkExpiredBody)
                .foregroundStyle(.secondary)
            Button(DevicesCopy.openFreshLink) {
                model.openPairing()
            }
            .accessibilityIdentifier(AXID.Journal.Devices.Pairing.reopen)
        }
    }

    private func failedContent(detail: String) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(DevicesCopy.pairingFailedTitle)
                .font(.headline)
            if !detail.isEmpty {
                Text(detail)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
            Button(DevicesCopy.tryAgain) {
                model.openPairing()
            }
            .accessibilityIdentifier(AXID.Journal.Devices.Pairing.reopen)
        }
    }
}
