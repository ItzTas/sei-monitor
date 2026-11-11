use std::fmt;

use zeroize::Zeroizing;

pub const REDACTED: &str = "[redacted]";

#[derive(Clone, Default)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Secret {
        Secret(Zeroizing::new(value))
    }

    pub fn reveal(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Login {
        username: &'static str,
        password: Secret,
    }

    #[test]
    fn secret_never_prints_its_value() {
        let secret = Secret::new("hunter2".to_owned());
        let login = Login {
            username: "fulano.tal",
            password: secret.clone(),
        };

        assert_eq!(format!("{secret}"), REDACTED);
        assert_eq!(format!("{secret:?}"), REDACTED);
        assert_eq!(format!("{secret:#?}"), REDACTED);
        assert_eq!(format!("{}", Secret::default()), REDACTED);
        assert_eq!(format!("{:?}", Secret::default()), REDACTED);
        for output in [format!("{login:?}"), format!("{login:#?}")] {
            assert!(output.contains(REDACTED), "{output}");
            assert!(output.contains(login.username), "{output}");
            assert!(!output.contains("hunter2"), "{output}");
        }
        assert_eq!(login.password.reveal(), "hunter2");
    }

    #[test]
    fn empty_secret_is_empty() {
        assert!(Secret::default().is_empty());
        assert!(Secret::new(String::new()).is_empty());
        assert!(!Secret::new("x".to_owned()).is_empty());
    }
}
