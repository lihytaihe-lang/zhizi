/// The release version of this build (the repository's VERSION file).  Kept in step by
/// script/sync_version.sh; the test harnesses report it as artifactVersion and the regression
/// gates compare it with the bundle's CFBundleShortVersionString.
public enum ReleaseVersion {
    public static let current = "0.1.5"
}
