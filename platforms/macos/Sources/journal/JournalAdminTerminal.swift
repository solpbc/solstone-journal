// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import Foundation
import os

/// "open admin terminal": a Terminal window in the owner's own login shell, where
/// `solstone` resolves first to this running app's bundled command line.
///
/// Nothing is installed. The app writes a small `.command` script and its shell startup
/// shims into a private temporary directory and hands the script to Terminal through
/// Launch Services, which sends no Apple Event the owner has to allow. Terminal runs the
/// script in a new window. The script starts the owner's login shell so that their own
/// startup files still run, and the bundled directory goes on PATH only after the last
/// of them. As the shell starts, it deletes the directory.
enum JournalAdminTerminal {
    static let menuItemTitle = "open admin terminal"
    static let failureTitle = "couldn't open the admin terminal"
    static let terminalBundleIdentifier = "com.apple.Terminal"

    /// The menu command: opens the terminal, or says why it could not.
    @MainActor
    static func openFromMenu() {
        open { error in
            guard let error else { return }
            Logger.journalApp.error("open admin terminal failed: \(error.localizedDescription)")
            let alert = NSAlert()
            alert.messageText = failureTitle
            alert.informativeText = error.localizedDescription
            alert.runModal()
        }
    }

    /// The bundled command line of the app at `bundleURL`: the running app, wherever it is.
    static func binDirectory(bundleURL: URL) -> URL {
        bundleURL
            .resolvingSymlinksInPath()
            .appendingPathComponent("Contents/Resources/solstone-runtime/bin", isDirectory: true)
    }

    /// Writes the script and its shims into a new private directory under `temporaryDirectory`
    /// and returns the script.
    static func prepare(binDirectory: URL, temporaryDirectory: URL, fileManager: FileManager = .default) throws -> URL {
        let directory = temporaryDirectory
            .appendingPathComponent("journal-admin-terminal-\(UUID().uuidString)", isDirectory: true)
        let privateDirectory: [FileAttributeKey: Any] = [.posixPermissions: 0o700]
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: false, attributes: privateDirectory)
        let zshDirectory = directory.appendingPathComponent("zsh", isDirectory: true)
        try fileManager.createDirectory(at: zshDirectory, withIntermediateDirectories: false, attributes: privateDirectory)
        try write(zshStartup, to: zshDirectory.appendingPathComponent(".zshenv"), permissions: 0o600)
        try write(bashStartup, to: directory.appendingPathComponent("bashrc"), permissions: 0o600)
        let script = directory.appendingPathComponent("admin-terminal.command")
        try write(commandScript(binDirectory: binDirectory.path), to: script, permissions: 0o700)
        return script
    }

    /// Opens the admin terminal for the running app. The completion reports on the main actor.
    @MainActor
    static func open(
        bundleURL: URL = Bundle.main.bundleURL,
        workspace: NSWorkspace = .shared,
        completion: @escaping @MainActor (Error?) -> Void
    ) {
        guard let terminal = workspace.urlForApplication(withBundleIdentifier: terminalBundleIdentifier) else {
            completion(CocoaError(.fileNoSuchFile))
            return
        }
        let script: URL
        do {
            script = try prepare(
                binDirectory: binDirectory(bundleURL: bundleURL),
                temporaryDirectory: FileManager.default.temporaryDirectory
            )
        } catch {
            completion(error)
            return
        }
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = true
        workspace.open([script], withApplicationAt: terminal, configuration: configuration) { _, error in
            if error != nil {
                try? FileManager.default.removeItem(at: script.deletingLastPathComponent())
            }
            Task { @MainActor in completion(error) }
        }
    }

    /// Wraps `value` in single quotes for a POSIX shell.
    static func shellQuoted(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    /// The script Terminal runs. It starts the owner's login shell (`$SHELL` in Terminal's
    /// own session) with the startup shim for zsh or bash, so the bundled directory goes on
    /// PATH after the owner's startup files. fish takes it after its config through `-C`. Any other
    /// shell gets it before its startup files, so those may still put another `solstone` first.
    static func commandScript(binDirectory: String) -> String {
        """
        #!/bin/sh
        # Written by the journal app's "open admin terminal". It starts your own login shell
        # with this journal app's command line first on PATH, and removes itself as the shell
        # starts. Nothing is installed.
        JOURNAL_ADMIN_BIN=\(shellQuoted(binDirectory))
        JOURNAL_ADMIN_DIR=$(cd -P -- "$(dirname -- "$0")" && pwd) || exit 1
        export JOURNAL_ADMIN_BIN JOURNAL_ADMIN_DIR
        /bin/rm -f -- "$0"
        shell=${SHELL:-/bin/zsh}
        case ${shell##*/} in
        zsh)
            if [ -n "${ZDOTDIR+set}" ]; then JOURNAL_ADMIN_HAD_ZDOTDIR=1; fi
            JOURNAL_ADMIN_ZDOTDIR=${ZDOTDIR:-$HOME}
            ZDOTDIR=$JOURNAL_ADMIN_DIR/zsh
            export JOURNAL_ADMIN_HAD_ZDOTDIR JOURNAL_ADMIN_ZDOTDIR ZDOTDIR
            exec "$shell" -l
            ;;
        bash)
            exec "$shell" --rcfile "$JOURNAL_ADMIN_DIR/bashrc" -i
            ;;
        fish)
            /bin/rm -rf -- "$JOURNAL_ADMIN_DIR"
            exec "$shell" -l -C 'set -gx PATH $JOURNAL_ADMIN_BIN $PATH; set -e JOURNAL_ADMIN_BIN JOURNAL_ADMIN_DIR'
            ;;
        *)
            /bin/rm -rf -- "$JOURNAL_ADMIN_DIR"
            PATH=$JOURNAL_ADMIN_BIN:$PATH
            export PATH
            unset JOURNAL_ADMIN_BIN JOURNAL_ADMIN_DIR
            exec "$shell" -l
            ;;
        esac

        """
    }

    /// zsh reads its first startup file, `.zshenv`, from `$ZDOTDIR`, which the script points
    /// here. This one hands `ZDOTDIR` back to the owner before reading their own `.zshenv`, so
    /// every later startup file, the system's included, is theirs. It removes the directory
    /// and leaves a one-time prompt hook: the first prompt comes after the last startup file,
    /// and that is when the bundled directory goes first on PATH.
    static let zshStartup = """
        ZDOTDIR=$JOURNAL_ADMIN_ZDOTDIR
        if [[ -z ${JOURNAL_ADMIN_HAD_ZDOTDIR:-} ]]; then unset ZDOTDIR; fi
        /bin/rm -rf -- "$JOURNAL_ADMIN_DIR"
        unset JOURNAL_ADMIN_DIR JOURNAL_ADMIN_ZDOTDIR JOURNAL_ADMIN_HAD_ZDOTDIR
        _journal_admin_path() {
            export PATH="$JOURNAL_ADMIN_BIN:$PATH"
            unset JOURNAL_ADMIN_BIN
            precmd_functions=(${precmd_functions:#_journal_admin_path})
            unfunction _journal_admin_path
        }
        precmd_functions+=(_journal_admin_path)
        if [[ -r ${ZDOTDIR:-$HOME}/.zshenv ]]; then builtin source -- "${ZDOTDIR:-$HOME}/.zshenv"; fi

        """

    /// bash reads no rc file after a login shell's profile, so the admin terminal runs an
    /// interactive bash on this file, which reads the owner's login files in a login shell's
    /// order and then puts the bundled directory first.
    static let bashStartup = """
        if [ -r /etc/profile ]; then . /etc/profile; fi
        if [ -r "$HOME/.bash_profile" ]; then . "$HOME/.bash_profile"
        elif [ -r "$HOME/.bash_login" ]; then . "$HOME/.bash_login"
        elif [ -r "$HOME/.profile" ]; then . "$HOME/.profile"
        fi
        PATH="$JOURNAL_ADMIN_BIN:$PATH"
        export PATH
        /bin/rm -rf -- "$JOURNAL_ADMIN_DIR"
        unset JOURNAL_ADMIN_BIN JOURNAL_ADMIN_DIR

        """

    private static func write(_ contents: String, to url: URL, permissions: Int) throws {
        try Data(contents.utf8).write(to: url, options: .withoutOverwriting)
        try FileManager.default.setAttributes([.posixPermissions: permissions], ofItemAtPath: url.path)
    }
}
