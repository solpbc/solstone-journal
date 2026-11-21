public enum UICopy {
    public static let JOURNAL_CHILD_BREAKER_TRIPPED = JournalCopy.runtimeJournalChildBreakerTripped()
    public static let JOURNAL_CHILD_CONTAINMENT_UNRESOLVED = JournalCopy.runtimeJournalChildContainmentUnresolved()
    public static let JOURNAL_MATERIALIZE_FAILED = JournalCopy.runtimeJournalMaterializeFailed()
    public static let JOURNAL_SPAWN_BLOCKED_PORTS = JournalCopy.runtimeJournalSpawnBlockedPorts()
    public static let JOURNAL_DRIVE_CANNOT_RUN = JournalCopy.runtimeJournalDriveCannotRun()
    public static let JOURNAL_SPAWN_PORT_CHECK_FAILED = JournalCopy.runtimeJournalSpawnPortCheckFailed()
    public static let JOURNAL_SPAWN_PORT_HELD_ELSEWHERE = JournalCopy.runtimeJournalSpawnPortHeldElsewhere()
    public static let JOURNAL_SPAWN_BLOCKED_LEGACY_SERVICE = JournalCopy.runtimeJournalSpawnBlockedLegacyService()
    public static let JOURNAL_SPAWN_SERVICE_CHECK_FAILED = JournalCopy.runtimeJournalSpawnServiceCheckFailed()
    public static let JOURNAL_READINESS_TIMEOUT = JournalCopy.runtimeJournalReadinessTimeout()
    public static let JOURNAL_SETUP_NEEDED_BEFORE_UPGRADE = JournalCopy.runtimeJournalSetupNeededBeforeUpgrade()
    public static let INSTALLER_READINESS_GATE_FAILED = JournalCopy.runtimeInstallerReadinessGateFailed()

    public static func installerVerifyIntegrityWarning(library: String) -> String {
        JournalCopy.runtimeCouldnTGetReadyContinuing(library)
    }
}
