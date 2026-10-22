//! Release identity is the signed bundle's identity, not the Cargo package version.

pub fn beta_enabled(channel: Option<&str>) -> bool {
    channel == Some("beta")
}

pub fn version() -> String {
    bundle_value("CFBundleShortVersionString")
        .unwrap_or_else(|| env!("SCRAWL_RELEASE_VERSION").to_owned())
}

pub fn build() -> String {
    bundle_value("CFBundleVersion").unwrap_or_else(|| "dev".into())
}

pub fn is_beta() -> bool {
    version()
        .split_once("-beta.")
        .is_some_and(|(_, n)| n.parse::<u32>().is_ok_and(|n| n > 0))
}

pub fn display_version() -> String {
    format!("{} ({})", version(), build())
}

#[cfg(target_os = "macos")]
fn bundle_value(key: &str) -> Option<String> {
    use objc2_foundation::{NSBundle, NSString};
    NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str(key))
        .and_then(|value| value.downcast::<NSString>().ok())
        .map(|value| value.to_string())
}

#[cfg(not(target_os = "macos"))]
fn bundle_value(_: &str) -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
pub fn report_problem() {
    let os = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".into());
    let body = format!(
        "Scrawl {}\nBuild: {}\nmacOS: {os}\n\nWhat happened?\n\nSteps to reproduce:\n\nExpected behavior:\n",
        version(),
        build()
    );
    let encoded: String = body.bytes().map(|b| format!("%{b:02X}")).collect();
    let url = format!(
        "https://github.com/kitlangton/scrawl/issues/new?labels=beta&title=%5BBeta%5D%20&body={encoded}"
    );
    let _ = std::process::Command::new("open").arg(url).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beta_requires_explicit_opt_in() {
        for value in [
            None,
            Some("stable"),
            Some("Beta"),
            Some("nightly"),
            Some(""),
        ] {
            assert!(!beta_enabled(value));
        }
        assert!(beta_enabled(Some("beta")));
    }
}
