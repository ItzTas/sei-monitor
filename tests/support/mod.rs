#![allow(dead_code)]

pub mod fake_sei;

pub fn ci() -> bool {
    std::env::var_os("CI").is_some_and(|value| !value.is_empty())
}
