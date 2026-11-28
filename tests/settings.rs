use monitor_hub_sdk_testkit::settings::{assert_no_drift, check};

use sei_monitor::config::SettingsDocument;

#[test]
fn the_document_structure_matches_the_declaration() {
    assert_no_drift::<SettingsDocument>(include_str!("../settings.json"));
}

#[test]
fn the_hub_testkit_accepts_the_settings() {
    let problems = check(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_BIN_EXE_sei-monitor"),
    );

    assert_eq!(problems, Vec::<String>::new());
}
