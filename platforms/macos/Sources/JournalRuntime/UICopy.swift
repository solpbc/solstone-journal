public enum UICopy {
    public static let JOURNAL_CHILD_BREAKER_TRIPPED = "your journal stopped unexpectedly several times within a minute"
    public static let JOURNAL_CHILD_CONTAINMENT_UNRESOLVED = "your journal stopped, and not all of its processes could be confirmed closed"
    public static let JOURNAL_MATERIALIZE_FAILED = "journal runtime couldn't be prepared"
    public static let JOURNAL_SPAWN_BLOCKED_PORTS = "journal ports are still in use"
    public static let JOURNAL_DRIVE_CANNOT_RUN = "your journal can't run on this drive because the drive can't save files the way your journal needs. use a mac-formatted drive for your journal."
    public static let JOURNAL_SPAWN_PORT_CHECK_FAILED = "couldn't verify journal ports are free"
    public static let JOURNAL_SPAWN_BLOCKED_LEGACY_SERVICE = "journal service needs attention before starting"
    public static let JOURNAL_SPAWN_SERVICE_CHECK_FAILED = "couldn't verify journal service ownership"
    public static let JOURNAL_READINESS_TIMEOUT = "journal didn't become ready in time"
    public static let JOURNAL_SETUP_NEEDED_BEFORE_UPGRADE = "journal setup needed before upgrade can continue"
    public static let INSTALLER_READINESS_GATE_FAILED = "couldn't get the journal ready for this mac"

    public static func installerVerifyIntegrityWarning(library: String) -> String {
        "couldn't get \(library) ready; continuing"
    }
}
