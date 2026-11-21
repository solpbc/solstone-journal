// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation

public enum JournalCopy {
    public static var language: String {
        let preferred = Bundle.module.preferredLocalizations.first ?? "en-US"
        return packagedLanguages.first { $0.caseInsensitiveCompare(preferred) == .orderedSame } ?? "en-US"
    }

    public static var requestedLanguage: String? {
        language.caseInsensitiveCompare("en-US") == .orderedSame ? nil : language
    }

    static func renderPlural(_ key: String, _ arguments: [CVarArg]) -> String {
        let format = Bundle.module.localizedString(forKey: key, value: nil, table: "Journal")
        return String(format: format, locale: Locale(identifier: language), arguments: arguments)
    }

    // Replace numbered string placeholders in one pass. Owner names, literal
    // percent signs and bidirectional controls keep their original bytes.
    private static let placeholders = try! NSRegularExpression(pattern: #"%%|%([1-9][0-9]*)\$@"#)

    static func render(_ key: String, _ arguments: [String]) -> String {
        let format = Bundle.module.localizedString(forKey: key, value: nil, table: "Journal")
        let source = format as NSString
        let rendered = NSMutableString(string: format)
        for match in placeholders.matches(in: format, range: NSRange(location: 0, length: source.length)).reversed() {
            let number = match.range(at: 1)
            let value: String
            if number.location == NSNotFound {
                value = "%"
            } else {
                let index = Int(source.substring(with: number))!
                value = arguments[index - 1]
            }
            rendered.replaceCharacters(in: match.range, with: value)
        }
        return rendered as String
    }
}
