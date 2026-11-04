// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import SwiftUI

enum JournalSidebarTabState: CaseIterable {
    case selected
    case unselected
}

enum JournalEnabledState: CaseIterable {
    case enabled
    case disabled
}

enum JournalDevicesCopiedState: CaseIterable {
    case idle
    case copied
}

enum JournalFirstRunRouteState: CaseIterable {
    case deciding
    case welcome
    case folder
    case nameLocation
    case mark
    case thinking
    case setupProgress
    case finishInBrowser
    case adopting
    case home
}

enum JournalAdoptState: CaseIterable {
    case idle
    case opening
    case landed
    case blocked
    case failed
}

enum JournalRitualOperationToken: CaseIterable {
    case none
    case setup
    case mark
    case thinkingSave
    case thinkingFinalize
    case thinkingDownload
    case speechDownload
    case outbound
}

enum JournalOutboundToken: CaseIterable {
    case none
    case delivering
    case delivered
    case failed
}

extension JournalSidebarTabState {
    static let axTokens = ["selected", "unselected"]

    var axToken: String {
        switch self {
        case .selected: return "selected"
        case .unselected: return "unselected"
        }
    }
}

extension JournalRunDisplay {
    static let axTokens = ["starting", "running", "stopped", "blocked", "unknown"]

    var axToken: String {
        rawValue
    }
}

extension JournalHealthDisplay {
    static let axTokens = ["healthy", "stopped", "unknown"]

    var axToken: String {
        rawValue
    }
}

extension JournalEnabledState {
    static let axTokens = ["enabled", "disabled"]

    var axToken: String {
        switch self {
        case .enabled: return "enabled"
        case .disabled: return "disabled"
        }
    }
}

extension JournalDevicesLoadState {
    static let axTokens = ["loading", "loaded", "empty", "not_running", "not_ready"]

    var axToken: String {
        rawValue
    }
}

extension PairingState {
    static let axTokens = ["idle", "opening", "open", "paired", "expired", "open_failed"]

    var axToken: String {
        switch self {
        case .idle: return "idle"
        case .opening: return "opening"
        case .open: return "open"
        case .paired: return "paired"
        case .expired: return "expired"
        case .openFailed: return "open_failed"
        }
    }
}

enum JournalPairedUnpairState: String, CaseIterable, Sendable {
    case idle
    case busy
    case removed
    case failed

    static let axTokens = allCases.map(\.rawValue)

    var axToken: String {
        rawValue
    }
}

extension JournalDevicesCopiedState {
    static let axTokens = ["idle", "copied"]

    var axToken: String {
        switch self {
        case .idle: return "idle"
        case .copied: return "copied"
        }
    }
}

extension JournalFirstRunRouteState {
    static let axTokens = [
        "deciding", "welcome", "folder", "name_location", "mark", "thinking",
        "setup_progress", "finish_in_browser", "adopting", "home"
    ]

    var axToken: String {
        switch self {
        case .deciding: return "deciding"
        case .welcome: return "welcome"
        case .folder: return "folder"
        case .nameLocation: return "name_location"
        case .mark: return "mark"
        case .thinking: return "thinking"
        case .setupProgress: return "setup_progress"
        case .finishInBrowser: return "finish_in_browser"
        case .adopting: return "adopting"
        case .home: return "home"
        }
    }
}

extension JournalRitualOperationToken {
    static let axTokens = [
        "none", "setup", "mark", "thinking_save", "thinking_finalize",
        "thinking_download", "speech_download", "outbound",
    ]

    var axToken: String {
        switch self {
        case .none: return "none"
        case .setup: return "setup"
        case .mark: return "mark"
        case .thinkingSave: return "thinking_save"
        case .thinkingFinalize: return "thinking_finalize"
        case .thinkingDownload: return "thinking_download"
        case .speechDownload: return "speech_download"
        case .outbound: return "outbound"
        }
    }
}

extension JournalOutboundToken {
    static let axTokens = ["none", "delivering", "delivered", "failed"]

    var axToken: String {
        switch self {
        case .none: return "none"
        case .delivering: return "delivering"
        case .delivered: return "delivered"
        case .failed: return "failed"
        }
    }
}

extension JournalAdoptState {
    static let axTokens = ["idle", "opening", "landed", "blocked", "failed"]

    var axToken: String {
        switch self {
        case .idle: return "idle"
        case .opening: return "opening"
        case .landed: return "landed"
        case .blocked: return "blocked"
        case .failed: return "failed"
        }
    }
}

extension JournalFirstRunRoute {
    var axToken: String {
        switch self {
        case .deciding:
            return JournalFirstRunRouteState.deciding.axToken
        case .ritual(.welcome):
            return JournalFirstRunRouteState.welcome.axToken
        case .ritual(.folder):
            return JournalFirstRunRouteState.folder.axToken
        case .ritual(.nameLocation):
            return JournalFirstRunRouteState.nameLocation.axToken
        case .ritual(.mark):
            return JournalFirstRunRouteState.mark.axToken
        case .ritual(.thinking):
            return JournalFirstRunRouteState.thinking.axToken
        case .ritual(.setupProgress):
            return JournalFirstRunRouteState.setupProgress.axToken
        case .ritual(.finishInBrowser):
            return JournalFirstRunRouteState.finishInBrowser.axToken
        case .adopting:
            return JournalFirstRunRouteState.adopting.axToken
        case .home:
            return JournalFirstRunRouteState.home.axToken
        }
    }
}

extension JournalFirstRunModel {
    var adoptState: JournalAdoptState {
        if adoptMessage == JournalFirstRunCopy.adoptLandingLine { return .landed }
        if route == .home { return .landed }
        if errorMessage != nil { return .failed }
        if route == .adopting { return .opening }
        return .idle
    }
}
