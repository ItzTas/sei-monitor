use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub(crate) fn cases<T: DeserializeOwned>(file: &str, pointer: &str) -> Vec<T> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/golden")
        .join(format!("{file}.json"));
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("could not read {}: {err}", path.display()));
    let document: Value = serde_json::from_str(&text)
        .unwrap_or_else(|err| panic!("{} is not JSON: {err}", path.display()));
    let list = document
        .pointer(pointer)
        .unwrap_or_else(|| panic!("{} has nothing at {pointer:?}", path.display()));
    let cases = Vec::<T>::deserialize(list).unwrap_or_else(|err| {
        panic!(
            "{} at {pointer:?} has an unexpected shape: {err}",
            path.display()
        )
    });
    assert!(
        !cases.is_empty(),
        "{} at {pointer:?} has no cases",
        path.display()
    );
    cases
}
